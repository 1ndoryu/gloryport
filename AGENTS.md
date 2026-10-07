# GLORYPORT — instrucciones del proyecto

Especializa el `AGENTS.md` del área (`area-trabajo/AGENTS.md`): el protocolo común,
el gate (§5–§6) y el sistema documental (§7) valen aquí; abajo solo lo propio del proyecto.

Herramienta de escritorio **solo Windows**, minimalista: lista puertos TCP en escucha y termina el
proceso desde la bandeja del sistema. Un solo binario Rust nativo (sin Electron), con modo CLI.

- **Windows only**: no añadir código ni dependencias para otras plataformas.
- **Minimalismo**: núcleo = escanear y matar; cada feature justifica su peso.
- **Recursos acotados**: sin timers en background; escaneo bajo demanda; NO invocar procesos externos
  (`netstat`, `taskkill`, `reg.exe`, PowerShell) — usar API Win32.
- **Un hilo**: el bucle de eventos de la bandeja es de un solo hilo; operaciones bloqueantes acotadas.
- **Errores explícitos**: nunca silenciar un fallo; estado, notificación o salida CLI con código.
- Comandos: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
  `cargo build --release` (binario `target/release/gloryport.exe`). Un commit solo con gate verde:
  fmt + clippy + test + verificación funcional real.
- Estructura: `src/ports.rs`, `src/process.rs`, `src/tray.rs`, `src/popup.rs`, `src/fonts.rs`,
  `src/autostart.rs`, `src/icon.rs`, `src/cli.rs`; `tools/make-fonts.py`, `tools/smoke-tray.ps1`, `docs/arquitectura.md`.
