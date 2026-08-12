//! Resolución de nombres y terminación de procesos (Win32 directo).
//!
//! Permisos mínimos en cada apertura: `PROCESS_QUERY_LIMITED_INFORMATION` para leer el
//! nombre y `PROCESS_TERMINATE` para terminar. Nunca `PROCESS_ALL_ACCESS`.

use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, PROCESS_VM_READ,
};

/// Tope defensivo de la línea de comandos leída (Windows permite ~32 767 chars).
const CMDLINE_MAX_BYTES: usize = 16_384;

/// `PROCESSINFOCLASS::ProcessBasicInformation` (Winternl): solo se usa el PEB.
const PROCESS_BASIC_INFORMATION_CLASS: i32 = 0;

/// Vista mínima de `PROCESS_BASIC_INFORMATION` (x64): basta el PEB.
#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb_base_address: usize,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

// FFI directo a ntdll (la crate `windows` lo expone solo en el módulo Wdk).
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process_handle: windows::Win32::Foundation::HANDLE,
        process_information_class: i32,
        process_information: *mut core::ffi::c_void,
        process_information_length: u32,
        return_length: *mut u32,
    ) -> i32; // NTSTATUS
}

/// PIDs que nunca se terminan: System Idle (0), System (4) y el propio GLORYPORT.
pub fn is_protected_pid(pid: u32) -> bool {
    pid == 0 || pid == 4 || pid == unsafe { GetCurrentProcessId() }
}

/// Ruta completa del ejecutable de un PID, o `None` si no es accesible.
///
/// Devuelve el path real (p. ej. `C:\Program Files\nodejs\node.exe`), que permite
/// distinguir aplicaciones de usuario de procesos del sistema sin depender del nombre.
pub fn resolve_process_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .is_ok();
    let _ = unsafe { CloseHandle(handle) };
    if !ok {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// Línea de comandos completa de un proceso, o `None` si no es accesible.
///
/// Lee el PEB del proceso (x64) vía `NtQueryInformationProcess` +
/// `ReadProcessMemory`, sin WMI, sin procesos hijos y sin elevación. Los procesos
/// del sistema o de otro usuario devuelven `None` (el filtro de aplicaciones ya
/// los oculta del popup). GLORYPORT declara Windows 10/11 de 64 bits.
pub fn resolve_process_cmdline(pid: u32) -> Option<String> {
    if pid == 0 || size_of::<usize>() != 8 {
        return None;
    }
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }.ok()?;
    let result = unsafe { read_cmdline(handle) };
    let _ = unsafe { CloseHandle(handle) };
    result
}

/// Lee `RTL_USER_PROCESS_PARAMETERS.CommandLine` desde el PEB (offsets x64).
///
/// Offsets x64: PEB → ProcessParameters en `+0x20`; dentro de los parámetros,
/// `CommandLine` es un `UNICODE_STRING` en `+0x70` (Length u16, MaxLength u16,
/// padding 4, Buffer* en `+0x78`). Un build de 32 bits no entra (ver llamador).
unsafe fn read_cmdline(handle: windows::Win32::Foundation::HANDLE) -> Option<String> {
    let mut pbi = ProcessBasicInformation {
        exit_status: 0,
        peb_base_address: 0,
        affinity_mask: 0,
        base_priority: 0,
        unique_process_id: 0,
        inherited_from_unique_process_id: 0,
    };
    let mut len: u32 = 0;
    let status = NtQueryInformationProcess(
        handle,
        PROCESS_BASIC_INFORMATION_CLASS,
        (&mut pbi as *mut ProcessBasicInformation).cast(),
        size_of::<ProcessBasicInformation>() as u32,
        &mut len,
    );
    if status < 0 {
        return None;
    }

    let peb = pbi.peb_base_address;
    let mut params: usize = 0;
    if !read_bytes(
        handle,
        peb + 0x20,
        as_bytes_mut(std::slice::from_mut(&mut params)),
    ) {
        return None;
    }

    // Cabecera UNICODE_STRING de 16 bytes: Length(2) + MaxLength(2) + pad(4) + Buffer*(8).
    let mut header = [0u8; 16];
    if !read_bytes(handle, params + 0x70, &mut header) {
        return None;
    }
    let cmd_len = u16::from_ne_bytes([header[0], header[1]]) as usize;
    let buf_ptr = usize::from_ne_bytes(header[8..16].try_into().ok()?);
    if cmd_len == 0 {
        return Some(String::new());
    }
    let read_len = cmd_len.min(CMDLINE_MAX_BYTES);
    let mut buf = vec![0u16; read_len / 2];
    if !read_bytes(handle, buf_ptr, as_bytes_mut(&mut buf[..])) {
        return None;
    }
    Some(String::from_utf16_lossy(&buf))
}

