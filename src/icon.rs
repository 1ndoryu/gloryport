//! Carga del icono embebido (`assets/gloryport.ico`) como `HICON`.
//!
//! Se parsea el contenedor ICO en memoria y se toma la imagen de mayor tamaño;
//! el binario queda autocontenido (sin archivo temporal ni recursos externos).

use windows::Win32::UI::WindowsAndMessaging::{CreateIconFromResourceEx, HICON};

const ICO_BYTES: &[u8] = include_bytes!("../assets/gloryport.ico");

pub fn load_icon() -> Result<HICON, String> {
    let (offset, size) = largest_image();
    let data = &ICO_BYTES[offset..offset + size];
    unsafe { CreateIconFromResourceEx(data, true, 0x0003_0000, 0, 0, Default::default()) }
        .map_err(|e| format!("CreateIconFromResourceEx falló: {e}"))
}

/// Devuelve `(offset, tamaño)` de la imagen más grande dentro del ICO.
fn largest_image() -> (usize, usize) {
    debug_assert!(ICO_BYTES.len() >= 6);
    let count = u16::from_le_bytes([ICO_BYTES[4], ICO_BYTES[5]]) as usize;
    let mut best = (6usize, 0usize);
    for i in 0..count {
        let entry = 6 + i * 16;
        if entry + 16 > ICO_BYTES.len() {
            break;
        }
        // Índices seguros por el guard de arriba (`entry + 16 > len` => break).
        // Se indexa byte a byte en vez de `slice.try_into().expect(..)`: esa
        // conversión era infalible por construcción (el slice tiene 4 bytes
        // literales) y el gate la contaba como pánico alcanzable.
        let size = u32::from_le_bytes([
            ICO_BYTES[entry + 8],
            ICO_BYTES[entry + 9],
            ICO_BYTES[entry + 10],
            ICO_BYTES[entry + 11],
        ]) as usize;
        let offset = u32::from_le_bytes([
            ICO_BYTES[entry + 12],
            ICO_BYTES[entry + 13],
            ICO_BYTES[entry + 14],
            ICO_BYTES[entry + 15],
        ]) as usize;
        if size > best.1 && offset + size <= ICO_BYTES.len() {
            best = (offset, size);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ico_has_valid_header() {
        assert_eq!(ICO_BYTES[0], 0);
        assert_eq!(ICO_BYTES[1], 0);
        assert_eq!(ICO_BYTES[2], 1); // tipo ICO
        assert!(u16::from_le_bytes([ICO_BYTES[4], ICO_BYTES[5]]) >= 1);
    }

    #[test]
    fn largest_image_is_within_bounds() {
        let (offset, size) = largest_image();
        assert!(size > 0);
        assert!(offset + size <= ICO_BYTES.len());
    }
}
