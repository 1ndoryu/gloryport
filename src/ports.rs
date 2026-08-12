//! Escáner de puertos TCP en escucha (solo Windows).
//!
//! Usa `GetExtendedTcpTable` (iphlpapi) directamente: una sola llamada Win32 por familia
//! (IPv4/IPv6), sin invocar `netstat` ni procesos externos. El costo es mínimo y la
//! latencia, acotada, por lo que puede ejecutarse bajo demanda en cada apertura del menú.

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};

use crate::process;

/// Valores de `AF_INET`/`AF_INET6` para `GetExtendedTcpTable`.
const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;

/// Máximo de entradas en la caché de nombres de proceso (acota la memoria).
const NAME_CACHE_MAX: usize = 512;
/// TTL de la caché: evita abrir el mismo proceso repetidamente entre aperturas del menú.
const NAME_CACHE_TTL: Duration = Duration::from_secs(10);

/// Un puerto TCP en escucha y el proceso que lo ocupa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    pub port: u16,
    pub pid: u32,
    pub address: String,
    pub process_name: String,
    /// Ruta completa del ejecutable (p. ej. `C:\Program Files\nodejs\node.exe`).
    /// `None` cuando el proceso ya no existe o Windows niega el acceso.
    pub process_path: Option<String>,
}

impl PortInfo {
    fn from_v4(row: &MIB_TCPROW_OWNER_PID) -> Self {
        Self {
            port: decode_port(row.dwLocalPort),
            pid: row.dwOwningPid,
            address: ipv4_to_string(row.dwLocalAddr),
            process_name: String::new(),
            process_path: None,
        }
    }

    fn from_v6(row: &MIB_TCP6ROW_OWNER_PID) -> Self {
        Self {
            port: decode_port(row.dwLocalPort),
            pid: row.dwOwningPid,
            address: ipv6_to_string(&row.ucLocalAddr),
            process_name: String::new(),
            process_path: None,
        }
    }
}

/// Error del escáner, con el código Win32 original para diagnóstico.
#[derive(Debug)]
pub struct ScanError {
    pub message: String,
    pub code: u32,
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (código Win32 {})", self.message, self.code)
    }
}

/// Escanea todos los puertos TCP en escucha (IPv4 + IPv6), deduplicados y ordenados.
pub fn scan_listeners() -> Result<Vec<PortInfo>, ScanError> {
    let mut out = Vec::new();
    collect_v4(&mut out)?;
    collect_v6(&mut out)?;
    dedupe_and_sort(&mut out);
    Ok(out)
}

/// Adjunta el nombre del proceso a cada fila usando una caché con TTL.
pub fn attach_process_names(rows: &mut [PortInfo], cache: &mut NameCache) {
    for row in rows.iter_mut() {
        let (name, path) = cache.get(row.pid);
        row.process_name = name;
        row.process_path = path;
    }
}

/// Filtra la lista dejando solo puertos de **aplicaciones de usuario**.
///
/// Regla: puerto ≥ 1024 (los inferiores son privilegiados/servicios de Windows) y
/// ejecutable resoluble **fuera** de la carpeta Windows. Esto oculta servicios del
/// sistema (svchost, System, RPC…) y procesos muertos/sin permiso ("desconocido"),
/// que nunca deben cerrarse desde la bandeja.
pub fn solo_aplicaciones(rows: Vec<PortInfo>) -> Vec<PortInfo> {
    rows.into_iter().filter(es_puerto_de_aplicacion).collect()
}

fn es_puerto_de_aplicacion(row: &PortInfo) -> bool {
    if row.port < 1024 {
        return false;
    }
    row.process_path
        .as_deref()
        .is_some_and(|p| !es_ruta_del_sistema(p))
}

/// ¿La ruta está dentro de la carpeta Windows del sistema (case-insensitive)?
fn es_ruta_del_sistema(path: &str) -> bool {
    let root = std::env::var("SystemRoot")
        .unwrap_or_else(|_| "C:\\Windows".to_string())
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    let p = path.trim_end_matches('\\').to_ascii_lowercase();
    p == root || p.starts_with(&format!("{root}\\"))
}

/// Caché acotada de nombres de proceso: evita llamadas Win32 repetidas.
pub struct NameCache {
    entries: HashMap<u32, ResolvedProcess>,
}

struct ResolvedProcess {
    name: String,
    path: Option<String>,
    at: Instant,
}

impl Default for NameCache {
    fn default() -> Self {
        Self::new()
    }
}

