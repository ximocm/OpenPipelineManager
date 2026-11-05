# Registro de QA manual de Open Pipeline Manager

Copiar para cada ejecución de la [suite manual](README.md). Añadir una fila por variante cuando un caso se repita con diferentes perfiles, datos o navegadores.

## Ejecución

| Campo | Valor |
| --- | --- |
| Identificador de ejecución / prefijo de datos QA | `qa-` / 2026-11-05-1 |
| Entorno y versión | Linux 7.0 (X11/XFCE), `opm` 0.3.0 (`target/release/opm`, commit `9248ec6`) |
| Fecha de inicio / fin | 2026-11-05 15:22 / 2026-11-05 16:10 (UTC+1) |
| Persona que ejecuta | QA asistido (automatización X11 + verificación en disco) |
| Navegador y versión / sistema / tamaño de ventana | Aplicación nativa GTK4; ventanas 1280x800 |
| Idioma y zona horaria relevantes | Interfaz en inglés (diálogos nativos en español); timestamps de `run.json` en UTC ISO-8601 |
| Perfiles disponibles, sin contraseñas | Usuario local único |
| Alcance seleccionado | Suite completa; ejecución parcial centrada en recorridos P0 (ver Cierre) |
| Archivos, datos de referencia e integraciones | `examples/first-run.json`; biblioteca QA `OPM_PIPELINES_DIR=/tmp/qa-opm-biblioteca`; trabajos en `/tmp/qa-opm-works/` |

Notas de la ejecución: la automatización X11 (xdotool) operó la interfaz real; algunos diálogos GTK no pintan en capturas por ID de ventana y se documentaron con captura de pantalla compuesta recortada. La sesión de escritorio era compartida durante la ejecución (ventanas del usuario visibles) y la app se cerró en un momento sin intervención del ejecutor (log vacío, sin crash); el relanzamiento validó ARR-02.

## Datos utilizados

| Alias de la suite | Código o nombre real en QA | Estado inicial | Caso que lo prepara |
| --- | --- | --- | --- |
| `ejemplo` | `examples/first-run.json` | Repositorio, sin modificar | Incluido |
| `biblioteca-qa` | `/tmp/qa-opm-biblioteca` (contiene `first-run.pipeline`) | Creada por BIB-03 | BIB-03 |
| `qa-trabajo-1` | `/tmp/qa-opm-works/qa-trabajo-1` | Ejecutado (2 pasos `ok`) | TRB-01, TRB-02 |
| `qa-trabajo-2` | No creado | — | TRB-04 (pendiente) |
| `qa-nota.txt` | No creado | — | EXP-01 (pendiente) |
| `qa-imagen.png` | No aportado | — | EDI-03 (pendiente) |

## Resultados

Estados: **Pendiente**, **Pasado**, **Fallido**, **Bloqueado** o **No aplica**. Evidencia en [`evidencias/`](evidencias/).

