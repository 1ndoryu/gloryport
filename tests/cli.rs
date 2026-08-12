//! Pruebas E2E reales: ocupa un puerto con el helper, lo lista y lo mata.

use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn gloryport() -> Command {
    Command::new(env!("CARGO_BIN_EXE_gloryport"))
}

fn helper() -> Command {
    Command::new(env!("CARGO_BIN_EXE_gloryport-test-helper"))
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_until_listening(port: u16, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn e2e_list_and_kill_port() {
    let port = free_port();
    let mut child: Child = helper()
        .arg(port.to_string())
        .stdout(Stdio::null())
        .spawn()
        .expect("helper debe arrancar");
    assert!(
        wait_until_listening(port, Duration::from_secs(10)),
        "el helper no quedó escuchando"
    );

    let list = gloryport().args(["list"]).output().expect("gloryport list");
    assert!(list.status.success(), "list falló: {:?}", list);
    let text = String::from_utf8_lossy(&list.stdout);
    assert!(
        text.contains(&format!("{port}")),
        "el puerto {port} no aparece en list:\n{text}"
    );

    let kill = gloryport()
        .args(["kill", &port.to_string()])
        .output()
        .expect("gloryport kill");
    let kill_text = String::from_utf8_lossy(&kill.stdout);
    assert!(
        kill.status.success(),
        "kill falló: {kill_text} / stderr: {}",
        String::from_utf8_lossy(&kill.stderr)
    );

    // El helper debe haber sido terminado por GLORYPORT.
    let status = child.wait().expect("wait del helper");
    assert!(!status.success(), "el helper debería haber muerto");

    // El puerto debe quedar libre para bindearlo de nuevo.
    assert!(
        TcpListener::bind(("127.0.0.1", port)).is_ok(),
        "el puerto {port} siguió ocupado tras el kill"
    );
}

#[test]
fn version_and_help_exit_zero() {
    let version = gloryport().arg("--version").output().unwrap();
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("gloryport"));

    let help = gloryport().arg("--help").output().unwrap();
    assert!(help.status.success());
}

#[test]
fn list_json_is_valid() {
    let out = gloryport().args(["list", "--json"]).output().unwrap();
    assert!(out.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("list --json debe ser JSON válido");
    assert!(value.is_array());
}
