//! Bandeja del sistema: icono, menú de puertos, notificaciones y ciclo de vida.
//!
//! El bucle es de un solo hilo y sin timers: el menú se reconstruye completo en cada
//! apertura (escaneo bajo demanda). Esto mantiene CPU ~0% cuando la app está inactiva.

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
    NOTIFY_ICON_INFOTIP_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, PostMessageW,
    PostQuitMessage, RegisterClassW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HMENU,
    MF_CHECKED, MF_DISABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, TPM_LEFTALIGN,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN, WM_APP, WM_CONTEXTMENU, WM_DESTROY,
    WM_LBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};

use crate::ports::PortInfo;
use crate::{autostart, icon, ports, process};

const TRAY_ID: u32 = 1;
const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_SHOW_MENU: u32 = WM_APP + 2;

const ID_REFRESH: usize = 100;
const ID_AUTOSTART: usize = 101;
const ID_ABOUT: usize = 102;
const ID_EXIT: usize = 103;
const ID_PORT_BASE: usize = 200;

/// Límite de filas en el menú para que siga siendo usable con muchos puertos.
const MAX_MENU_PORTS: usize = 60;

/// Arranca la app de bandeja. Devuelve `Err(mensaje)` si algo impide operar.
pub fn run() -> Result<(), String> {
    // Instancia única: si otra está viva, le pedimos mostrar el menú y salimos.
    let mutex = unsafe { CreateMutexW(None, true, w!("GLORYPORT_Tray_SingleInstance")) }
        .map_err(|e| format!("CreateMutexW falló: {e}"))?;
    let already_running = unsafe { GetLastError().0 == ERROR_ALREADY_EXISTS.0 };
    if already_running {
        if let Ok(hwnd) = unsafe { FindWindowW(w!("GloryPortTrayWnd"), PCWSTR::null()) } {
            let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_SHOW_MENU, WPARAM(0), LPARAM(0)) };
        }
        return Ok(());
    }
    // El mutex se mantiene vivo mientras corre la app (RAII lo libera al salir).
    let _guard = MutexGuard(mutex);

    let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None) }
        .map_err(|e| format!("GetModuleHandleW falló: {e}"))?
        .into();

    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: w!("GloryPortTrayWnd"),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(format!(
                "RegisterClassW falló (Win32 {})",
                last_win32_code()
            ));
        }

        let hwnd = CreateWindowExW(
            Default::default(),
            w!("GloryPortTrayWnd"),
            w!("GLORYPORT"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        )
        .map_err(|e| format!("CreateWindowExW falló: {e}"))?;

        let icon = icon::load_icon().map_err(|e| e.to_string())?;

        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: icon,
            ..Default::default()
        };
        copy_wide_into("GLORYPORT — puertos TCP en escucha", &mut nid.szTip);
        if Shell_NotifyIconW(NIM_ADD, &nid).0 == 0 {
            return Err(format!(
                "Shell_NotifyIconW(NIM_ADD) falló (Win32 {})",
                last_win32_code()
            ));
        }

        // Versión 4: clic derecho llega como WM_CONTEXTMENU (comportamiento moderno).
        let mut version = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            ..Default::default()
        };
        version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &version);

        // Bucle de mensajes: termina con WM_QUIT (Salir o cierre del sistema).
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }

        // Cleanup garantizado del orden inverso a la creación.
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        let _ = DestroyIcon(icon);
        let _ = DestroyWindow(hwnd);
        let _ = unregister_class();
    }
    Ok(())
}

/// Mantiene abierto el mutex de instancia única mientras viva (RAII).
struct MutexGuard(windows::Win32::Foundation::HANDLE);

impl Drop for MutexGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

unsafe fn unregister_class() -> windows::core::Result<()> {
    let hinstance: HINSTANCE = GetModuleHandleW(None)?.into();
    windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(
        w!("GloryPortTrayWnd"),
        Some(hinstance),
    )
}

