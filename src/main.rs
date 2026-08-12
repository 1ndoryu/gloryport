#![windows_subsystem = "windows"]

//! GLORYPORT — gestor minimalista de puertos TCP para Windows desde la bandeja.
//!
//! Ver `docs/arquitectura.md` para la decisión de diseño y el plan.

mod autostart;
mod cli;
mod icon;
mod ports;
mod process;
mod tray;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    cli::run(&args)
}