/// `ReadProcessMemory` con resultado booleano; el destino debe ser un slice de bytes.
unsafe fn read_bytes(
    handle: windows::Win32::Foundation::HANDLE,
    src: usize,
    dst: &mut [u8],
) -> bool {
    if dst.is_empty() {
        return true;
    }
    ReadProcessMemory(
        handle,
        src as *const core::ffi::c_void,
        dst.as_mut_ptr().cast(),
        dst.len(),
        None,
    )
    .is_ok()
}

/// Reinterpreta un slice como bytes (para `ReadProcessMemory`).
fn as_bytes_mut<T>(slice: &mut [T]) -> &mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(slice.as_mut_ptr().cast::<u8>(), size_of_val(slice)) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillError {
    /// PID protegido (0, 4 o el propio GLORYPORT).
    Protected,
    /// Permisos insuficientes (típicamente proceso elevado o SYSTEM).
    AccessDenied,
    /// El proceso ya no existe.
    NotFound,
    /// Cualquier otro error Win32.
    Api { code: u32 },
}

impl std::fmt::Display for KillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KillError::Protected => write!(f, "PID protegido (no se puede terminar)"),
            KillError::AccessDenied => write!(
                f,
                "acceso denegado: el proceso requiere más privilegios que GLORYPORT"
            ),
            KillError::NotFound => write!(f, "el proceso ya no existe"),
            KillError::Api { code } => write!(f, "error Win32 {code}"),
        }
    }
}

/// Termina el proceso de forma inmediata (equivalente a force-kill del original).
pub fn kill_pid(pid: u32) -> Result<(), KillError> {
    if is_protected_pid(pid) {
        return Err(KillError::Protected);
    }
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }
        .map_err(|e| map_open_error(pid, &e))?;
    let res = unsafe { TerminateProcess(handle, 1) };
    let _ = unsafe { CloseHandle(handle) };
    res.map_err(|e| KillError::Api {
        code: win32_code(&e),
    })
}

fn map_open_error(pid: u32, e: &windows::core::Error) -> KillError {
    let code = win32_code(e);
    if code == ERROR_ACCESS_DENIED.0 {
        KillError::AccessDenied
    } else if code == ERROR_INVALID_PARAMETER.0 {
        // OpenProcess devuelve ERROR_INVALID_PARAMETER para PIDs inexistentes.
        let _ = pid;
        KillError::NotFound
    } else {
        KillError::Api { code }
    }
}

/// Extrae el código Win32 (16 bits bajos) de un error del binding `windows`.
pub fn win32_code(e: &windows::core::Error) -> u32 {
    (e.code().0 & 0xFFFF) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_pids_are_rejected() {
        assert!(is_protected_pid(0));
        assert!(is_protected_pid(4));
        assert!(is_protected_pid(unsafe { GetCurrentProcessId() }));
        assert!(!is_protected_pid(12345));
    }

    #[test]
    fn resolve_own_process_path() {
        let path = resolve_process_path(unsafe { GetCurrentProcessId() });
        assert!(
            path.as_deref().is_some_and(|p| p.ends_with(".exe")),
            "el runner de tests debería resolver su ruta completa"
        );
    }

    #[test]
    fn resolve_own_process_cmdline() {
        let cmd = resolve_process_cmdline(unsafe { GetCurrentProcessId() });
        assert!(
            cmd.as_deref().is_some_and(|c| !c.is_empty()),
            "el runner de tests debería resolver su propia línea de comandos"
        );
    }
}
