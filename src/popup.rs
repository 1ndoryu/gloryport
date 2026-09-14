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
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateRoundRectRgn, CreateSolidBrush, GetMonitorInfoW, MonitorFromPoint,
    SetWindowRgn, HBRUSH, HPEN, MONITORINFO, MONITOR_DEFAULTTONEAREST, PS_SOLID,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    TOOLTIPS_CLASSW, TTDT_AUTOPOP, TTDT_INITIAL, TTDT_RESHOW, TTF_ABSOLUTE, TTF_IDISHWND,
    TTF_TRACK, TTM_ADDTOOLW, TTM_SETDELAYTIME, TTM_SETMAXTIPWIDTH, TTM_SETTIPBKCOLOR,
    TTM_SETTIPTEXTCOLOR, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW,
    LoadCursorW, PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, ShowWindow,
    TranslateMessage, CW_USEDEFAULT, HCURSOR, IDC_ARROW, MSG, SW_SHOWNOACTIVATE, WINDOW_STYLE,
    WM_APP, WM_SETFONT, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::fonts;
use crate::ports::PortInfo;

mod interaccion;
mod pintado;

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

/// Bloquea `STATE` recuperándose del envenenamiento del mutex: si otra hebra
/// paniqueó mientras lo sostenía, `into_inner()` rescata el dato en vez de
/// propagar el panic y tumbar el popup de producción.
fn lock_state() -> std::sync::MutexGuard<'static, Option<PopupState>> {
    match STATE.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

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
    if lock_state().is_some() {
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
        *lock_state() = Some(state);

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
    lock_state()
        .take()
        .and_then(|s| s.action)
        .unwrap_or(Action::None)
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
    lock_state().is_some()
}

/// Cierra el popup activo sin acción (p. ej. segundo clic en el icono de bandeja).
pub fn cancel_active() {
    if !is_open() {
        return;
    }
    unsafe {
        if let Ok(hwnd) = FindWindowW(w!("GloryPortPopupWnd"), PCWSTR::null()) {
            if !hwnd.is_invalid() {
                interaccion::finish(hwnd, Action::None);
            }
        }
    }
}

/// Destruye el tooltip de filas (hijo del popup) si sigue vivo.
unsafe fn destroy_row_tooltip() -> bool {
    let guard = lock_state();
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
        lpfnWndProc: Some(interaccion::wnd_proc),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(y: i32) -> (i32, i32) {
        (PAD_X, y)
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
