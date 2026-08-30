//! Popup de bandeja estilo "Wispr Flow": ventana Win32 minimalista pintada con GDI.
//!
//! Reemplaza el menú nativo (`TrackPopupMenu`) con la paleta crema/tinta/lavanda y las
//! fuentes Figtree + EB Garamond del estilo de referencia. La ventana es de un solo
//! uso: se abre, se cierra con una acción (o Esc / clic fuera) y se destruye.
//! El bucle modal no usa `PostQuitMessage`, de modo que el bucle de la bandeja sigue
//! vivo al cerrar el popup.

use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreatePen, CreateRoundRectRgn,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetMonitorInfoW,
    GetTextExtentPoint32W, InvalidateRect, MonitorFromPoint, RoundRect, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, DRAW_TEXT_FORMAT, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, HBRUSH, HDC, HFONT, HPEN, MONITORINFO, MONITOR_DEFAULTTONEAREST, PS_SOLID, SRCCOPY,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    NMHDR, NMTTDISPINFOW, TOOLTIPS_CLASSW, TTDT_AUTOPOP, TTDT_INITIAL, TTDT_RESHOW, TTF_ABSOLUTE,
    TTF_IDISHWND, TTF_TRACK, TTM_ADDTOOLW, TTM_SETDELAYTIME, TTM_SETMAXTIPWIDTH, TTM_SETTIPBKCOLOR,
    TTM_SETTIPTEXTCOLOR, TTM_TRACKACTIVATE, TTM_TRACKPOSITION, TTM_UPDATETIPTEXTW, TTN_NEEDTEXTW,
    TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW, WM_MOUSELEAVE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetClientRect,
    GetCursorPos, GetMessageW, LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassW,
    SendMessageW, SetCursor, SetForegroundWindow, ShowWindow, TranslateMessage, CW_USEDEFAULT,
    HCURSOR, IDC_ARROW, MSG, SW_SHOWNOACTIVATE, WA_INACTIVE, WINDOW_STYLE, WM_ACTIVATE, WM_APP,
    WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NOTIFY, WM_PAINT,
    WM_SETCURSOR, WM_SETFONT, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::fonts;
use crate::ports::PortInfo;

/// Mensaje privado que cierra el bucle modal del popup (sin matar el loop de la bandeja).
const WM_APP_POPUP_DONE: u32 = WM_APP + 3;

// ── Tokens de estilo "Wispr Flow" (COLORREF = 0x00BBGGRR) ─────────────────────
const CREAM: COLORREF = COLORREF(0x00EB_FFFF); // Lumen Cream: fondo
const INK: COLORREF = COLORREF(0x001A_1A1A); // Vast Ink: texto y bordes
const LAVENDER: COLORREF = COLORREF(0x00FF_D7F0); // Lavender Whisper: primario
const FOREST: COLORREF = COLORREF(0x0046_4F03); // Forest Ink: badges/acento
const STONE: COLORREF = COLORREF(0x00D0_E4E4); // Lumen Stone: divisores
const FOG: COLORREF = COLORREF(0x0080_8A8A); // Fog: texto secundario

// ── Layout (píxeles): aire generoso, sin cabecera ni insignia ─────────────────
const WIDTH: i32 = 520;
const BORDER: i32 = 2;
const PAD_X: i32 = 14;
const PAD_TOP: i32 = 12;
const ROW_H: i32 = 38;
const FOOTER_GAP: i32 = 10;
const FOOTER_ITEM_H: i32 = 28;
const PAD_BOTTOM: i32 = 12;
const CORNER_RADIUS: i32 = 14;
const SCROLL_W: i32 = 4;
const MAX_VISIBLE_ROWS: usize = 9;
/// Tope de filas de puertos (mismo límite que el menú nativo de v1).
const MAX_TOTAL_ROWS: usize = 60;
const WHEEL_STEP: usize = 3;
/// El tooltip no envuelve: la ruta completa se muestra en una sola línea.
const TOOLTIP_MAX_WIDTH: i32 = 0;
/// Retardo inicial antes de mostrar el tooltip (ms): evita destellos al pasar.
const TOOLTIP_INITIAL_MS: u32 = 350;
/// El tooltip permanece visible hasta 12 s o hasta que el ratón se vaya.
const TOOLTIP_AUTOPOP_MS: u32 = 12_000;
/// Reaparición rápida al moverse de una fila a otra.
const TOOLTIP_RESHOW_MS: u32 = 100;