impl NameCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn get(&mut self, pid: u32) -> (String, Option<String>) {
        if let Some(entry) = self.entries.get(&pid) {
            if entry.at.elapsed() < NAME_CACHE_TTL {
                return (entry.name.clone(), entry.path.clone());
            }
        }
        // Una sola apertura del proceso resuelve ruta y nombre derivado.
        let path = process::resolve_process_path(pid);
        let name = path
            .as_deref()
            .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
            .unwrap_or_else(|| "desconocido".to_string());
        let entry = ResolvedProcess {
            name: name.clone(),
            path: path.clone(),
            at: Instant::now(),
        };
        self.entries.insert(pid, entry);
        self.evict_if_needed();
        (name, path)
    }

    fn evict_if_needed(&mut self) {
        if self.entries.len() <= NAME_CACHE_MAX {
            return;
        }
        self.entries.retain(|_, e| e.at.elapsed() < NAME_CACHE_TTL);
        if self.entries.len() > NAME_CACHE_MAX {
            // Caso límite (muchos PIDs nuevos): resetear evita crecer sin tope.
            self.entries.clear();
        }
    }
}

fn collect_v4(out: &mut Vec<PortInfo>) -> Result<(), ScanError> {
    let rows = unsafe { tcp_table_v4()? };
    out.extend(rows.iter().map(PortInfo::from_v4));
    Ok(())
}

fn collect_v6(out: &mut Vec<PortInfo>) -> Result<(), ScanError> {
    let rows = unsafe { tcp_table_v6()? };
    out.extend(rows.iter().map(PortInfo::from_v6));
    Ok(())
}

/// Llama a `GetExtendedTcpTable` dos veces (tamaño + datos) y devuelve las filas IPv4.
///
/// `unsafe`: los punteros se derivan de un buffer propio con alineación 4 (Vec<u32>),
/// suficiente para las estructuras MIB (solo campos u32/[u8;16]).
unsafe fn tcp_table_v4() -> Result<Vec<MIB_TCPROW_OWNER_PID>, ScanError> {
    let mut size: u32 = 0;
    let r = GetExtendedTcpTable(
        Some(std::ptr::null_mut()),
        &mut size,
        true,
        AF_INET,
        TCP_TABLE_OWNER_PID_LISTENER,
        0,
    );
    if r != ERROR_INSUFFICIENT_BUFFER.0 {
        return Err(ScanError {
            message: "GetExtendedTcpTable (IPv4) no devolvió el tamaño".into(),
            code: r,
        });
    }
    let mut buf: Vec<u32> = vec![0; size as usize / 4 + 1];
    let r = GetExtendedTcpTable(
        Some(buf.as_mut_ptr() as *mut c_void),
        &mut size,
        true,
        AF_INET,
        TCP_TABLE_OWNER_PID_LISTENER,
        0,
    );
    if r != NO_ERROR.0 {
        return Err(ScanError {
            message: "GetExtendedTcpTable (IPv4) falló".into(),
            code: r,
        });
    }
    let table = &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
    Ok(std::slice::from_raw_parts(
        (&table.table[0]) as *const MIB_TCPROW_OWNER_PID,
        table.dwNumEntries as usize,
    )
    .to_vec())
}

unsafe fn tcp_table_v6() -> Result<Vec<MIB_TCP6ROW_OWNER_PID>, ScanError> {
    let mut size: u32 = 0;
    let r = GetExtendedTcpTable(
        Some(std::ptr::null_mut()),
        &mut size,
        true,
        AF_INET6,
        TCP_TABLE_OWNER_PID_LISTENER,
        0,
    );
    if r != ERROR_INSUFFICIENT_BUFFER.0 {
        return Err(ScanError {
            message: "GetExtendedTcpTable (IPv6) no devolvió el tamaño".into(),
            code: r,
        });
    }
    let mut buf: Vec<u32> = vec![0; size as usize / 4 + 1];
    let r = GetExtendedTcpTable(
        Some(buf.as_mut_ptr() as *mut c_void),
        &mut size,
        true,
        AF_INET6,
        TCP_TABLE_OWNER_PID_LISTENER,
        0,
    );
    if r != NO_ERROR.0 {
        return Err(ScanError {
            message: "GetExtendedTcpTable (IPv6) falló".into(),
            code: r,
        });
    }
    let table = &*(buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID);
    Ok(std::slice::from_raw_parts(
        (&table.table[0]) as *const MIB_TCP6ROW_OWNER_PID,
        table.dwNumEntries as usize,
    )
    .to_vec())
}

/// Decodifica el puerto guardado en network byte order dentro de un `u32`.
fn decode_port(raw: u32) -> u16 {
    u16::from_be((raw & 0xFFFF) as u16)
}

fn ipv4_to_string(raw: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        (raw >> 24) & 0xFF,
        (raw >> 16) & 0xFF,
        (raw >> 8) & 0xFF,
        raw & 0xFF
    )
}

