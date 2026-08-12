# Plan — GLORYPORT v1

Fecha: 2026-08-12 · Estado: **completado** · Dueño: agente Codex

## Objetivo

Crear desde cero, en la carpeta `GLORYPORT`, una herramienta solo Windows tipo port-killer:
bandeja del sistema, minimalista, bajo consumo, rápida y estable; con arquitectura planificada
y documentada en Markdown.

## Alcance

- Núcleo: escaneo de puertos TCP en escucha (Win32 directo), nombre del proceso por PID
  (caché TTL), terminación del proceso.
- UI: icono de bandeja con menú (puertos ordenados, actualizar, auto-inicio, acerca de, salir),
  notificaciones de resultado, single-instance, cierre limpio.
- CLI: `list` (tabla/JSON), `kill <puerto>`, `--version`; un solo binario para ambos modos.
- Docs: arquitectura, README, roadmap, plan, AGENTS.md del proyecto.

## No alcance

UDP, elevación, servicios Windows, favoritos, k8s/tunnels, ventana principal, timers
automáticos, otras plataformas.

## Dependencias

- `windows` crate 0.62 (features acotadas), `serde`/`serde_json` (salida JSON).
- Cargo/Rust 1.95 MSVC disponible en el equipo.

## Fases verificables

1. P0 Estructura + arquitectura + gate del proyecto → docs creados, `cargo check` OK.
2. P1 Núcleo + CLI → `list` muestra un puerto real ocupado por el helper.
3. P2 Bandeja → ventana oculta creada, menú operativo, single-instance.
4. P3 Calidad → fmt/clippy/tests verdes, E2E kill real, smoke tray.
5. P4 Cierre → roadmap/completados actualizados, commits por bloque, release build.

## Gate

`cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` + `cargo test` +
E2E funcional (helper ocupa puerto → `list` lo ve → `kill` lo libera) + smoke de bandeja.

## Definition of Done

Listado en `docs/arquitectura.md` §11. Criterio clave: bandeja funcional con kill real,
CLI funcional y gate verde con evidencia.

## Evidencia

- Commits en la rama `main` del repo GLORYPORT (uno por bloque).
- Resultados de tests y smoke en `Agente/completados/tareas-2026-08-12.md`.

## Pendientes reales tras el cierre

Validación de uso prolongado y fases P5/P6 (ver `roadmap.md`).