/// Acción elegida por el usuario en el popup.
#[derive(Debug)]
pub enum Action {
    None,
    Kill(PortInfo),
    Refresh,
    ToggleAutostart,
}

/// Región interactiva bajo el cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    None,
    Row(usize),
    Refresh,
    ToggleAutostart,
}

/// Geometría calculada del popup; también se prueba de forma aislada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    width: i32,
    height: i32,
    rows_visible: usize,
    rows_top: i32,
    rows_bottom: i32,
    footer_top: i32,
    max_scroll: usize,
    has_scroll: bool,
}

impl Layout {
    fn new(rows_total: usize) -> Self {
        let rows_visible = rows_total.clamp(1, MAX_VISIBLE_ROWS);
        let max_scroll = rows_total.saturating_sub(rows_visible);
        let rows_top = BORDER + PAD_TOP;
        let rows_bottom = rows_top + rows_visible as i32 * ROW_H;
        let footer_top = rows_bottom + FOOTER_GAP;
        let height = footer_top + 2 * FOOTER_ITEM_H + PAD_BOTTOM + BORDER;
        Self {
            width: WIDTH,
            height,
            rows_visible,
            rows_top,
            rows_bottom,
            footer_top,
            max_scroll,
            has_scroll: max_scroll > 0,
        }
    }

    fn content_right(&self) -> i32 {
        if self.has_scroll {
            self.width - PAD_X - SCROLL_W - 4
        } else {
            self.width - PAD_X
        }
    }

    fn row_rect(&self, visible_idx: usize) -> RECT {
        let top = self.rows_top + visible_idx as i32 * ROW_H;
        RECT {
            left: 0,
            top,
            right: self.width,
            bottom: top + ROW_H,
        }
    }
}

/// Hit-test por coordenadas de cliente; fila devuelta ya incluye el desplazamiento.
fn hit_test(pt: (i32, i32), layout: &Layout, scroll: usize, rows_total: usize) -> Hit {
    let (x, y) = pt;
    if x < BORDER || x >= layout.width - BORDER {
        return Hit::None;
    }
    if y >= layout.rows_top && y < layout.rows_bottom {
        if x >= layout.content_right() {
            return Hit::None;
        }
        let local = ((y - layout.rows_top) / ROW_H) as usize;
        let idx = scroll + local;
        return if idx < rows_total {
            Hit::Row(idx)
        } else {
            Hit::None
        };
    }
    if y >= layout.footer_top && y < layout.footer_top + 2 * FOOTER_ITEM_H {
        return match (y - layout.footer_top) / FOOTER_ITEM_H {
            0 => Hit::Refresh,
            _ => Hit::ToggleAutostart,
        };
    }
    Hit::None
}

/// Ajusta la posición para que el popup quede dentro del área de trabajo.
fn clamp_pos(x: i32, y: i32, w: i32, h: i32, work: RECT) -> (i32, i32) {
    let x = x.clamp(work.left, (work.right - w).max(work.left));
    let y = y.clamp(work.top, (work.bottom - h).max(work.top));
    (x, y)
}

/// Desplazamiento de scroll por rueda: 3 filas por muesca, acotado.
fn scroll_step(current: usize, wheel_delta: i32, max: usize) -> usize {
    if wheel_delta > 0 {
        current.saturating_sub(WHEEL_STEP)
    } else {
        current.saturating_add(WHEEL_STEP).min(max)
    }
}

// ── Estado y recursos GDI (un solo popup a la vez, mismo hilo) ───────────────
struct PopupState {
    ports: Vec<PortInfo>,
    autostart_on: bool,
    action: Option<Action>,
    hover: Option<Hit>,
    scroll: usize,
    done: bool,
    tracking: bool,
    /// Tooltip de filas (hwnd raw) que muestra la ruta completa bajo el cursor.
    tooltip: Option<usize>,
    /// `TTTOOLINFOW` registrada con `TTM_ADDTOOLW`, necesaria para
    /// `TTM_TRACKACTIVATE`. Solo se toca desde el hilo de la UI.
    tool_ti: Option<ToolTi>,
}

/// Envuelve la `TTTOOLINFOW` del tooltip para poder guardarla en el estado
/// (`Mutex` exige `Send`). Contiene punteros raw, pero solo se usa desde el hilo
/// de la UI del popup; `unsafe impl` es seguro por ese invariante.
struct ToolTi(TTTOOLINFOW);
unsafe impl Send for ToolTi {}

