# AGENTS.md — GLORYPORT

## Qué es

GLORYPORT es una herramienta de escritorio **solo Windows**, minimalista, que lista los
puertos TCP en escucha y permite terminar el proceso que los ocupa desde la **bandeja del
sistema**. Un solo binario Rust nativo (sin runtime externo, sin Electron), con modo CLI
para scripting.

## Contrato del proyecto

- **Windows only**: no añadir código ni dependencias para otras plataformas.
- **Minimalismo**: el núcleo del producto son escanear y matar. Cualquier feature nueva
  justifica su peso en memoria, binario y complejidad antes de entrar.
- **Recursos acotados**: no hay timers en background. Se escanea bajo demanda (menú o CLI).
  No invocar procesos externos (`netstat`, `taskkill`, `reg.exe`, PowerShell) desde el
  binario: usar las API Win32 correspondientes.
- **Un hilo**: el bucle de eventos de la bandeja es de un solo hilo. Toda operación que
  pueda bloquear debe ser acotada (escaneo: una llamada Win32; kill: inmediato).
- **Errores explícitos**: nunca silenciar un fallo; convertirlo en estado, notificación o
  salida CLI con código de error.

## Comandos canónicos

```powershell
cargo fmt --check        # formato
cargo clippy --all-targets -- -D warnings   # lint estricto
cargo test               # unit + integración (usa gloryport-test-helper)
cargo build --release    # binario final: target/release/gloryport.exe
```

## Gate de calidad

Antes de cerrar un bloque: `fmt --check`, `clippy -D warnings`, `test`, y una verificación
funcional real (listar un puerto ocupado por el helper y matarlo). Un commit solo con gate
verde.

## Estructura

- `src/ports.rs` — escáner `GetExtendedTcpTable` + resolución de nombres de proceso.
- `src/process.rs` — terminación de procesos (`OpenProcess`/`TerminateProcess`).
- `src/tray.rs` — bandeja del sistema, menú, notificaciones, single-instance.
- `src/autostart.rs` — auto-inicio vía clave `Run` de HKCU (sin `reg.exe`).
- `src/icon.rs` — icono embebido (ICO → `HICON`).
- `src/cli.rs` — modos `list`, `kill`, `tray`, `--version`.
- `docs/arquitectura.md` — decisión de diseño y plan.
