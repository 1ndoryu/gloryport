# GLORYPORT

Gestor minimalista de puertos TCP para **Windows 10/11 (64 bits)**, desde la bandeja del
sistema. Lista los puertos en escucha y termina el proceso que los ocupa en un clic.

Inspirado en [port-killer](https://github.com/productdevbook/port-killer), pero **desde
cero, solo Windows, un solo binario Rust nativo**: sin Electron, sin runtime externo, sin
procesos hijos (`netstat`, `taskkill`, `reg.exe`), sin timers en background.

![GLORYPORT](assets/gloryport.png)

## Características

- Bandeja del sistema con menú de puertos en escucha (puerto, proceso, PID), ordenado y
  deduplicado (IPv4/IPv6 del mismo proceso = una fila).
- Kill con confirmación implícita: un clic termina el proceso y muestra notificación con el
  resultado (éxito o error con motivo real, p. ej. acceso denegado).
- Auto-inicio opcional vía clave `Run` de HKCU (sin `reg.exe`).
- Instancia única: una segunda copia notifica a la primera y sale sola.
- CLI para scripting: `list` (tabla o JSON) y `kill <puerto>` con exit codes.
- Bajo consumo: sin refresco automático, ~0 % CPU en reposo, ~1,5–2,5 MB de RAM privada en
  bandeja (WorkingSet ~10–17 MB según el sistema), binario release de ~192 KB (0,19 MB).

## Requisitos

- Windows 10/11 64 bits.
- Rust 1.85+ (toolchain MSVC) solo para compilar desde fuente.

## Compilar

```powershell
cargo build --release
.\target\release\gloryport.exe --version
```

El binario es autocontenido (el icono va embebido); no requiere instalar nada más.

## Uso

### Bandeja (modo principal)

```powershell
gloryport          # o: gloryport tray
```

Aparece el icono en la bandeja. Un clic (o el menú) abre la lista de puertos; cada entrada
es `puerto  proceso (PID)`. El menú incluye:

- **Actualizar**: re-escanea la tabla TCP.
- **Auto-inicio**: activa/desactiva el arranque con Windows (HKCU).
- **Acerca de**: versión y stack.
- **Salir**: cierre limpio (elimina icono, ventana y mutex).

> Nota: al invocar la CLI desde PowerShell, el binario usa subsistema gráfico (sin ventana
> de consola al arrancar como app); si una salida se ve truncada por el host, usa `cmd /c`
> o redirige a archivo. En `cmd` y en llamadas programáticas la salida es completa.

### CLI

```powershell
gloryport list                # tabla: puerto, dirección, PID, proceso
gloryport list --json         # mismo resultado en JSON (útil para scripting)
gloryport kill 3000           # termina los procesos que escuchan en el puerto 3000
gloryport kill 3000 --pid 1234  # filtra por PID (si hay varios dueños)
gloryport --version
gloryport --help
```

Ejemplo:

```text
PUERTO  DIRECCIÓN             PID      PROCESO
135     0.0.0.0               1556     desconocido
445     0.0.0.0               4        desconocido
3000     127.0.0.1            1234     node
```

Exit codes: `0` éxito, `1` error de ejecución (p. ej. puerto libre o acceso denegado),
`2` error de uso de argumentos.

## Arquitectura y planificación

- [docs/arquitectura.md](docs/arquitectura.md) — decisiones de stack, módulos, flujos,
  seguridad, estrategia de recursos y Definition of Done.
- [roadmap.md](roadmap.md) — cola operativa y fases futuras.
- [AGENTS.md](AGENTS.md) — contrato del proyecto y gate de calidad.

## Calidad verificada (v1)

| Chequeo | Resultado |
|---|---|
| `cargo test` | 13/13 OK (10 unit + 3 E2E con puerto real) |
| `cargo clippy --all-targets -- -D warnings` | OK |
| `cargo fmt --check` | OK |
| Smoke bandeja | Ventana oculta creada, proceso vivo, segunda instancia sale sola |
| RAM en bandeja | WorkingSet ~10 MB / privada ~1,5 MB, 4 hilos |
| `gloryport list` (release) | ~44 ms, 26 filas en máquina de prueba |
| Binario release | 196.608 bytes (~192 KB) |

## Límites conocidos

- Solo TCP (IPv4 + IPv6) en escucha; no cubre UDP ni conexiones establecidas.
- Sin elevación: un proceso protegido (p. ej. SYSTEM) reporta el error real en lugar de
  pedir permisos de administrador.
- Los nombres de proceso no resolubles se muestran como "desconocido" (caché TTL 10 s).

## Hoja de ruta futura

Vigilancia de puertos configurados, instalador ligero/firma, línea de comando del proceso en
el tooltip — detalle y prioridad en [roadmap.md](roadmap.md).

## Licencia

MIT (ver `Cargo.toml`).
