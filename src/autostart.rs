//! Auto-inicio del usuario (HKCU) vía la clave `Run`, sin invocar `reg.exe`.
//!
//! Solo escribe en HKCU: no requiere elevación y el usuario puede revertirlo desde
//! el Administrador de tareas o el menú de la bandeja.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("GLORYPORT");

/// Ruta completa del ejecutable actual (GLORYPORT tray/CLI).
pub fn exe_path() -> String {
    let mut buf = [0u16; 32768];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) };
    String::from_utf16_lossy(&buf[..n as usize])
}

/// Valor de la clave `Run` tal como lo escribimos (ruta entre comillas).
fn run_value() -> String {
    format!("\"{}\"", exe_path())
}

/// ¿Está GLORYPORT en el auto-inicio y apuntando al ejecutable actual?
pub fn is_enabled() -> bool {
    unsafe {
        let Ok(key) = open_key(KEY_READ) else {
            return false;
        };
        let mut value = [0u16; 4096];
        let mut size = (value.len() * 2) as u32;
        let mut kind = REG_VALUE_TYPE(0);
        let status = RegQueryValueExW(
            key,
            VALUE_NAME,
            None,
            Some(&mut kind),
            Some(value.as_mut_ptr() as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);
        if status.0 != 0 {
            return false;
        }
        let stored = String::from_utf16_lossy(&value[..(size as usize) / 2]);
        let stored = stored.trim_end_matches('\0').trim();
        stored == run_value()
    }
}

/// Activa o desactiva el auto-inicio. Devuelve `Err(mensaje)` con el código Win32.
pub fn set_enabled(on: bool) -> Result<(), String> {
    unsafe {
        let key = open_key(KEY_SET_VALUE)
            .map_err(|code| format!("no se pudo abrir Run (Win32 {code})"))?;
        let status = if on {
            let wide: Vec<u16> = run_value()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
            RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(bytes))
        } else {
            RegDeleteValueW(key, VALUE_NAME)
        };
        let _ = RegCloseKey(key);
        if status.0 != 0 && (on || status.0 != ERROR_FILE_NOT_FOUND.0) {
            return Err(format!("operación sobre Run falló (Win32 {})", status.0));
        }
    }
    Ok(())
}

unsafe fn open_key(access: REG_SAM_FLAGS) -> Result<HKEY, u32> {
    let mut key = HKEY::default();
    let status = RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key);
    if status.0 != 0 {
        Err(status.0)
    } else {
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_path_is_absolute_and_ends_with_exe() {
        let p = exe_path();
        assert!(!p.is_empty());
        assert!(p.ends_with(".exe") || p.ends_with(".EXE"));
    }
}
