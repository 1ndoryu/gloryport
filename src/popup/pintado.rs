//! Pintado del popup con doble buffer GDI: fondo, filas de puertos, pie (autostart/toggle) y
//! barra de scroll. Recibe el estado ya resuelto y dibuja sin tocar la ventana ni los mensajes.

use super::{
    Hit, Layout, PopupState, Ui, BORDER, CORNER_RADIUS, CREAM, FOG, FOOTER_ITEM_H, INK,
    MAX_VISIBLE_ROWS, PAD_X, SCROLL_W, STATE, UI,
};

use windows::Win32::Foundation::{COLORREF, HWND, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    DrawTextW, EndPaint, FillRect, GetTextExtentPoint32W, RoundRect, SelectObject, SetBkMode,
    SetTextColor, DRAW_TEXT_FORMAT, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    HBRUSH, HDC, HFONT, HPEN, SRCCOPY, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

pub(super) fn paint(hwnd: HWND) {
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
}