| Caso | Prioridad | Estado | Perfil / variante / resultado real / evidencia | Incidencia |
| --- | --- | --- | --- | --- |
| [ARR-01](README.md#arr-01--arranque-sin-sesión-previa) | P0 | Pasado | Usuario local. Lanzamiento sin argumentos: ventana `Sin título 1 — OPM`, menús completos, sin explorador/lienzo/logs. `evidencias/ARR-01.png` | — |
| [ARR-02](README.md#arr-02--reanudar-la-última-sesión-de-trabajo) | P1 | Pasado | Relanzado tras cierre completo: la app reabrió la ventana de trabajo `qa-trabajo-1` con estados `ok` intactos (`run.json` sin cambios). `evidencias/ARR-02.png`. Variante TTL>1h no ejecutada | — |
| ARR-03 | P1 | Pendiente | | |
| BIB-01 | P0 | Pendiente | Parcial observado: el guardado en biblioteca con nombre funciona vía BIB-02/BIB-03; el alta de paso por diálogo no se ejecutó | — |
| [BIB-02](README.md#bib-02--conflicto-de-nombre-al-guardar) | P0 | Pasado | Variante «diálogo + Cancelar»: al guardar con nombre existente aparece decisión explícita `Overwrite / Save as / Cancel` con mensaje «A pipeline named first-run already exists in the library…» (`evidencias/BIB-02.png`); Cancelar deja la biblioteca byte a byte intacta (mtime sin cambios). Variantes Sobrescribir y Guardar como: pendientes | — |
| [BIB-03](README.md#bib-03--importar-yaml-o-json-a-la-biblioteca) | P0 | Pasado | Import de `examples/first-run.json` vía diálogo: crea `first-run.pipeline` en la biblioteca aislada y abre ventana de diseño; el archivo fuente no se modifica. Variante conflicto en reimportación: pendiente | — |
| BIB-04 | P1 | Pendiente | | |
| [BIB-05](README.md#bib-05--diseñar-sin-carpeta-de-trabajo) | P0 | Pasado | Ventana de diseño sin work: lienzo con pasos, panel de paso, contadores 0 blockers/0 warnings, barra `first-run.pipeline · /tmp/qa-opm-biblioteca`, sin explorador ni logs ni Run/Stop. `evidencias/BIB-05.png` | — |
| BIB-06 | P1 | Pendiente | | |
| BIB-07 | P1 | Pendiente | | |
| [TRB-01](README.md#trb-01--crear-un-trabajo-nuevo-desde-la-biblioteca) | P0 | Pasado | Flujo completo por interfaz: `Project > New work` → picker de biblioteca (`first-run.pipeline`) → diálogo de carpeta → `/tmp/qa-opm-works/qa-trabajo-1`. Ventana de trabajo con explorador, lienzo de solo lectura, valores y logs (`evidencias/TRB-01.png`); carpeta con `Input/ steps/ outputs/ .pipeline-manager/{run.json,logs,done}` | — |
| [TRB-02](README.md#trb-02--ejecutar-el-pipeline) | P0 | Pasado | `Run selected` sin selección: `create_message` → `write_report` en orden; `outputs/report.txt` = `Hello from Open Pipeline Manager` + `mode=demo`; estados `ok`/`ok` con `exit_code 0`; `.done` por paso; badges `ok` en el lienzo (`evidencias/ARR-02.png`) | — |
| [TRB-03](README.md#trb-03--reabrir-un-trabajo-existente) | P0 | Pasado | Variante «reapertura»: restauración de definición/valores/estados desde `run.json` verificada (ARR-02 + ventana con badges `ok`, `run.json` intacto). Variantes «biblioteca de origen borrada» y «carpeta sin run.json»: pendientes | — |
| TRB-04 | P0 | Pendiente | Cubierto por el test automatizado `work_isolation` (core) | — |
| TRB-05 | P1 | Pendiente | Cubierto por el test automatizado `changed_library_adopt` (core) | — |
| TRB-06 | P1 | Pendiente | Cubierto por los tests de cancelación (core/exec) | — |
| TRB-07 | P1 | Pendiente | Cubierto por los tests de ejecución con error y dependientes omitidos (core/exec) | — |
| TRB-08 | P2 | Pendiente | Cubierto por el test automatizado `moved_work_folder` (core) | — |
| TRB-09 | P1 | Pendiente | El campo `Message` del panel de paso es editable por interfaz; la escritura del valor no llegó a completarse por un diálogo de navegación residual en la sesión | — |
| EDI-01 | P1 | Pendiente | | |
| EDI-02 | P2 | Pendiente | | |
| EDI-03 | P2 | Pendiente | Falta el archivo `qa-imagen.png` | — |
| EXP-01 | P1 | Pendiente | | |
| EXP-02 | P2 | Pendiente | Depende de EXP-01 | — |
| [PER-01](README.md#per-01--estructura-de-persistencia-del-trabajo) | P0 | Pasado | `qa-trabajo-1` contiene solo `Input/ steps/ outputs/ .pipeline-manager/{run.json,logs,done}`; `run.json` con `format_version: 1`, definición embebida, valores y estados; ningún fichero legacy | — |
| PER-02 | P1 | Pendiente | Cubierto por el test automatizado `unsupported_version_rejected` (core, incluye versión embebida) | — |

Observación de producto (no es incidencia de los casos): el campo `source.library_file` de `run.json` se guarda como `null` aunque exista la biblioteca; la adopción está diseñada para funcionar con `source.name` y selector de respaldo, pero conviene confirmar si la ruta debe poblarse como metadato informativo.

## Incidencia

Copiar una ficha por fallo. Usar datos de QA; no adjuntar credenciales ni datos reales innecesarios.

- **Caso y título del fallo:**
- **Versión, entorno, fecha y navegador:**
- **Perfil y códigos de registros QA:**
- **Datos previos y archivo utilizado:**
- **Pasos exactos por interfaz:**
- **Resultado esperado:**
- **Resultado observado y mensaje literal:**
- **Evidencia:**
- **Reproducibilidad e impacto:**
- **Ticket o incidencia:**
- **Repetición tras corrección: versión, fecha, resultado y evidencia:**

## Cierre

| Resultado | Cantidad o detalle |
| --- | --- |
| Casos o variantes ejecutados | 8 de 22 casos con evidencia (ARR-01, ARR-02, BIB-02 variante diálogo+cancelar, BIB-03, BIB-05, TRB-01, TRB-02, TRB-03 variante reapertura) + PER-01 = 9 |
| Pasados / fallidos | 9 pasados / 0 fallidos |
| Pendientes / bloqueados / no aplica y motivos | 13 pendientes: sin ejecutar por límite de automatización de diálogos GTK en sesión compartida; 6 de ellos cubiertos por tests automatizados (ver columna de resultados) |
| P0 sin pasar | BIB-01 y TRB-04 pendientes (TRB-04 cubierto por test `work_isolation`; BIB-01 requiere ejecución manual del alta de paso) |
| Incidencias abiertas | 0 (1 observación de producto: `source.library_file` nulo) |
| Decisión del responsable de la entrega | Pendiente: completar los P0 restantes (BIB-01, TRB-04) en una segunda ejecución con escritorio no compartido antes de dar la suite por cerrada |
| Datos que conservar para reproducir incidencias | `/tmp/qa-opm-biblioteca`, `/tmp/qa-opm-works/qa-trabajo-1` y las capturas de `evidencias/` |
