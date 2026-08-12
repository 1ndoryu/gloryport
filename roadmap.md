# GLORYPORT — Roadmap

## Contexto

Herramienta de escritorio solo Windows para listar puertos TCP en escucha y matar el proceso
dueño desde la bandeja del sistema. Rust nativo, un binario, sin runtime. Fuente de decisión:
[docs/arquitectura.md](docs/arquitectura.md).

## Siguiente bloque ejecutable

- **Validar v1 en uso real**: correr `gloryport tray` durante varios días, matar puertos en
  proyectos diarios y registrar cualquier fricción (faltan PIDs, permisos, notificaciones).
- **`128A-9` (rápida)** — declarar `rust-version` en `Cargo.toml` (el código usa
  `is_none_or`, MSRV real 1.82+; la elipsis del PID ya quedó resuelta en 128A-10).
- **P5 — Puertos vigilados**: avisar por notificación cuando un puerto configurado aparece o
  desaparece de la escucha (sin polling: solo al abrir el menú o en CLI `watch`).

## Tareas pendientes (por prioridad)

1. `128A-2` — P5: vigilancia de puertos configurados (config en `%APPDATA%\GLORYPORT`).
2. `128A-3` — P6: instalador ligero (MSI/zip) y firma de código.
3. `128A-4` — Mostrar línea de comando del proceso en el tooltip del menú (PID → cmdline).
4. `128A-5` — Re-verificar puerto/PID justo antes de matar desde el menú (evitar matar un
   PID reutilizado entre apertura del menú y clic).
5. `128A-6` — Recuperación de instancia única si el mutex queda huérfano (reintentar
   `CreateMutexW`/`FindWindowW` o reclamar cuando no exista ventana).
6. `128A-8` — Mitigación de iconos fantasma en la bandeja tras cierres forzados del
   proceso (refresh del icono al arrancar y/o limpieza al detectar ventana muerta).
7. `128A-11` — Observaciones del review de 128A-10: con scroll, indicar el total de filas
   ("9 de 12" discreto en el pie) y distinguir en CLI las causas de "desconocido" (proceso
   muerto vs. sin permiso/SYSTEM) con un enum en v2.

## Planes activos

- [Plan GLORYPORT v1](Agente/planes/completados/plan-gloryport-2026-08-12.md) — cerrado
  (evidencia en `Agente/completados/tareas-2026-08-12.md`).