static STATE: Mutex<Option<PopupState>> = Mutex::new(None);
static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();
/// Cursor de flecha del popup (raw `usize` porque `HCURSOR` no es `Sync`): evita
/// que el sistema deje el cursor previo del hilo (p. ej. el de espera) cuando la
/// clase no declara cursor propio.
static CURSOR_ARROW: OnceLock<usize> = OnceLock::new();
/// Época (ms) del último cierre del popup: el clic de bandeja que lo cerró puede
/// llegar después y no debe reabrirlo (carrera clásica de menús de bandeja).
static LAST_CLOSED_MS: AtomicU64 = AtomicU64::new(0);

struct Ui {
    brush_cream: HBRUSH,
    brush_lavender: HBRUSH,
    brush_forest: HBRUSH,
    brush_stone: HBRUSH,
    brush_ink: HBRUSH,
    pen_ink2: HPEN,
    pen_stone2: HPEN,
    pen_lavender2: HPEN,
    fonts: &'static fonts::Fonts,
}

// Los objetos GDI viven durante todo el proceso y solo se tocan desde el hilo de
// la UI; el marcado manual permite exponerlos vía `LazyLock` estático.
unsafe impl Send for Ui {}
unsafe impl Sync for Ui {}

static UI: LazyLock<Ui> = LazyLock::new(|| unsafe {
    Ui {
        brush_cream: CreateSolidBrush(CREAM),
        brush_lavender: CreateSolidBrush(LAVENDER),
        brush_forest: CreateSolidBrush(FOREST),
        brush_stone: CreateSolidBrush(STONE),
        brush_ink: CreateSolidBrush(INK),
        pen_ink2: CreatePen(PS_SOLID, 2, INK),
        pen_stone2: CreatePen(PS_SOLID, 2, STONE),
        pen_lavender2: CreatePen(PS_SOLID, 2, LAVENDER),
        fonts: fonts::get(),
    }
});

/// Muestra el popup modal en el cursor y devuelve la acción elegida (bloqueante).
pub fn show(owner: HWND, ports: Vec<PortInfo>, autostart_on: bool) -> Action {
    if STATE.lock().unwrap().is_some() {
        // Popup ya abierto: reentrada del mismo hilo (clic en bandeja), se ignora.
        return Action::None;
    }
    unsafe {
        register_class();
        let ports = truncate_ports(ports);
        let layout = Layout::new(ports.len());
        let (x, y) = popup_position(layout.width, layout.height);
        let Some(hinstance) = hinstance() else {
            return Action::None;
        };

        let hwnd = match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("GloryPortPopupWnd"),
            PCWSTR::null(),
            WS_POPUP,
            x,
            y,
            layout.width,
            layout.height,
            Some(owner),
            None,
            Some(hinstance),
            None,
        ) {
            Ok(h) if !h.is_invalid() => h,
            _ => return Action::None,
        };

        let rgn = CreateRoundRectRgn(
            0,
            0,
            layout.width + 1,
            layout.height + 1,
            CORNER_RADIUS * 2,
            CORNER_RADIUS * 2,
        );
        if !rgn.is_invalid() {
            let _ = SetWindowRgn(hwnd, Some(rgn), true);
        }

        let mut state = PopupState {
            ports,
            autostart_on,
            action: None,
            hover: None,
            scroll: 0,
            done: false,
            tracking: false,
            tooltip: None,
            tool_ti: None,
        };
        if let Some((hwnd_tooltip, tool_ti)) = create_row_tooltip(hwnd) {
            state.tooltip = Some(hwnd_tooltip.0 as usize);
            state.tool_ti = Some(tool_ti);
        }
        *STATE.lock().unwrap() = Some(state);

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));

        // Bucle modal propio: no usa PostQuitMessage; termina con WM_APP_POPUP_DONE.
        let mut msg = MSG::default();
        let mut quit_code: Option<i32> = None;
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 <= 0 {
                quit_code = Some(msg.wParam.0 as i32);
                break;
            }
            if msg.message == WM_APP_POPUP_DONE {
                break;
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }

        let _ = destroy_row_tooltip();
        let _ = DestroyWindow(hwnd);
        record_close();
        // Si llegó un WM_QUIT externo, se re-encola para que el bucle de la bandeja salga.
        if let Some(code) = quit_code {
            PostQuitMessage(code);
        }
    }
    let action = STATE
        .lock()
        .unwrap()
        .take()
        .and_then(|s| s.action)
        .unwrap_or(Action::None);
    action
}

/// Registra el cierre del popup con la hora actual (ms desde la época).
fn record_close() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    LAST_CLOSED_MS.store(now, Ordering::Relaxed);
}

