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
/// Procesos que nunca se ofrecen al kill aunque cumplan el filtro de aplicación:
/// sincronizadores del sistema y apps del entorno (Intel, la propia Freebuff) que
/// no deben cerrarse desde la bandeja. También acepta scripts (p. ej. el
/// orchestrator.js de Freebuff, que corre bajo bun.exe).
const PROCESOS_EXCLUIDOS: &[&str] = &[
    "googledrivefs.exe", // sincronizador del sistema
    "esrv.exe",          // Intel SUR/actualizador: no es del workspace
    "freebuff.exe",      // la propia app Freebuff
    "orchestrator.js",   // orquestador interno de Freebuff (bajo bun.exe)
];

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
    /// Línea de comandos completa del proceso (p. ej. `node ...\server.js`).
    /// `None` cuando no es legible (sistema/otro usuario) o el proceso murió.
    pub process_cmd: Option<String>,
    /// Proyecto del área de trabajo al que pertenece el proceso: la carpeta
    /// inmediatamente posterior a la raíz del workspace en su cmdline/ruta.
    /// `None` cuando no cae en el workspace o no se puede resolver.
    #[serde(default)]
    pub proyecto: Option<String>,
}

impl PortInfo {
    fn from_v4(row: &MIB_TCPROW_OWNER_PID) -> Self {
        Self {
            port: decode_port(row.dwLocalPort),
            pid: row.dwOwningPid,
            address: ipv4_to_string(row.dwLocalAddr),
            process_name: String::new(),
            process_path: None,
            process_cmd: None,
            proyecto: None,
        }
    }

