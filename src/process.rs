//! Resolución de nombres y terminación de procesos (Win32 directo).
//!
//! Permisos mínimos en cada apertura: `PROCESS_QUERY_LIMITED_INFORMATION` para leer el
//! nombre y `PROCESS_TERMINATE` para terminar. Nunca `PROCESS_ALL_ACCESS`.

use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

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
}