/// Devuelve el último código Win32 (16 bits bajos).
fn last_win32_code() -> u32 {
    unsafe { GetLastError().0 }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    _wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_APP_TRAY => {
            match lparam.0 as u32 {
                WM_LBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_SHOW_MENU => {
            show_menu(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, _wparam, lparam),
    }
}

/// Reconstruye y muestra el menú con los puertos actuales, y procesa la selección.
unsafe fn show_menu(hwnd: HWND) {
    let Ok(hmenu) = CreatePopupMenu() else {
        return;
    };
    let _ = SetForegroundWindow(hwnd);

    let mut ports = match ports::scan_listeners() {
        Ok(p) => p,
        Err(e) => {
            let _ = AppendMenuW(
                hmenu,
                MF_STRING | MF_DISABLED | MF_GRAYED,
                0,
                PCWSTR(HSTRING::from(format!("GLORYPORT — error: {e}")).as_ptr()),
            );
            let _ = show_and_run(hmenu, hwnd, Vec::new());
            return;
        }
    };
    {
        static NAME_CACHE: std::sync::LazyLock<std::sync::Mutex<ports::NameCache>> =
            std::sync::LazyLock::new(|| std::sync::Mutex::new(ports::NameCache::new()));
        if let Ok(mut cache) = NAME_CACHE.lock() {
            ports::attach_process_names(&mut ports, &mut cache);
        }
    }

    let header = format!("GLORYPORT — {} puertos TCP", ports.len());
    let _ = AppendMenuW(
        hmenu,
        MF_STRING | MF_DISABLED | MF_GRAYED,
        0,
        PCWSTR(HSTRING::from(header).as_ptr()),
    );
    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

    let shown = ports.len().min(MAX_MENU_PORTS);
    for (i, row) in ports.iter().take(shown).enumerate() {
        let label = ports::menu_label(row);
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            ID_PORT_BASE + i,
            PCWSTR(HSTRING::from(label).as_ptr()),
        );
    }
    if ports.len() > shown {
        let note = format!("… y {} puertos más", ports.len() - shown);
        let _ = AppendMenuW(
            hmenu,
            MF_STRING | MF_DISABLED | MF_GRAYED,
            0,
            PCWSTR(HSTRING::from(note).as_ptr()),
        );
    }

    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(
        hmenu,
        MF_STRING,
        ID_REFRESH,
        PCWSTR(w!("Actualizar lista").as_ptr()),
    );
    let autostart_flag = if autostart::is_enabled() {
        MF_CHECKED
    } else {
        MF_UNCHECKED
    };
    let _ = AppendMenuW(
        hmenu,
        MF_STRING | autostart_flag,
        ID_AUTOSTART,
        PCWSTR(w!("Iniciar con Windows").as_ptr()),
    );
    let _ = AppendMenuW(
        hmenu,
        MF_STRING,
        ID_ABOUT,
        PCWSTR(w!("Acerca de GLORYPORT").as_ptr()),
    );
    let _ = AppendMenuW(hmenu, MF_STRING, ID_EXIT, PCWSTR(w!("Salir").as_ptr()));

    let _ = show_and_run(hmenu, hwnd, ports);
}

/// Muestra el menú de forma modal, destruye el handle y ejecuta la acción elegida.
unsafe fn show_and_run(hmenu: HMENU, hwnd: HWND, ports: Vec<PortInfo>) -> Result<(), String> {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let cmd = TrackPopupMenu(
        hmenu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN,
        pt.x,
        pt.y,
        Some(0),
        hwnd,
        None,
    )
    .0;
    let _ = DestroyMenu(hmenu);
    if cmd <= 0 {
        return Ok(());
    }
    let cmd = cmd as usize;

    if cmd >= ID_PORT_BASE {
        let idx = cmd - ID_PORT_BASE;
        if let Some(row) = ports.get(idx) {
            handle_kill(hwnd, row);
        }
        return Ok(());
    }

    match cmd {
        // Re-escanea y reabre el menú al instante; el menú ya se reconstruye en
        // cada apertura, pero este item permite refrescar sin cerrar y volver a abrir.
        ID_REFRESH => {
            show_menu(hwnd);
            Ok(())
        }
        ID_AUTOSTART => {
            let on = !autostart::is_enabled();
            match autostart::set_enabled(on) {
                Ok(()) => notify(
                    hwnd,
                    "GLORYPORT",
                    if on {
                        "Auto-inicio activado con Windows."
                    } else {
                        "Auto-inicio desactivado."
                    },
                    NIIF_INFO,
                ),
                Err(e) => notify(
                    hwnd,
                    "GLORYPORT",
                    &format!("No se pudo cambiar el auto-inicio: {e}"),
                    NIIF_ERROR,
                ),
            }
            Ok(())
        }
        ID_ABOUT => {
            notify(
                hwnd,
                "Acerca de GLORYPORT",
                &format!(
                    "v{} — Puertos TCP en escucha desde la bandeja.\nClic en un puerto termina su proceso.",
                    env!("CARGO_PKG_VERSION")
                ),
                NIIF_INFO,
            );
            Ok(())
        }
        ID_EXIT => {
            let _ = PostMessageW(Some(hwnd), WM_DESTROY, WPARAM(0), LPARAM(0));
            Ok(())
        }
        _ => Ok(()),
    }
}

fn handle_kill(hwnd: HWND, row: &PortInfo) {
    match process::kill_pid(row.pid) {
        Ok(()) => notify(
            hwnd,
            "Puerto liberado",
            &format!(
                "El puerto {} quedó libre: se terminó {} (PID {}).",
                row.port, row.process_name, row.pid
            ),
            NIIF_INFO,
        ),
        Err(e) => notify(
            hwnd,
            "No se pudo liberar el puerto",
            &format!("Puerto {}: {}.", row.port, e),
            NIIF_ERROR,
        ),
    }
}

/// Notificación de bandeja (balloon/toast), no bloqueante.
fn notify(hwnd: HWND, title: &str, body: &str, flags: NOTIFY_ICON_INFOTIP_FLAGS) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_INFO,
        dwInfoFlags: flags,
        ..Default::default()
    };
    copy_wide_into(title, &mut nid.szInfoTitle);
    copy_wide_into(body, &mut nid.szInfo);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Copia texto como UTF-16 truncado y terminado en NUL dentro de un buffer fijo.
fn copy_wide_into(text: &str, buf: &mut [u16]) {
    let mut chars = text.encode_utf16();
    for slot in buf.iter_mut() {
        *slot = chars.next().unwrap_or(0);
    }
}