    fn from_v6(row: &MIB_TCP6ROW_OWNER_PID) -> Self {
        Self {
            port: decode_port(row.dwLocalPort),
            pid: row.dwOwningPid,
            address: ipv6_to_string(&row.ucLocalAddr),
            process_name: String::new(),
            process_path: None,
            process_cmd: None,
            proyecto: None,
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

/// Adjunta el nombre del proceso a cada fila usando una caché con TTL, y deriva
/// el proyecto del área de trabajo desde el cmdline/ruta recién resueltos.
pub fn attach_process_names(
    rows: &mut [PortInfo],
    cache: &mut NameCache,
    cfg: &crate::config::Config,
) {
    for row in rows.iter_mut() {
        let (name, path, cmd) = cache.get(row.pid);
        row.process_name = name;
        row.process_path = path;
        row.process_cmd = cmd;
        row.proyecto = proyecto_para(row, cfg);
    }
}

/// Extensiones de script que delatan el "programa real" detrás de un intérprete
/// (node, bun, deno…): la primera ruta con estas extensiones en la línea de
/// comandos es la aplicación que realmente sirve el puerto.
const EXTENSIONES_SCRIPT: &[&str] = &["js", "mjs", "cjs", "ts", "mts", "cts"];

/// Etiqueta legible del proceso para popup/CLI.
///
/// Para intérpretes muestra el script que ejecutan (`…\codex-bridge\bridge\server.js`
/// en vez de `node.exe`); para el resto, el nombre del ejecutable. Deriva del
/// proceso real de cada escaneo: **nunca** mapea puerto→aplicación, porque una
/// misma app puede ocupar puertos distintos en días distintos.
pub fn etiqueta_visible(row: &PortInfo) -> String {
    if let Some(cmd) = row.process_cmd.as_deref() {
        if let Some(script) = primer_script(cmd) {
            return acortar_ruta(&normalizar_ruta(&script));
        }
    }
    row.process_name.clone()
}

/// Proyecto del área de trabajo al que pertenece el proceso, derivado de su
/// línea de comandos (y, si falta, de la ruta del ejecutable): la carpeta
/// inmediatamente posterior a la raíz del workspace (`area-trabajo\gloryapi\…`
/// → `gloryapi`). Nada hardcodeado: sale de la ruta real del proceso, así que
/// si el proyecto cambia de carpeta la etiqueta cambia sola. Un alias manual
/// de la config por puerto tiene prioridad sobre la derivación.
pub fn proyecto_para(row: &PortInfo, cfg: &crate::config::Config) -> Option<String> {
    if let Some(alias) = cfg.alias_para(row.port) {
        return Some(alias.to_string());
    }
    let raiz = cfg.workspace_raiz();
    for texto in [row.process_cmd.as_deref(), row.process_path.as_deref()]
        .into_iter()
        .flatten()
    {
        if let Some(proyecto) = proyecto_en_ruta(texto, raiz) {
            return Some(proyecto);
        }
    }
    None
}

/// Primera carpeta tras la raíz del workspace en una ruta (case-insensitive).
fn proyecto_en_ruta(texto: &str, raiz: &str) -> Option<String> {
    let comps: Vec<&str> = texto.split(['\\', '/']).filter(|c| !c.is_empty()).collect();
    comps
        .iter()
        .position(|c| c.eq_ignore_ascii_case(raiz))
        .and_then(|i| comps.get(i + 1))
        .map(|p| (*p).to_string())
}

/// Etiqueta de fila del popup: `Proyecto · proceso` cuando el proyecto se
/// conoce, solo el proceso en caso contrario (app externa, sistema…).
pub fn etiqueta_popup(row: &PortInfo) -> String {
    let proceso = etiqueta_visible(row);
    match row.proyecto.as_deref() {
        Some(proyecto) if !proyecto.is_empty() => {
            // Con el proyecto como prefijo, el `…\` del comienzo de la ruta es
            // ruido: la ruta acortada se muestra sin él.
            let proceso = proceso.strip_prefix("…\\").unwrap_or(&proceso);
            format!("{proyecto} · {proceso}")
        }
        _ => proceso,
    }
}

/// Primer argumento de la línea de comandos que parece un script (extensión
/// conocida), saltando flags y el propio ejecutable.
fn primer_script(cmd: &str) -> Option<String> {
    tokenizar_cmdline(cmd).into_iter().skip(1).find(|token| {
        if token.starts_with('-') {
            return false;
        }
        let ext = token.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        EXTENSIONES_SCRIPT.contains(&ext.as_str())
    })
}

/// Tokeniza una línea de comandos de Windows respetando comillas dobles.
fn tokenizar_cmdline(cmd: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in cmd.chars() {
        match c {
            '"' => in_quote = !in_quote,
            ' ' | '\t' if !in_quote => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// Normaliza la ruta textualmente (colapsa `.` y `..`) sin tocar el disco:
/// convierte `node_modules\.bin\..\vite\bin\vite.js` en la ruta real del script.
fn normalizar_ruta(ruta: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for comp in ruta.split(['\\', '/']) {
        match comp {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    let prefix = if ruta.starts_with("\\\\") { "\\\\" } else { "" };
    format!("{prefix}{}", out.join("\\"))
}

/// Acorta la ruta a sus últimos 3 componentes para que quepa en la fila del popup
/// conservando lo identificable (`…\codex-bridge\bridge\server.js`).
fn acortar_ruta(ruta: &str) -> String {
    let comps: Vec<&str> = ruta.split(['\\', '/']).filter(|c| !c.is_empty()).collect();
    if comps.len() <= 3 {
        return ruta.to_string();
    }
    format!("…\\{}", comps[comps.len() - 3..].join("\\"))
}

/// Filtra la lista dejando solo puertos de **aplicaciones de usuario**.
///
/// Regla: puerto ≥ 1024 (los inferiores son privilegiados/servicios de Windows),
/// ejecutable resoluble **fuera** de la carpeta Windows y proceso no incluido en la
/// blocklist. Esto oculta servicios del sistema (svchost, System, RPC…), procesos
/// muertos/sin permiso ("desconocido") y sincronizadores (p. ej. GoogleDriveFS),
/// que nunca deben cerrarse desde la bandeja.
pub fn solo_aplicaciones(rows: Vec<PortInfo>, cfg: &crate::config::Config) -> Vec<PortInfo> {
    rows.into_iter()
        .filter(|r| es_puerto_de_aplicacion(r, cfg))
        .collect()
}

fn es_puerto_de_aplicacion(row: &PortInfo, cfg: &crate::config::Config) -> bool {
    if row.port < 1024 {
        return false;
    }
    if esta_excluido(row) {
        return false;
    }
    if esta_oculto_por_config(row, cfg) {
        return false;
    }
    row.process_path
        .as_deref()
        .is_some_and(|p| !es_ruta_del_sistema(p))
}

/// ¿El usuario marcó este puerto como oculto en la config? Matchea el proyecto
/// derivado, el nombre del proceso, el basename del path y el script del cmdline,
/// case-insensitive (p. ej. ocultar el proyecto `gloryapi` completo).
fn esta_oculto_por_config(row: &PortInfo, cfg: &crate::config::Config) -> bool {
    let mut candidatos: Vec<String> = Vec::with_capacity(4);
    if let Some(proyecto) = row.proyecto.as_deref() {
        candidatos.push(proyecto.to_string());
    }
    if !row.process_name.is_empty() {
        candidatos.push(row.process_name.clone());
    }
    if let Some(path) = row.process_path.as_deref() {
        if let Some(base) = path.rsplit('\\').next() {
            candidatos.push(base.to_string());
        }
    }
    if let Some(script) = row.process_cmd.as_deref().and_then(primer_script) {
        if let Some(base) = script.rsplit(['\\', '/']).next() {
            candidatos.push(base.to_string());
        }
    }
    candidatos.iter().any(|c| cfg.esta_oculto(c))
}

/// ¿El proceso está en la blocklist? Compara el nombre derivado, el basename del
/// path y el script de la línea de comandos (case-insensitive), para cubrir también
/// filas con nombre aún vacío y procesos intérprete cuyo script es el que importa
/// (p. ej. bun.exe sirviendo `orchestrator.js`).
fn esta_excluido(row: &PortInfo) -> bool {
    let mut candidatos: Vec<String> = Vec::with_capacity(2);
    let nombre = if row.process_name.is_empty() {
        row.process_path
            .as_deref()
            .and_then(|p| p.rsplit('\\').next())
            .unwrap_or("")
    } else {
        row.process_name.as_str()
    };
    candidatos.push(nombre.to_string());
    if let Some(script) = row.process_cmd.as_deref().and_then(primer_script) {
        if let Some(base) = script.rsplit(['\\', '/']).next() {
            candidatos.push(base.to_string());
        }
    }
    candidatos.iter().any(|candidato| {
        PROCESOS_EXCLUIDOS
            .iter()
            .any(|excluido| candidato.eq_ignore_ascii_case(excluido))
    })
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
    cmd: Option<String>,
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

    pub fn get(&mut self, pid: u32) -> (String, Option<String>, Option<String>) {
        if let Some(entry) = self.entries.get(&pid) {
            if entry.at.elapsed() < NAME_CACHE_TTL {
                return (entry.name.clone(), entry.path.clone(), entry.cmd.clone());
            }
        }
        // Una sola apertura del proceso resuelve ruta y nombre derivado.
        let path = process::resolve_process_path(pid);
        let cmd = process::resolve_process_cmdline(pid);
        let name = path
            .as_deref()
            .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
            .unwrap_or_else(|| "desconocido".to_string());
        let entry = ResolvedProcess {
            name: name.clone(),
            path: path.clone(),
            cmd: cmd.clone(),
            at: Instant::now(),
        };
        self.entries.insert(pid, entry);
        self.evict_if_needed();
        (name, path, cmd)
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

/// `MIB_TCPROW.dwLocalAddr` viene como valor `inet_addr` (primer octeto en el
/// byte bajo), así que los octetos en memoria ya están en orden de puntos
/// (`7F 00 00 01` para 127.0.0.1). Formatear los bytes nativos reproduce
/// exactamente ese orden sin importar la endianness de la máquina.
fn ipv4_to_string(raw: u32) -> String {
    let [a, b, c, d] = raw.to_ne_bytes();
    format!("{a}.{b}.{c}.{d}")
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
        assert_eq!(ipv4_to_string(0x0100_007F), "127.0.0.1");
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
                process_cmd: None,
                proyecto: None,
            },
            PortInfo {
                port: 80,
                pid: 4,
                address: "0.0.0.0".into(),
                process_name: String::new(),
                process_path: None,
                process_cmd: None,
                proyecto: None,
            },
            PortInfo {
                port: 3000,
                pid: 100,
                address: "[::]".into(),
                process_name: String::new(),
                process_path: None,
                process_cmd: None,
                proyecto: None,
            },
            PortInfo {
                port: 3000,
                pid: 101,
                address: "127.0.0.1".into(),
                process_name: String::new(),
                process_path: None,
                process_cmd: None,
                proyecto: None,
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
        let (a, path, _cmd) = cache.get(u32::MAX); // PID improbable: sin ruta ni nombre
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
                process_cmd: None,
                proyecto: None,
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
            row(
                7679,
                Some(r"C:\Program Files\Google\Drive File Stream\GoogleDriveFS.exe"),
            ), // sincronizador excluido por blocklist
            row(
                49351,
                Some(r"C:\Program Files\Intel\SUR\QUEENCREEK\x64\esrv.exe"),
            ), // Intel: no debe ofrecerse
            row(
                59489,
                Some(
                    r"C:\Users\Owner\AppData\Local\Programs\@codebufffreebuff-desktop\Freebuff.exe",
                ),
            ), // la propia app Freebuff
        ];

        let kept = solo_aplicaciones(rows, &crate::config::Config::default());
        let kept_ports: Vec<u16> = kept.iter().map(|r| r.port).collect();
        assert_eq!(kept_ports, vec![3000, 5432]);

        // La blocklist también aplica cuando el nombre derivado está relleno.
        let mut fila_google = row(
            7679,
            Some(r"C:\Program Files\Google\Drive File Stream\GoogleDriveFS.exe"),
        );
        fila_google.process_name = "GoogleDriveFS.exe".into();
        assert!(!es_puerto_de_aplicacion(
            &fila_google,
            &crate::config::Config::default()
        ));

        // El orchestrator de Freebuff corre bajo bun.exe: la blocklist debe
        // casar con el script del cmdline, no con el ejecutable.
        let mut orquestador = row(
            59494,
            Some(
                r"C:\Users\Owner\AppData\Local\Programs\@codebufffreebuff-desktop\resources\bun\bun.exe",
            ),
        );
        orquestador.process_name = "bun.exe".into();
        orquestador.process_cmd = Some(
            r"C:\Users\Owner\AppData\Local\Programs\@codebufffreebuff-desktop\resources\bun\bun.exe C:\Users\Owner\AppData\Local\Programs\@codebufffreebuff-desktop\resources\orchestrator\orchestrator.js"
                .to_string(),
        );
        assert!(!es_puerto_de_aplicacion(
            &orquestador,
            &crate::config::Config::default()
        ));

        // Ocultar por proyecto desde la config (p. ej. gloryapi) sin tocar el binario.
        let mut cfg_oculta = crate::config::Config::default();
        cfg_oculta.ocultar.push("gloryapi".into());
        let mut fila_gloryapi = row(3101, Some(r"C:\Program Files\nodejs\node.exe"));
        fila_gloryapi.proyecto = Some("gloryapi".into());
        assert!(es_puerto_de_aplicacion(
            &fila_gloryapi,
            &crate::config::Config::default()
        ));
        assert!(!es_puerto_de_aplicacion(&fila_gloryapi, &cfg_oculta));
    }

    #[test]
    fn etiqueta_muestra_script_de_node_y_bun() {
        fn row(name: &str, cmd: Option<&str>) -> PortInfo {
            PortInfo {
                port: 3101,
                pid: 1000,
                address: "127.0.0.1".into(),
                process_name: name.into(),
                process_path: Some(r"C:\Program Files\nodejs\node.exe".into()),
                process_cmd: cmd.map(str::to_string),
                proyecto: None,
            }
        }

        let bridge = row(
            "node.exe",
            Some(
                r#""C:\Program Files\nodejs\node.exe" C:\Users\Owner\OneDrive\Documentos\area-trabajo\gloryapi\integrations\codex-bridge\bridge\server.js"#,
            ),
        );
        assert_eq!(
            etiqueta_visible(&bridge),
            "…\\codex-bridge\\bridge\\server.js"
        );

        let bun = row(
            "bun.exe",
            Some(
                r#"C:\Users\Owner\AppData\Local\Programs\@codebufffreebuff-desktop\resources\bun\bun.exe C:\Users\Owner\AppData\Local\Programs\@codebufffreebuff-desktop\resources\orchestrator\orchestrator.js"#,
            ),
        );
        assert_eq!(
            etiqueta_visible(&bun),
            "…\\resources\\orchestrator\\orchestrator.js"
        );

        // Ruta con `\.bin\..` se normaliza y el flag se salta.
        let vite = row(
            "node.exe",
            Some(
                r#""node" "C:\Users\Owner\OneDrive\Documentos\area-trabajo\gloryapi\node_modules\.bin\..\vite\bin\vite.js""#,
            ),
        );
        let label = etiqueta_visible(&vite);
        assert_eq!(label, "…\\vite\\bin\\vite.js");
        assert!(!label.contains(".bin"));

        // Sin línea de comandos: cae al nombre del ejecutable.
        let sin_cmd = row("node.exe", None);
        assert_eq!(etiqueta_visible(&sin_cmd), "node.exe");
    }

    #[test]
    fn proyecto_se_deriva_del_cmdline() {
        let cfg = crate::config::Config::default();
        fn row(cmd: Option<&str>, path: Option<&str>) -> PortInfo {
            PortInfo {
                port: 3101,
                pid: 1000,
                address: "127.0.0.1".into(),
                process_name: "node.exe".into(),
                process_path: path.map(str::to_string),
                process_cmd: cmd.map(str::to_string),
                proyecto: None,
            }
        }

        // cmdline con ruta del script dentro del workspace.
        let gloryapi = row(
            Some(
                r#""C:\Program Files\nodejs\node.exe" C:\Users\Owner\OneDrive\Documentos\area-trabajo\gloryapi\server\dist\index.js"#,
            ),
            Some(r"C:\Program Files\nodejs\node.exe"),
        );
        assert_eq!(proyecto_para(&gloryapi, &cfg).as_deref(), Some("gloryapi"));

        // Case-insensitive y carpetas con espacios: PROYECTO TASKS.
        let tasks = row(
            Some(
                r#""node" "C:\Users\Owner\OneDrive\Documentos\AREA-TRABAJO\PROYECTO TASKS\frontend\node_modules\.bin\..\vite\bin\vite.js""#,
            ),
            None,
        );
        assert_eq!(
            proyecto_para(&tasks, &cfg).as_deref(),
            Some("PROYECTO TASKS")
        );

        // Fuera del workspace (Freebuff, Intel…): sin proyecto.
        let externo = row(
            Some(r"C:\Users\Owner\AppData\Local\Programs\@codebufffreebuff-desktop\Freebuff.exe"),
            None,
        );
        assert_eq!(proyecto_para(&externo, &cfg), None);

        // Sin cmdline ni ruta resoluble.
        let sin_datos = row(None, None);
        assert_eq!(proyecto_para(&sin_datos, &cfg), None);
    }

    #[test]
    fn alias_de_config_y_workspace_personalizado() {
        let row = |port: u16, cmd: &str| PortInfo {
            port,
            pid: 1000,
            address: "127.0.0.1".into(),
            process_name: "node.exe".into(),
            process_path: None,
            process_cmd: Some(cmd.to_string()),
            proyecto: None,
        };

        // Alias manual por puerto: prioridad sobre la derivación.
        let cfg: crate::config::Config =
            serde_json::from_str(r#"{"nombres": {"3000": "Tasks backend"}}"#).unwrap();
        let backend = row(
            3000,
            r"C:\tmp\glory-target\glory_backend_main\debug\glory-backend.exe",
        );
        assert_eq!(
            proyecto_para(&backend, &cfg).as_deref(),
            Some("Tasks backend")
        );

        // Raíz de workspace personalizada vía config.
        let cfg2: crate::config::Config =
            serde_json::from_str(r#"{"workspace": "proyectos"}"#).unwrap();
        let web = row(4100, r"C:\dev\proyectos\webapp\server\index.js");
        assert_eq!(proyecto_para(&web, &cfg2).as_deref(), Some("webapp"));
        // Con la raíz por defecto no se reconoce esa ruta.
        assert_eq!(proyecto_para(&web, &crate::config::Config::default()), None);
    }

    #[test]
    fn etiqueta_popup_prefiere_proyecto() {
        let mut row = PortInfo {
            port: 4100,
            pid: 1000,
            address: "127.0.0.1".into(),
            process_name: "node.exe".into(),
            process_path: Some(r"C:\Program Files\nodejs\node.exe".into()),
            process_cmd: Some(
                r#""C:\Program Files\nodejs\node.exe" C:\Users\Owner\OneDrive\Documentos\area-trabajo\gloryapi\integrations\codex-bridge\bridge\server.js"#
                    .to_string(),
            ),
            proyecto: None,
        };
        row.proyecto = Some("gloryapi".into());
        assert_eq!(
            etiqueta_popup(&row),
            "gloryapi · codex-bridge\\bridge\\server.js"
        );

        // Sin proyecto: igual que la etiqueta normal.
        row.proyecto = None;
        assert_eq!(etiqueta_popup(&row), etiqueta_visible(&row));
    }
}