/// ¿Cerró un popup hace menos de `within`? Si es así, el clic de bandeja que se
/// está procesando probablemente es el mismo gesto que provocó el cierre y se
/// consume sin reabrir.
pub fn closed_recently(within: Duration) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = LAST_CLOSED_MS.load(Ordering::Relaxed);
    was_recent(now, last, within.as_millis() as u64)
}

/// Función pura del filtro temporal, testeable sin reloj real.
fn was_recent(now_ms: u64, last_ms: u64, within_ms: u64) -> bool {
    last_ms != 0 && now_ms.saturating_sub(last_ms) <= within_ms
}

/// ¿Hay un popup abierto en este momento?
pub fn is_open() -> bool {
    STATE.lock().unwrap().is_some()
}

/// Cierra el popup activo sin acción (p. ej. segundo clic en el icono de bandeja).
pub fn cancel_active() {
    if !is_open() {
        return;
    }
    unsafe {
        if let Ok(hwnd) = FindWindowW(w!("GloryPortPopupWnd"), PCWSTR::null()) {
            if !hwnd.is_invalid() {
                finish(hwnd, Action::None);
            }
        }
    }
}

/// Destruye el tooltip de filas (hijo del popup) si sigue vivo.
unsafe fn destroy_row_tooltip() -> bool {
    let guard = STATE.lock().unwrap();
    let Some(tooltip) = guard.as_ref().and_then(|s| s.tooltip) else {
        return false;
    };
    let hwnd = HWND(tooltip as *mut core::ffi::c_void);
    let _ = DestroyWindow(hwnd);
    true
}

/// Crea el tooltip de filas (hijo del popup, estilo TTS_NOPREFIX) que muestra la
/// ruta completa. Es un tooltip "track": se posiciona y se activa/desactiva
/// explícitamente desde `set_hover`, sin depender de que el control subclasee al
/// popup (fallo observado con el tooltip clásico: no aparecía). Devuelve el hwnd
/// y la `TTTOOLINFOW` registrada (guardada en el estado del popup).
unsafe fn create_row_tooltip(parent: HWND) -> Option<(HWND, ToolTi)> {
    let hinstance = hinstance()?;
    let hwnd = CreateWindowExW(
        Default::default(),
        TOOLTIPS_CLASSW,
        PCWSTR::null(),
        // TTS_ALWAYSTIP: el tooltip se muestra aunque el popup no esté activo.
        // TTS_NOPREFIX: no interpreta '&' como acelerador en las rutas.
        WINDOW_STYLE(WS_POPUP.0 | TTS_ALWAYSTIP | TTS_NOPREFIX),
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        Some(parent),
        None,
        Some(hinstance),
        None,
    )
    .ok()?;
    if hwnd.is_invalid() {
        return None;
    }
    let _ = SendMessageW(
        hwnd,
        WM_SETFONT,
        Some(WPARAM(fonts::get().figtree_400_13.0 as usize)),
        Some(LPARAM(1)),
    );
    // 0 = el tooltip no envuelve; la ruta se muestra en una sola línea completa.
    let _ = SendMessageW(
        hwnd,
        TTM_SETMAXTIPWIDTH,
        Some(WPARAM(0)),
        Some(LPARAM(TOOLTIP_MAX_WIDTH as isize)),
    );
    let _ = SendMessageW(
        hwnd,
        TTM_SETDELAYTIME,
        Some(WPARAM(TTDT_INITIAL as usize)),
        Some(LPARAM(TOOLTIP_INITIAL_MS as isize)),
    );
    let _ = SendMessageW(
        hwnd,
        TTM_SETDELAYTIME,
        Some(WPARAM(TTDT_AUTOPOP as usize)),
        Some(LPARAM(TOOLTIP_AUTOPOP_MS as isize)),
    );
    let _ = SendMessageW(
        hwnd,
        TTM_SETDELAYTIME,
        Some(WPARAM(TTDT_RESHOW as usize)),
        Some(LPARAM(TOOLTIP_RESHOW_MS as isize)),
    );
    let _ = SendMessageW(
        hwnd,
        TTM_SETTIPBKCOLOR,
        Some(WPARAM(INK.0 as usize)),
        Some(LPARAM(0)),
    );
    let _ = SendMessageW(
        hwnd,
        TTM_SETTIPTEXTCOLOR,
        Some(WPARAM(CREAM.0 as usize)),
        Some(LPARAM(0)),
    );
    // TTF_IDISHWND: el tooltip pertenece al popup y las notificaciones de texto
    // (TTN_NEEDTEXTW) llegan a `wnd_proc`; no se subclasea ningún control hijo.
    // TTF_TRACK|TTF_ABSOLUTE: el tooltip se posiciona y se activa/desactiva
    // explícitamente desde `set_hover` (plan robusto: no depende de que el
    // control detecte el rect por sí solo).
    let ti = TTTOOLINFOW {
        cbSize: size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_IDISHWND | TTF_TRACK | TTF_ABSOLUTE,
        hwnd: parent,
        uId: parent.0 as usize,
        // LPSTR_TEXTCALLBACK: sin texto fijo; el control pide el texto por
        // TTN_NEEDTEXTW en cada fila. Con lpszText = NULL no habría nada que
        // mostrar y el tooltip jamás aparecería.
        lpszText: PWSTR(-1isize as *mut u16),
        rect: RECT {
            left: 0,
            top: 0,
            right: WIDTH,
            bottom: MAX_VISIBLE_ROWS as i32 * ROW_H + PAD_TOP + BORDER,
        },
        ..Default::default()
    };
    let ti_ptr = &ti as *const TTTOOLINFOW as usize;
    let _ = SendMessageW(
        hwnd,
        TTM_ADDTOOLW,
        Some(WPARAM(0)),
        Some(LPARAM(ti_ptr as isize)),
    );
    Some((hwnd, ToolTi(ti)))
}

