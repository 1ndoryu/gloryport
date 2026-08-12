#![windows_subsystem = "windows"]

//! GLORYPORT — gestor minimalista de puertos TCP para Windows desde la bandeja.
//!
//! Ver `docs/arquitectura.md` para la decisión de diseño y el plan.

mod autostart;
mod cli;
mod fonts;
mod icon;
mod popup;
mod ports;
mod process;
mod tray;

fn main() -> std::process::ExitCode {
    // Per-monitor DPI v2: el popup queda nítido en monitores con escalas distintas.
    let _ = unsafe {
        windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    cli::run(&args)
}
