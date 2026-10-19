# Roadmap — GLORYPORT

**Contexto.** Herramienta de escritorio solo Windows para listar los puertos TCP en escucha y matar
el proceso dueño desde la bandeja del sistema. Rust nativo, un binario, sin runtime. Fuente de
decisión: `docs/arquitectura.md`.

## Siguiente bloque

- [ ] `19AA-1` — **Validar la v1 en uso real.** Correr `gloryport tray` durante varios días, matar
  puertos en proyectos diarios y registrar cualquier fricción (faltan PIDs, permisos,
  notificaciones).
- [ ] `128A-9` — **Declarar `rust-version` en `Cargo.toml`** (rápida). El código usa `is_none_or`,
  así que el MSRV real es 1.82+; la elipsis del PID ya quedó resuelta en `128A-10`.
- [ ] `128A-13` — **Fix IPv4 invertido en `list`** (rápida). `ports.rs` imprime el `u32` sin
  `u32::from_be` (muestra `1.0.0.127` en vez de `127.0.0.1`); el test alimenta el valor ya en BE y
  lo enmascara. Arreglar con `u32::from_be` y un test con el valor real de la API (`0x0100_007F`).
- [ ] `128A-5` — **Re-verificar puerto y PID justo antes de matar desde el menú** (rápida), para no
  matar un PID reutilizado entre la apertura del menú y el clic.
- [ ] `128A-14` — **Vía de salida por UI** (rápida): «Salir» en el menú contextual de la bandeja, y
  corregir los números del README (binario real 356.352 bytes; RAM ~17,5 MB WS / ~2,4 MB privada,
  1–2 hilos).

## Pendientes

- [ ] `128A-2` — **P5: puertos vigilados.** Avisar por notificación cuando un puerto configurado
  aparece o desaparece de la escucha, sin polling: solo al abrir el menú o en CLI `watch`. La config
  vive en `%APPDATA%\GLORYPORT`.
- [ ] `128A-3` — **P6: instalador ligero** (MSI/zip) y firma de código.
- [ ] `128A-6` — **Recuperación de instancia única si el mutex queda huérfano:** reintentar
  `CreateMutexW`/`FindWindowW` o reclamar cuando no exista ventana.
- [ ] `128A-8` — **Mitigación de iconos fantasma en la bandeja** tras cierres forzados del proceso:
  refrescar el icono al arrancar y/o limpiar al detectar ventana muerta.
- [ ] `128A-11` — **Observaciones del review de `128A-10`:** con scroll, indicar el total de filas
  («9 de 12» discreto en el pie) y distinguir en la CLI las causas de «desconocido» (proceso muerto
  vs. sin permiso/SYSTEM) con un enum en v2.

## Bloqueos

## Planes activos
