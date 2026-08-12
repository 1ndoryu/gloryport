//! Bandeja del sistema: icono, popup de puertos, notificaciones y ciclo de vida.
//!
//! La UI es el popup pintado con GDI (ver `popup.rs`), estilo "Wispr Flow". El bucle es
//! de un solo hilo y sin timers: el escaneo ocurre solo al abrir el popup, así la CPU
//! se mantiene ~0% cuando la app está inactiva.

use std::sync::OnceLock;
use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD,
    NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
    NOTIFY_ICON_INFOTIP_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW, FindWindowW,
    GetMessageW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    TranslateMessage, HICON, MSG, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WNDCLASSW,
    WS_OVERLAPPED,
};

use crate::popup::Action;
use crate::ports::PortInfo;
use crate::{autostart, fonts, icon, popup, ports, process};

const TRAY_ID: u32 = 1;
const WM_APP_TRAY: u32 = WM_APP + 1;
const WM_APP_SHOW_MENU: u32 = WM_APP + 2;

/// Ventana e icono vigentes para re-agregar el icono si el shell lo recrea.
static TRAY_HWND: OnceLock<usize> = OnceLock::new();
static TRAY_ICON: OnceLock<usize> = OnceLock::new();
/// Mensaje registrado que el shell envía al recrear la bandeja (p. ej. Explorer).
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

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
        let _ = TRAY_HWND.set(hwnd.0 as usize);

        let icon = icon::load_icon().map_err(|e| e.to_string())?;
        let _ = TRAY_ICON.set(icon.0 as usize);

        let nid = build_nid(hwnd, icon);
        if Shell_NotifyIconW(NIM_ADD, &nid).0 == 0 {
            return Err(format!(
                "Shell_NotifyIconW(NIM_ADD) falló (Win32 {})",
                last_win32_code()
            ));
        }

        set_version_4(hwnd);

        // TaskbarCreated: Explorer recrea la bandeja tras reiniciar o colgarse.
        let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));

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
        fonts::cleanup();
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
            // Con NOTIFYICON_VERSION_4 el shell empaqueta el id del icono en la
            // palabra alta de lParam: solo la palabra baja es el mensaje de ratón.
            let mouse_msg = lparam.0 as u32 & 0xFFFF;
            match mouse_msg {
                WM_LBUTTONUP | WM_CONTEXTMENU => toggle_or_show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        _ if TASKBAR_CREATED.get().copied() == Some(msg) => {
            re_add_icon();
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

/// Alterna el popup: si ya está abierto lo cierra sin acción; si no, lo abre.
unsafe fn toggle_or_show_menu(hwnd: HWND) {
    if popup::is_open() {
        popup::cancel_active();
    } else if popup::closed_recently(Duration::from_millis(250)) {
        // El popup acaba de cerrarse por este mismo gesto (el DOWN activó la
        // bandeja y cerró el popup; el UP llega después). Consumir el clic evita
        // que se reabra al instante.
    } else {
        show_menu(hwnd);
    }
}

/// Re-agrega el icono cuando Explorer recrea la bandeja (TaskbarCreated).
unsafe fn re_add_icon() {
    let (Some(hwnd_raw), Some(icon_raw)) = (TRAY_HWND.get().copied(), TRAY_ICON.get().copied())
    else {
        return;
    };
    let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
    let icon = HICON(icon_raw as *mut core::ffi::c_void);
    let nid = build_nid(hwnd, icon);
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    set_version_4(hwnd);
}

unsafe fn build_nid(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
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
    nid
}

/// Versión 4: clic derecho llega como WM_CONTEXTMENU (comportamiento moderno).
unsafe fn set_version_4(hwnd: HWND) {
    let mut version = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        ..Default::default()
    };
    version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    let _ = Shell_NotifyIconW(NIM_SETVERSION, &version);
}

/// Escanea los puertos y muestra el popup estilizado; ejecuta la acción elegida.
unsafe fn show_menu(hwnd: HWND) {
    let mut ports = match ports::scan_listeners() {
        Ok(p) => p,
        Err(e) => {
            notify(
                hwnd,
                "GLORYPORT",
                &format!("No se pudo escanear los puertos: {e}"),
                NIIF_ERROR,
            );
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
    // Solo aplicaciones de usuario: los servicios del sistema y los procesos sin
    // nombre resoluble no deben ofrecerse al kill desde la bandeja.
    ports = ports::solo_aplicaciones(ports);

    match popup::show(hwnd, ports, autostart::is_enabled()) {
        Action::None => {}
        Action::Kill(row) => handle_kill(hwnd, &row),
        // Re-escanea y reabre el popup al instante.
        Action::Refresh => show_menu(hwnd),
        Action::ToggleAutostart => {
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
        }
        Action::About => {
            notify(
                hwnd,
                "Acerca de GLORYPORT",
                &format!(
                    "v{} — Puertos TCP en escucha desde la bandeja.\nClic en un puerto termina su proceso.",
                    env!("CARGO_PKG_VERSION")
                ),
                NIIF_INFO,
            );
        }
        Action::Exit => {
            let _ = PostMessageW(Some(hwnd), WM_DESTROY, WPARAM(0), LPARAM(0));
        }
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