/// Formato IPv6 con compresión básica de la corrida más larga de ceros (`::`),
/// suficiente para identificación en menú/tooltip.
fn ipv6_to_string(addr: &[u8; 16]) -> String {
    let groups: Vec<u16> = addr
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();

    // Corrida más larga de grupos cero (mínimo 2 para que valga la pena comprimir).
    let mut best = (0usize, 0usize);
    let mut i = 0;
    while i < groups.len() {
        if groups[i] == 0 {
            let start = i;
            while i < groups.len() && groups[i] == 0 {
                i += 1;
            }
            let len = i - start;
            if len > best.1 {
                best = (start, len);
            }
        } else {
            i += 1;
        }
    }

    let (start, len) = best;
    let prefix: Vec<String> = groups[..start].iter().map(|g| format!("{g:x}")).collect();
    let suffix: Vec<String> = groups[start + len..]
        .iter()
        .map(|g| format!("{g:x}"))
        .collect();

    if len == 0 {
        return format!("[{}]", prefix.join(":"));
    }

    let mut s = String::from("[");
    if !prefix.is_empty() {
        s.push_str(&prefix.join(":"));
    }
    s.push_str("::");
    if !suffix.is_empty() {
        s.push_str(&suffix.join(":"));
    }
    s.push(']');
    s
}

fn dedupe_and_sort(rows: &mut Vec<PortInfo>) {
    rows.sort_by(|a, b| (a.port, a.pid, &a.address).cmp(&(b.port, b.pid, &b.address)));
    rows.dedup_by(|a, b| a.port == b.port && a.pid == b.pid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_port_from_network_order() {
        // 3000 = 0x0BB8; en memoria (u32 LE) queda 0x0000B80B.
        assert_eq!(decode_port(0x0000_B80B), 3000);
        assert_eq!(decode_port(0x0000_5000), 80);
        assert_eq!(decode_port(0x0000_FF00), 255);
    }

    #[test]
    fn ipv4_formatting() {
        assert_eq!(ipv4_to_string(0x7F00_0001), "127.0.0.1");
        assert_eq!(ipv4_to_string(0), "0.0.0.0");
    }

    #[test]
    fn ipv6_formatting() {
        assert_eq!(ipv6_to_string(&[0; 16]), "[::]");
        let localhost = {
            let mut a = [0u8; 16];
            a[15] = 1;
            a
        };
        assert_eq!(ipv6_to_string(&localhost), "[::1]");
        let doc = {
            let mut a = [0u8; 16];
            a[0] = 0x20;
            a[1] = 0x01;
            a[2] = 0x0d;
            a[3] = 0xb8;
            a[15] = 1;
            a
        };
        assert_eq!(ipv6_to_string(&doc), "[2001:db8::1]");
    }

    #[test]
    fn dedupe_and_order() {
        let mut rows = vec![
            PortInfo {
                port: 3000,
                pid: 100,
                address: "0.0.0.0".into(),
                process_name: String::new(),
                process_path: None,
            },
            PortInfo {
                port: 80,
                pid: 4,
                address: "0.0.0.0".into(),
                process_name: String::new(),
                process_path: None,
            },
            PortInfo {
                port: 3000,
                pid: 100,
                address: "[::]".into(),
                process_name: String::new(),
                process_path: None,
            },
            PortInfo {
                port: 3000,
                pid: 101,
                address: "127.0.0.1".into(),
                process_name: String::new(),
                process_path: None,
            },
        ];
        dedupe_and_sort(&mut rows);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].port, 80);
        assert_eq!(rows[1].port, 3000);
        assert_eq!(rows[1].pid, 100);
        assert_eq!(rows[2].pid, 101);
    }

    #[test]
    fn name_cache_evicts_and_reuses() {
        let mut cache = NameCache::new();
        let (a, path) = cache.get(u32::MAX); // PID improbable: sin ruta ni nombre
        assert_eq!(a, "desconocido");
        assert!(path.is_none());
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn filtro_solo_aplicaciones() {
        fn row(port: u16, path: Option<&str>) -> PortInfo {
            PortInfo {
                port,
                pid: 1000,
                address: "0.0.0.0".into(),
                process_name: String::new(),
                process_path: path.map(str::to_string),
            }
        }

        let rows = vec![
            row(80, Some(r"C:\Windows\System32\svchost.exe")), // servicio web/sistema
            row(135, None),                                    // RPC sin nombre resoluble
            row(445, Some(r"C:\Windows\System32\srvnet.sys")), // SMB (path Windows)
            row(49664, Some(r"C:\Windows\System32\svchost.exe")), // RPC dinámico
            row(3000, Some(r"C:\Program Files\nodejs\node.exe")), // app de usuario
            row(5432, Some(r"C:\Program Files\PostgreSQL\bin\postgres.exe")),
            row(8080, None), // proceso muerto/sin permiso: no confirmable
            row(80, Some(r"C:\Program Files\nodejs\node.exe")), // puerto privilegiado
            row(9000, Some(r"c:\windows\system32\dwm.exe")), // case-insensitive
        ];

        let kept = solo_aplicaciones(rows);
        let kept_ports: Vec<u16> = kept.iter().map(|r| r.port).collect();
        assert_eq!(kept_ports, vec![3000, 5432]);
    }
}
