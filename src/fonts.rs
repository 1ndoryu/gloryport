//! Fuentes embebidas del popup (OFL): Figtree y EB Garamond.
//!
//! Se registran como recursos de memoria con `AddFontMemResourceEx` (sin tocar la
//! instalación de fuentes del sistema) y se crean los `HFONT` correspondientes con
//! `CreateFontW`. Si el registro fallara, se cae a fuentes del sistema (Segoe UI /
//! Georgia) y la UI sigue funcionando sin degradar la paleta.
//!
//! Los TTFs estáticos se generan desde las fuentes variables oficiales de google/fonts
//! con `tools/make-fonts.py` (ver licencias OFL en `assets/fonts/`).

use std::ffi::c_void;
use std::sync::LazyLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Gdi::{
    AddFontMemResourceEx, CreateFontW, DeleteObject, RemoveFontMemResourceEx, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, HFONT, OUT_TT_PRECIS,
};

const FIGTREE_400: &[u8] = include_bytes!("../assets/fonts/Figtree-400.ttf");
const FIGTREE_500: &[u8] = include_bytes!("../assets/fonts/Figtree-500.ttf");
const FIGTREE_600: &[u8] = include_bytes!("../assets/fonts/Figtree-600.ttf");
const EB_GARAMOND_400: &[u8] = include_bytes!("../assets/fonts/EBGaramond-400.ttf");

/// Juego de fuentes usado por el popup, con el tamaño fijo de cada uso (píxeles).
pub struct Fonts {
    pub figtree_400_12: HFONT,
    pub figtree_400_13: HFONT,
    pub figtree_500_11: HFONT,
    pub figtree_500_13: HFONT,
    pub figtree_600_14: HFONT,
    pub garamond_20: HFONT,
    mem_handles: Vec<HANDLE>,
}

// Los HFONT se crean una sola vez y solo se usan desde el hilo de la UI; el
// marcado manual es seguro y permite exponerlos vía `LazyLock` estático.
unsafe impl Send for Fonts {}
unsafe impl Sync for Fonts {}

static FONTS: LazyLock<Fonts> = LazyLock::new(load_fonts);

/// Acceso al juego de fuentes (carga perezosa, una sola vez por proceso).
pub fn get() -> &'static Fonts {
    &FONTS
}

/// Libera los recursos de memoria y los HFONT al salir de la app (best-effort).
pub fn cleanup() {
    if let Some(fonts) = LazyLock::get(&FONTS) {
        unsafe {
            for hfont in [
                fonts.figtree_400_12,
                fonts.figtree_400_13,
                fonts.figtree_500_11,
                fonts.figtree_500_13,
                fonts.figtree_600_14,
                fonts.garamond_20,
            ] {
                let _ = DeleteObject(hfont.into());
            }
            for h in &fonts.mem_handles {
                let _ = RemoveFontMemResourceEx(*h);
            }
        }
    }
}

fn load_fonts() -> Fonts {
    unsafe {
        let mut mem_handles = Vec::new();
        let added: u32 = 0;
        for data in [FIGTREE_400, FIGTREE_500, FIGTREE_600, EB_GARAMOND_400] {
            let h = AddFontMemResourceEx(
                data.as_ptr() as *const c_void,
                data.len() as u32,
                None,
                &added,
            );
            if !h.is_invalid() {
                mem_handles.push(h);
            }
        }

        // Solo usamos las familias propias si se registraron las cuatro; en otro
        // caso caemos a tipografías del sistema para no romper el popup.
        let embedded_ok = mem_handles.len() == 4;
        let figtree_family = if embedded_ok { "Figtree" } else { "Segoe UI" };
        let garamond_family = if embedded_ok {
            "EB Garamond"
        } else {
            "Georgia"
        };

        Fonts {
            figtree_400_12: create_font(figtree_family, 400, 12),
            figtree_400_13: create_font(figtree_family, 400, 13),
            figtree_500_11: create_font(figtree_family, 500, 11),
            figtree_500_13: create_font(figtree_family, 500, 13),
            figtree_600_14: create_font(figtree_family, 600, 14),
            garamond_20: create_font(garamond_family, 400, 20),
            mem_handles,
        }
    }
}

/// Crea un HFONT TrueType con ClearType y altura en píxeles (negativa).
unsafe fn create_font(family: &str, weight: u16, size_px: i32) -> HFONT {
    let wide: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
    CreateFontW(
        -size_px,
        0,
        0,
        0,
        i32::from(weight),
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_TT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        CLEARTYPE_QUALITY,
        0,
        PCWSTR(wide.as_ptr()),
    )
}
