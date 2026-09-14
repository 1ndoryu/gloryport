//! Binario auxiliar SOLO para pruebas funcionales: mantiene un puerto TCP ocupado
//! hasta que lo maten. No forma parte del producto y no se publica en releases.

use std::net::TcpListener;
use std::time::Duration;

// Errores explícitos en vez de `expect`: el helper solo existe para las pruebas
// E2E, pero el gate mide todo `src/` como producción, y un pánico no distingue
// "faltó el argumento" de "el puerto está ocupado". Con `Result` el fallo sale
// por stderr con código != 0 (lo que consume `tests/cli.rs`) y queda registrado.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = std::env::args()
        .nth(1)
        .ok_or("uso: gloryport-test-helper <puerto>")?
        .parse()
        .map_err(|e| format!("puerto inválido: {e}"))?;
    let addr = format!("127.0.0.1:{port}");
    let _listener =
        TcpListener::bind(&addr).map_err(|e| format!("no se pudo ocupar {addr}: {e}"))?;
    println!("listening {addr} pid={}", std::process::id());
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