unsafe fn hinstance() -> Option<HINSTANCE> {
    GetModuleHandleW(None).map(|h| h.into()).ok()
}

unsafe fn register_class() {
    if CLASS_REGISTERED.get().is_some() {
        return;
    }
    let Some(hinstance) = hinstance() else {
        return;
    };
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or(HCURSOR(std::ptr::null_mut())),
        lpszClassName: w!("GloryPortPopupWnd"),
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        return; // fallo de registro: se reintenta en la próxima apertura
    }
    if !wc.hCursor.is_invalid() {
        // El cursor del sistema no se destruye; se conserva para WM_SETCURSOR.
        let _ = CURSOR_ARROW.set(wc.hCursor.0 as usize);
    }
    let _ = CLASS_REGISTERED.set(());
}

/// Posición en el cursor, recortada al área de trabajo del monitor bajo el puntero.
fn popup_position(w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = std::mem::zeroed::<MONITORINFO>();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut info).as_bool() {
            clamp_pos(pt.x, pt.y, w, h, info.rcWork)
        } else {
            (pt.x, pt.y)
        }
    }
}

fn truncate_ports(ports: Vec<PortInfo>) -> Vec<PortInfo> {
    ports.into_iter().take(MAX_TOTAL_ROWS).collect()
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
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
    let mut guard = STATE.lock().unwrap();
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
        let guard = STATE.lock().unwrap();
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
        let hit = STATE.lock().unwrap().as_ref().and_then(|s| s.hover);
        finish(hwnd, action_for(hit.unwrap_or(Hit::None)));
    }
}

fn action_for(hit: Hit) -> Action {
    match hit {
        Hit::None => Action::None,
        Hit::Row(idx) => STATE
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.ports.get(idx).cloned())
            .map(Action::Kill)
            .unwrap_or(Action::None),
        Hit::Refresh => Action::Refresh,
        Hit::ToggleAutostart => Action::ToggleAutostart,
    }
}

