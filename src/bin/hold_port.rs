//! Binario auxiliar SOLO para pruebas funcionales: mantiene un puerto TCP ocupado
//! hasta que lo maten. No forma parte del producto y no se publica en releases.

use std::net::TcpListener;
use std::time::Duration;

fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .expect("uso: gloryport-test-helper <puerto>")
        .parse()
        .expect("puerto inválido");
    let addr = format!("127.0.0.1:{port}");
    let _listener = TcpListener::bind(&addr).expect("no se pudo ocupar el puerto");
    println!("listening {addr} pid={}", std::process::id());
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
