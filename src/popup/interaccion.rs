//! Interacción del popup: `wnd_proc` y gestión de mensajes (ratón, teclado, tooltip de filas y
//! cierre). No pinta nada: delega el dibujo en [`super::pintado`].

use super::{
    hit_test, lock_state, scroll_step, Action, Hit, Layout, CURSOR_ARROW, WM_APP_POPUP_DONE,
};

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Controls::{
    NMHDR, NMTTDISPINFOW, TTM_TRACKACTIVATE, TTM_TRACKPOSITION, TTM_UPDATETIPTEXTW, TTN_NEEDTEXTW,
    TTTOOLINFOW, WM_MOUSELEAVE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, FindWindowW, GetCursorPos, PostMessageW, SendMessageW, SetCursor, HCURSOR,
    WA_INACTIVE, WM_ACTIVATE, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_NOTIFY, WM_PAINT, WM_SETCURSOR,
};

pub(super) unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            super::pintado::paint(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1), // sin borrado: pintamos todo el fondo
        WM_SETCURSOR => {
            // Fija siempre la flecha: la clase no hereda el cursor del hilo (causa
            // del cursor de espera al pasar por encima del popup).
            if let Some(cursor) = CURSOR_ARROW.get() {
                let _ = SetCursor(Some(HCURSOR(*cursor as *mut core::ffi::c_void)));
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            on_mousemove(hwnd, lparam);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            set_hover(hwnd, None);
            untrack();
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            on_wheel(wparam);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            on_lbuttonup(hwnd, lparam);
            LRESULT(0)
        }
        WM_NOTIFY => {
            if on_notify(lparam) {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_KEYDOWN => {
            on_key(hwnd, wparam);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if wparam.0 as u32 & 0xFFFF == WA_INACTIVE {
                finish(hwnd, Action::None);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn on_mousemove(hwnd: HWND, lparam: LPARAM) {
    let (x, y) = lparam_xy(lparam);
    arm_track_leave(hwnd);
    let hit = current_hit(x, y);
    set_hover(hwnd, Some(hit));
}

fn on_wheel(wparam: WPARAM) {
    let delta = ((wparam.0 >> 16) as u16) as i16 as i32;
    let mut guard = lock_state();
    if let Some(s) = guard.as_mut() {
        let layout = Layout::new(s.ports.len());
        let next = scroll_step(s.scroll, delta, layout.max_scroll);
        if next != s.scroll {
            s.scroll = next;
            s.hover = None;
            if let Ok(hwnd) = popup_hwnd() {
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            }
        }
    }
}

fn on_lbuttonup(hwnd: HWND, lparam: LPARAM) {
    let (x, y) = lparam_xy(lparam);
    let hit = current_hit(x, y);
    let action = action_for(hit);
    finish(hwnd, action);
}

/// Sirve el texto del tooltip de filas (`TTN_NEEDTEXTW`): copia la ruta completa
/// al buffer fijo `szText` del propio mensaje, sin punteros a memoria propia.
fn on_notify(lparam: LPARAM) -> bool {
    unsafe {
        let nm = &*(lparam.0 as *const NMHDR);
        let Ok(hwnd) = popup_hwnd() else {
            return false;
        };
        if nm.idFrom != hwnd.0 as usize {
            return false;
        }
        if nm.code != TTN_NEEDTEXTW {
            return false;
        }
        let info = &mut *(lparam.0 as *mut NMTTDISPINFOW);
        let guard = lock_state();
        let Some(state) = guard.as_ref() else {
            return false;
        };
        let text = state
            .hover
            .and_then(|hit| match hit {
                Hit::Row(idx) => state.ports.get(idx),
                _ => None,
            })
            .map(crate::ports::etiqueta_visible)
            .unwrap_or_default();
        let mut wide = text.encode_utf16();
        for slot in info.szText.iter_mut() {
            *slot = wide.next().unwrap_or(0);
        }
        info.lpszText = PWSTR(info.szText.as_mut_ptr());
        true
    }
}

fn on_key(hwnd: HWND, wparam: WPARAM) {
    let vk = (wparam.0 as u32) as u16;
    if vk == VK_ESCAPE.0 {
        finish(hwnd, Action::None);
    } else if vk == VK_RETURN.0 {
        let hit = lock_state().as_ref().and_then(|s| s.hover);
        finish(hwnd, action_for(hit.unwrap_or(Hit::None)));
    }
}

fn action_for(hit: Hit) -> Action {
    match hit {
        Hit::None => Action::None,
        Hit::Row(idx) => lock_state()
            .as_ref()
            .and_then(|s| s.ports.get(idx).cloned())
            .map(Action::Kill)
            .unwrap_or(Action::None),
        Hit::Refresh => Action::Refresh,
        Hit::ToggleAutostart => Action::ToggleAutostart,
    }
}

/// Registra la acción, marca `done` (evita doble cierre) y despierta el bucle modal.
pub(super) fn finish(hwnd: HWND, action: Action) {
    {
        let mut guard = lock_state();
        if let Some(s) = guard.as_mut() {
            if s.done {
                return;
            }
            s.done = true;
            s.action = Some(action);
        }
    }
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_APP_POPUP_DONE, WPARAM(0), LPARAM(0));
    }
}

fn arm_track_leave(hwnd: HWND) {
    let mut guard = lock_state();
    if let Some(s) = guard.as_mut() {
        if s.tracking {
            return;
        }
        s.tracking = true;
        let mut tme = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        let _ = unsafe { TrackMouseEvent(&mut tme) };
    }
}

fn untrack() {
    let mut guard = lock_state();
    if let Some(s) = guard.as_mut() {
        s.tracking = false;
    }
}

fn set_hover(hwnd: HWND, hit: Option<Hit>) {
    let mut guard = lock_state();
    let Some(s) = guard.as_mut() else {
        return;
    };
    if s.hover == hit {
        return;
    }
    s.hover = hit;
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
    // Tooltip "track": se posiciona junto al cursor (coordenadas de pantalla) y
    // se activa/desactiva de forma explícita, sin depender de que el control
    // subclasee al popup. El texto se actualiza aquí y también vía TTN_NEEDTEXTW.
    if let (Some(tooltip), Some(tool_ti)) = (s.tooltip, s.tool_ti.as_ref()) {
        let tooltip = HWND(tooltip as *mut core::ffi::c_void);
        let ti_ptr = &tool_ti.0 as *const TTTOOLINFOW as usize;
        match hit {
            Some(Hit::Row(idx)) if idx < s.ports.len() => {
                let label = crate::ports::etiqueta_visible(&s.ports[idx]);
                let mut wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
                let _ = unsafe {
                    SendMessageW(
                        tooltip,
                        TTM_UPDATETIPTEXTW,
                        Some(WPARAM(0)),
                        Some(LPARAM(wide.as_mut_ptr() as isize)),
                    )
                };
                let mut pt = POINT::default();
                let _ = unsafe { GetCursorPos(&mut pt) };
                pt.x += 16;
                pt.y += 24;
                let _ = unsafe {
                    SendMessageW(
                        tooltip,
                        TTM_TRACKPOSITION,
                        Some(WPARAM(0)),
                        Some(LPARAM(
                            (((pt.y as u32) << 16) | ((pt.x as u32) & 0xFFFF)) as isize,
                        )),
                    )
                };
                let _ = unsafe {
                    SendMessageW(
                        tooltip,
                        TTM_TRACKACTIVATE,
                        Some(WPARAM(1)),
                        Some(LPARAM(ti_ptr as isize)),
                    )
                };
            }
            _ => {
                let _ = unsafe {
                    SendMessageW(
                        tooltip,
                        TTM_TRACKACTIVATE,
                        Some(WPARAM(0)),
                        Some(LPARAM(ti_ptr as isize)),
                    )
                };
            }
        }
    }
}

fn current_hit(x: i32, y: i32) -> Hit {
    let guard = lock_state();
    guard.as_ref().map_or(Hit::None, |s| {
        let layout = Layout::new(s.ports.len());
        hit_test((x, y), &layout, s.scroll, s.ports.len())
    })
}

fn popup_hwnd() -> windows::core::Result<HWND> {
    unsafe { FindWindowW(w!("GloryPortPopupWnd"), PCWSTR::null()) }
}

fn lparam_xy(l: LPARAM) -> (i32, i32) {
    let v = l.0 as u32;
    (((v & 0xFFFF) as u16) as i32, ((v >> 16) as u16) as i32)
}