/// Registra la acción, marca `done` (evita doble cierre) y despierta el bucle modal.
fn finish(hwnd: HWND, action: Action) {
    {
        let mut guard = STATE.lock().unwrap();
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
    let mut guard = STATE.lock().unwrap();
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
    let mut guard = STATE.lock().unwrap();
    if let Some(s) = guard.as_mut() {
        s.tracking = false;
    }
}

fn set_hover(hwnd: HWND, hit: Option<Hit>) {
    let mut guard = STATE.lock().unwrap();
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
    let guard = STATE.lock().unwrap();
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

// ── Pintado con doble buffer GDI ─────────────────────────────────────────────

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        if hdc.is_invalid() {
            return;
        }
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bmp.into());

        if let Ok(guard) = STATE.lock() {
            if let Some(state) = guard.as_ref() {
                draw_all(mem, w, h, state);
            }
        }

        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        let _ = SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn draw_all(dc: HDC, w: i32, h: i32, state: &PopupState) {
    unsafe {
        let ui = &UI;
        let layout = Layout::new(state.ports.len());

        // Fondo crema + borde tinta de 2 px (la región de ventana recorta las esquinas).
        let _ = SelectObject(dc, ui.brush_cream.into());
        let _ = SelectObject(dc, ui.pen_ink2.into());
        let _ = RoundRect(dc, 1, 1, w - 1, h - 1, CORNER_RADIUS * 2, CORNER_RADIUS * 2);

        // Filas de puertos (con scroll si hace falta).
        let rows_total = state.ports.len();
        for vis in 0..layout.rows_visible {
            let row_rect = layout.row_rect(vis);
            let idx = state.scroll + vis;
            if idx >= rows_total {
                if rows_total == 0 && vis == 0 {
                    text(
                        dc,
                        ui.fonts.figtree_400_13,
                        "Sin puertos TCP en escucha",
                        row_rect,
                        FOG,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                }
                continue;
            }
            draw_row(dc, ui, &layout, &row_rect, state, idx);
            if vis + 1 < layout.rows_visible {
                let sep = RECT {
                    left: PAD_X,
                    top: row_rect.bottom - 1,
                    right: w - PAD_X,
                    bottom: row_rect.bottom,
                };
                let _ = FillRect(dc, &sep, ui.brush_stone);
            }
        }

        if layout.has_scroll {
            draw_scrollbar(dc, ui, &layout, state);
        }

        // Pie: Actualizar lista e Iniciar con Windows (toggle).
        draw_footer(dc, ui, &layout, state);
    }
}

fn draw_row(dc: HDC, ui: &Ui, layout: &Layout, row_rect: &RECT, state: &PopupState, idx: usize) {
    unsafe {
        let row = &state.ports[idx];
        if state.hover == Some(Hit::Row(idx)) {
            let pill = RECT {
                left: PAD_X - 6,
                top: row_rect.top + 3,
                right: layout.content_right() + 6,
                bottom: row_rect.bottom - 3,
            };
            round_pill(dc, pill, ui.brush_lavender, ui.pen_lavender2);
        }

        let port_rect = RECT {
            left: PAD_X,
            top: row_rect.top,
            right: PAD_X + 56,
            bottom: row_rect.bottom,
        };
        text(
            dc,
            ui.fonts.figtree_600_14,
            &row.port.to_string(),
            port_rect,
            INK,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );

        // Sin PID en la fila (decisión del usuario): la etiqueta llega hasta el
        // borde derecho del contenido. El PID sigue visible en el CLI/JSON.
        let name_rect = RECT {
            left: PAD_X + 60,
            top: row_rect.top,
            right: layout.content_right(),
            bottom: row_rect.bottom,
        };
        // Elipsis al INICIO: cuando la ruta no cabe, se recorta el comienzo y se
        // conserva el final (lo identificable), en lugar de cortar la cola.
        let etiqueta = crate::ports::etiqueta_popup(row);
        let (ancho, _) = measure(dc, ui.fonts.figtree_400_13, &etiqueta);
        if ancho <= name_rect.right - name_rect.left {
            // La ruta completa cabe en la fila: se dibuja entera.
            text(
                dc,
                ui.fonts.figtree_400_13,
                &etiqueta,
                name_rect,
                INK,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        } else {
            // No cabe: se recorta el comienzo conservando el final identificable.
            text_leading_ellipsis(dc, ui.fonts.figtree_400_13, &etiqueta, name_rect, INK);
        }
    }
}

fn draw_footer(dc: HDC, ui: &Ui, layout: &Layout, state: &PopupState) {
    unsafe {
        let items = [
            ("Actualizar lista", Hit::Refresh),
            ("Iniciar con Windows", Hit::ToggleAutostart),
        ];
        for (i, (label, hit)) in items.iter().enumerate() {
            let item_rect = RECT {
                left: PAD_X,
                top: layout.footer_top + i as i32 * FOOTER_ITEM_H,
                right: layout.width - PAD_X,
                bottom: layout.footer_top + (i as i32 + 1) * FOOTER_ITEM_H,
            };
            if state.hover == Some(*hit) {
                let pill = RECT {
                    left: PAD_X - 6,
                    top: item_rect.top + 3,
                    right: layout.width - PAD_X + 6,
                    bottom: item_rect.bottom - 3,
                };
                round_pill(dc, pill, ui.brush_lavender, ui.pen_lavender2);
            }
            text(
                dc,
                ui.fonts.figtree_400_13,
                label,
                item_rect,
                INK,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            if *hit == Hit::ToggleAutostart {
                draw_toggle(dc, ui, &item_rect, state.autostart_on);
            }
        }
    }
}

fn draw_toggle(dc: HDC, ui: &Ui, item_rect: &RECT, on: bool) {
    unsafe {
        let label = if on { "SÍ" } else { "NO" };
        let (tw, _) = measure(dc, ui.fonts.figtree_500_11, label);
        let pw = tw + 14;
        let ph = 18;
        let pill = RECT {
            left: item_rect.right - pw,
            top: item_rect.top + (item_rect.bottom - item_rect.top - ph) / 2,
            right: item_rect.right,
            bottom: item_rect.top + (item_rect.bottom - item_rect.top - ph) / 2 + ph,
        };
        if on {
            round_pill(dc, pill, ui.brush_forest, ui.pen_ink2);
            text(
                dc,
                ui.fonts.figtree_500_11,
                label,
                pill,
                CREAM,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        } else {
            round_pill(dc, pill, ui.brush_cream, ui.pen_stone2);
            text(
                dc,
                ui.fonts.figtree_500_11,
                label,
                pill,
                INK,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
    }
}

fn draw_scrollbar(dc: HDC, ui: &Ui, layout: &Layout, state: &PopupState) {
    unsafe {
        let track = RECT {
            left: layout.width - BORDER - SCROLL_W - 3,
            top: layout.rows_top + 2,
            right: layout.width - BORDER - 3,
            bottom: layout.rows_bottom - 2,
        };
        let _ = FillRect(dc, &track, ui.brush_stone);

        let total = state.ports.len().max(1);
        let thumb_h = ((track.bottom - track.top) * total.min(MAX_VISIBLE_ROWS) as i32
            / total as i32)
            .max(16);
        let travel = (track.bottom - track.top - thumb_h).max(0);
        let thumb_top = if layout.max_scroll > 0 {
            track.top + travel * state.scroll as i32 / layout.max_scroll as i32
        } else {
            track.top
        };
        let thumb = RECT {
            left: track.left,
            top: thumb_top,
            right: track.right,
            bottom: thumb_top + thumb_h,
        };
        let _ = FillRect(dc, &thumb, ui.brush_ink);
    }
}

/// Pill redondeada al mínimo de sus dimensiones (radio completo en los extremos).
unsafe fn round_pill(dc: HDC, rc: RECT, brush: HBRUSH, pen: HPEN) {
    let d = (rc.right - rc.left).min(rc.bottom - rc.top);
    let _ = SelectObject(dc, brush.into());
    let _ = SelectObject(dc, pen.into());
    let _ = RoundRect(dc, rc.left, rc.top, rc.right, rc.bottom, d, d);
}

unsafe fn text(dc: HDC, font: HFONT, s: &str, rc: RECT, color: COLORREF, flags: DRAW_TEXT_FORMAT) {
    let mut buf: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = SetTextColor(dc, color);
    let _ = SetBkMode(dc, TRANSPARENT);
    let old = SelectObject(dc, font.into());
    let mut r = rc;
    let _ = DrawTextW(dc, &mut buf, &mut r, flags);
    let _ = SelectObject(dc, old);
}

/// Texto que, si no cabe en `rc`, se recorta por el PRINCIPIO anteponiendo `…`
/// (conserva el final de la ruta). `DT_BEGINNING_ELLIPSIS` no existe en Win32,
/// por eso se mide con `GetTextExtentPoint32W` y se busca la cola más larga que quepa.
unsafe fn text_leading_ellipsis(dc: HDC, font: HFONT, s: &str, rc: RECT, color: COLORREF) {
    let avail = rc.right - rc.left;
    let out = sufijo_con_elipsis(s, |c| {
        let (cw, _) = measure(dc, font, c);
        cw <= avail
    });
    text(
        dc,
        font,
        &out,
        rc,
        color,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

/// Calcula la cadena a dibujar cuando el ancho es limitado: si el texto completo no
/// cabe, recorta por el PRINCIPIO (búsqueda binaria sobre índices de carácter) y
/// antepone `…`, conservando el final de la ruta. `cabe` mide una candidata.
fn sufijo_con_elipsis(s: &str, cabe: impl Fn(&str) -> bool) -> String {
    if cabe(s) {
        return s.to_string();
    }
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let cand = format!("…{}", &s[chars[mid].0..]);
        if cabe(&cand) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    if lo < chars.len() {
        format!("…{}", &s[chars[lo].0..])
    } else {
        "…".to_string()
    }
}

unsafe fn measure(dc: HDC, font: HFONT, s: &str) -> (i32, i32) {
    let buf: Vec<u16> = s.encode_utf16().collect();
    let old = SelectObject(dc, font.into());
    let mut size = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &buf, &mut size);
    let _ = SelectObject(dc, old);
    (size.cx, size.cy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(y: i32) -> (i32, i32) {
        (PAD_X, y)
    }

    #[test]
    fn sufijo_con_elipsis_conserva_el_final() {
        let ruta = "…\\codex-bridge\\bridge\\server.js";

        // Cabe completo: sin recorte.
        assert_eq!(sufijo_con_elipsis(ruta, |s| s.len() <= 40), ruta);

        // No cabe: se recorta el PRINCIPIO y se conserva el final de la ruta.
        assert_eq!(
            sufijo_con_elipsis(ruta, |s| s.len() <= 20),
            "…\\bridge\\server.js"
        );

        // Caso límite: ni el último carácter con `…` cabe → solo la elipsis.
        assert_eq!(sufijo_con_elipsis("abc", |s| s.len() <= 1), "…");
    }

    #[test]
    fn layout_grows_with_rows_and_caps_visible() {
        let empty = Layout::new(0);
        assert_eq!(empty.rows_visible, 1);
        assert_eq!(empty.max_scroll, 0);

        let five = Layout::new(5);
        assert_eq!(five.rows_visible, 5);
        assert_eq!(five.max_scroll, 0);
        assert!(!five.has_scroll);
        assert_eq!(five.height, 5 * ROW_H + 94);

        let nine = Layout::new(9);
        assert_eq!(nine.rows_visible, MAX_VISIBLE_ROWS);
        assert_eq!(nine.max_scroll, 0);
        assert!(!nine.has_scroll);
    }

    #[test]
    fn hit_test_rows_and_footer() {
        let layout = Layout::new(8);
        assert_eq!(
            hit_test(row(layout.rows_top + 5), &layout, 0, 8),
            Hit::Row(0)
        );
        assert_eq!(
            hit_test(row(layout.rows_top + ROW_H + 5), &layout, 0, 8),
            Hit::Row(1)
        );
        assert_eq!(
            hit_test(row(layout.footer_top + 5), &layout, 0, 8),
            Hit::Refresh
        );
        assert_eq!(
            hit_test(row(layout.footer_top + FOOTER_ITEM_H + 5), &layout, 0, 8),
            Hit::ToggleAutostart
        );
        assert_eq!(hit_test((2, 2), &layout, 0, 8), Hit::None);
    }

    #[test]
    fn hit_test_applies_scroll_and_ignores_empty_rows() {
        let layout = Layout::new(10);
        let bottom = layout.rows_top + (MAX_VISIBLE_ROWS as i32 - 1) * ROW_H + 5;
        assert_eq!(hit_test(row(bottom), &layout, 0, 10), Hit::Row(8));
        assert_eq!(hit_test(row(bottom), &layout, 1, 10), Hit::Row(9));
        assert_eq!(hit_test(row(bottom), &layout, 1, 0), Hit::None);
    }

    #[test]
    fn clamp_pos_keeps_window_inside_work_area() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(clamp_pos(0, 0, 300, 400, work), (0, 0));
        assert_eq!(clamp_pos(1800, 900, 300, 400, work), (1620, 640));
        assert_eq!(clamp_pos(-50, -20, 300, 400, work), (0, 0));
    }

    #[test]
    fn wheel_scroll_is_bounded() {
        assert_eq!(scroll_step(0, 120, 5), 0);
        assert_eq!(scroll_step(2, 120, 5), 0);
        assert_eq!(scroll_step(0, -120, 5), 3);
        assert_eq!(scroll_step(4, -120, 5), 5);
        assert_eq!(scroll_step(5, -120, 5), 5);
    }

    #[test]
    fn truncate_ports_keeps_cap() {
        let rows: Vec<PortInfo> = (0..65).map(port).collect();
        let kept = truncate_ports(rows);
        assert_eq!(kept.len(), MAX_TOTAL_ROWS);
    }

    #[test]
    fn close_suppression_only_after_a_real_close() {
        assert!(!was_recent(5_000, 0, 250)); // sin cierre previo
        assert!(was_recent(5_100, 5_000, 250)); // clic del mismo gesto
        assert!(!was_recent(5_500, 5_000, 250)); // gesto nuevo, fuera de la ventana
    }

    fn port(n: u16) -> PortInfo {
        PortInfo {
            port: n,
            pid: u32::from(n) + 100,
            address: "0.0.0.0".into(),
            process_name: "node.exe".into(),
            process_path: Some(r"C:\Program Files\nodejs\node.exe".into()),
            process_cmd: None,
            proyecto: None,
        }
    }
}
