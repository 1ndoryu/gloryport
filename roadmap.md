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
- [ ] `128A-5` — **Re-verificar puerto y PID justo antes de matar desde el menú** (rápida), para no
  matar un PID reutilizado entre la apertura del menú y el clic.
## Pendientes

Ninguno.

## Bloqueos

## Planes activos
