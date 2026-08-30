//! Modos de ejecución: bandeja (default), `list`, `kill`, `--version`.
//!
//! El mismo binario sirve como app de bandeja y como herramienta CLI para scripting;
//! con `windows_subsystem = "windows"` no abre ventana de consola al arrancar como app,
//! pero hereda stdout/stderr cuando se invoca desde una terminal.

use std::process::ExitCode;

use crate::{ports, process, tray};

/// Imprime a stdout ignorando tuberías cerradas (evita panic por broken pipe).
macro_rules! outln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// Imprime a stderr ignorando tuberías cerradas.
macro_rules! errln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        None | Some("tray") => match tray::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                errln!("gloryport: {e}");
                ExitCode::FAILURE
            }
        },
        Some("list") => cmd_list(&args[1..]),
        Some("kill") => cmd_kill(&args[1..]),
        Some("--version") | Some("version") | Some("-V") => {
            outln!("gloryport {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help") | Some("help") | Some("-h") => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            errln!("gloryport: comando desconocido '{other}'");
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn cmd_list(args: &[String]) -> ExitCode {
    let json = args.iter().any(|a| a == "--json");
    let include_system = args.iter().any(|a| a == "--incluir-sistema");
    if let Some(unknown) = args
        .iter()
        .find(|a| a.as_str() != "--json" && a.as_str() != "--incluir-sistema")
    {
        errln!("gloryport: opción desconocida '{unknown}'");
        return ExitCode::from(2);
    }
    let mut rows = match ports::scan_listeners() {
        Ok(rows) => rows,
        Err(e) => {
            errln!("gloryport: {e}");
            return ExitCode::FAILURE;
        }
    };
    let cfg = crate::config::Config::load();
    ports::attach_process_names(&mut rows, &mut ports::NameCache::new(), &cfg);
    // Por defecto solo aplicaciones de usuario; el sistema se ve con --incluir-sistema.
    if !include_system {
        rows = ports::solo_aplicaciones(rows, &cfg);
    }

    if json {
        match serde_json::to_string_pretty(&rows) {
            Ok(text) => outln!("{text}"),
            Err(e) => {
                errln!("gloryport: no se pudo serializar: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        outln!(
            "{:<7} {:<21} {:<8} {:<22} PROCESO",
            "PUERTO",
            "DIRECCIÓN",
            "PID",
            "PROYECTO"
        );
        for row in &rows {
            outln!(
                "{:<7} {:<21} {:<8} {:<22} {}",
                row.port,
                row.address,
                row.pid,
                row.proyecto.as_deref().unwrap_or("-"),
                ports::etiqueta_visible(row)
            );
        }
    }
    ExitCode::SUCCESS
}

fn cmd_kill(args: &[String]) -> ExitCode {
    let mut port_arg: Option<&str> = None;
    let mut pid_filter: Option<u32> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--pid" => {
                i += 1;
                let raw = args.get(i).map(String::as_str).unwrap_or_default();
                match raw.parse::<u32>() {
                    Ok(pid) => pid_filter = Some(pid),
                    Err(_) => {
                        errln!("gloryport: '--pid' requiere un número válido");
                        return ExitCode::from(2);
                    }
                }
            }
            other if other.starts_with('-') => {
                errln!("gloryport: opción desconocida '{other}'");
                return ExitCode::from(2);
            }
            other => {
                if port_arg.is_some() {
                    errln!("gloryport: solo se acepta un puerto");
                    return ExitCode::from(2);
                }
                port_arg = Some(other);
            }
        }
        i += 1;
    }

    let Some(port_text) = port_arg else {
        errln!("gloryport: uso: gloryport kill <puerto> [--pid <PID>]");
        return ExitCode::from(2);
    };
    let port: u16 = match port_text.parse() {
        Ok(p) if p > 0 => p,
        _ => {
            errln!("gloryport: puerto inválido '{port_text}' (1–65535)");
            return ExitCode::from(2);
        }
    };

    let mut rows = match ports::scan_listeners() {
        Ok(rows) => rows,
        Err(e) => {
            errln!("gloryport: {e}");
            return ExitCode::FAILURE;
        }
    };
    let cfg = crate::config::Config::load();
    ports::attach_process_names(&mut rows, &mut ports::NameCache::new(), &cfg);
    let targets: Vec<_> = rows
        .into_iter()
        .filter(|r| r.port == port && pid_filter.is_none_or(|pid| r.pid == pid))
        .collect();

    if targets.is_empty() {
        let detail = match pid_filter {
            Some(pid) => format!(" (PID {pid})"),
            None => String::new(),
        };
        errln!("gloryport: ningún proceso escuchando en el puerto {port}{detail}");
        return ExitCode::FAILURE;
    }

    let mut all_ok = true;
    for row in &targets {
        match process::kill_pid(row.pid) {
            Ok(()) => outln!(
                "ok: puerto {} liberado (terminado {} PID {})",
                row.port,
                row.process_name,
                row.pid
            ),
            Err(e) => {
                all_ok = false;
                errln!(
                    "fallo: puerto {} — {} (PID {}): {e}",
                    row.port,
                    row.process_name,
                    row.pid
                );
            }
        }
    }
    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn print_usage() {
    outln!(
        "GLORYPORT v{} — puertos TCP en escucha desde la bandeja (Windows)\n\
         \n\
         Uso:\n\
         \x20 gloryport            app de bandeja (default)\n\
         \x20 gloryport tray       igual que el anterior, explícito\n\
         \x20 gloryport list       lista puertos en escucha (tabla)\n\
         \x20 gloryport list --json  ídem en JSON\n\
         \x20 gloryport list --incluir-sistema  incluye servicios y puertos del sistema\n\
         \x20 gloryport kill <puerto> [--pid <PID>]  termina el proceso del puerto\n\
         \x20 gloryport --version  versión\n\
         \x20 gloryport --help     esta ayuda",
        env!("CARGO_PKG_VERSION")
    );
}
