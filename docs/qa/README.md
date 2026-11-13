# Suite de QA manual de Open Pipeline Manager

La [pantalla de inicio y galería 0.3.0](start-page.md) registra la nueva
navegación inicial, el trabajo reciente y las capturas reproducibles del README.

La [revisión visual del workspace GTK4](workspace.md) documenta el rediseño
0.4.1, sus capturas reales y los límites de la verificación de accesibilidad.

La [comprobación automatizada del Marketplace y el pincel](marketplace.md)
documenta el nuevo flujo de herramientas en OPM 0.4.0, con capturas de ventanas
aisladas. El alcance y el registro manual siguientes corresponden a OPM 0.3.0.

Alcance: aplicación de escritorio completa (versión 0.3.0, persistencia `pipeline-storage`), con énfasis en la biblioteca de pipelines, las carpetas de trabajo y la ejecución. Casos diseñados para localizar fallos por pantalla desde la interfaz, sin conocimiento del código.

Preparada el 2026-11-05 a partir de `MANUAL.md`, `docs/PIPELINE_STORAGE.md`, `DECISIONS.md`, las suites automatizadas de `core/` y `ui/`, y la revisión de superficies de `ui/src`. Verificada sobre el binario de producción en Linux/X11; los resultados esperados son criterios de QA, no resultados de una ejecución. El [registro de ejecución](registro-ejecucion.md) recoge la ejecución real.

## Preparación

No hay base de datos: el estado vive en la biblioteca de pipelines y en las carpetas de trabajo, ambas desechables para QA. Se conserva cualquier trabajo existente: los datos de QA usan el prefijo `qa-` y una biblioteca aislada por variable de entorno, sin tocar la biblioteca real del usuario.

| Requisito | Valor o preparación | Responsable | Disponibilidad |
| --- | --- | --- | --- |
| Entorno y versión | Linux con escritorio X11, binario `opm` 0.3.0 compilado en `target/release/opm`; shell POSIX (`/bin/sh`) y `cat` disponibles | QA | Disponible |
| Perfiles y permisos | Usuario local único (sin roles ni permisos); la app no pide credenciales | — | No aplica |
| Datos base | Biblioteca QA aislada: la app se lanza con `OPM_PIPELINES_DIR=/tmp/qa-opm-biblioteca` (se crea sola). Carpeta de trabajo QA: `/tmp/qa-opm-works/` (crearla antes o dejar que **New work** la cree) | QA | Disponible |
| Integraciones necesarias | Ninguna; los comandos de los ejemplos usan solo shell POSIX y `cat` | — | No aplica |

Prefijo de datos de esta ejecución: `qa-`. Registrar los nombres de pipeline y las carpetas de trabajo tal como aparecen en pantalla. Interfaz en inglés; zona horaria local; los timestamps de estado (`run.json`) son UTC ISO-8601.

### Datos y archivos

| Alias | Datos, estado inicial y resultado conocido | Cómo obtenerlo / caso previo | Incluido o pendiente |
| --- | --- | --- | --- |
| `ejemplo` | [`examples/first-run.json`](../examples/first-run.json): definición de 2 pasos (`create_message` → `write_report`); salida conocida del informe: `Hello from Open Pipeline Manager` y `mode=demo` | Está en el repositorio | Incluido |
| `biblioteca-qa` | `/tmp/qa-opm-biblioteca` con la variable `OPM_PIPELINES_DIR` | La crea la app al primer guardado | Disponible |
| `qa-trabajo-1` | Carpeta de trabajo creada por **New work** desde `qa-first-run`; estado inicial vacío | Caso TRB-01 | La crea la app |
| `qa-trabajo-2` | Segunda carpeta de trabajo independiente para el mismo pipeline | Caso TRB-04 | La crea la app |
| `qa-nota.txt` | Fichero de texto pequeño (p. ej. contenido `hola qa`) para casos de editor | Se crea desde el explorador (EXP-01) o con el botón de fichero nuevo | Disponible |
| `qa-imagen.png` | Cualquier PNG pequeño (p. ej. captura de pantalla) para EDI-03 | Proporcionado por QA | Pendiente: aportar un PNG |

Los casos de ejecución generan `outputs/`, `steps/` y `.pipeline-manager/` dentro de cada carpeta de trabajo; son desechables y se pueden borrar tras registrar la evidencia. Guardar en la biblioteca escribe ficheros `.pipeline` en `biblioteca-qa`; exportar escribe el fichero elegido en el destino que se indique (usar `/tmp/qa-opm-exports/`).

## Por dónde empezar

Recorrido inicial (dependencias reales): ARR-01 → BIB-01 → BIB-05 → TRB-01 → TRB-02 → TRB-09 → TRB-03 → TRB-04 → TRB-05. Con eso se cubren los cuatro flujos (diseñar, importar/exportar, ejecutar, reanudar) y el aislamiento.

Regresión después: BIB-02, BIB-03, BIB-04, BIB-06, BIB-07, TRB-06, TRB-07, TRB-08, PER-01, PER-02 y el bloque EDI/EXP. Los casos que cambian estado de un trabajo (TRB-02, TRB-05, TRB-06, TRB-07, TRB-09) se repiten sobre una copia nueva de la carpeta de trabajo o con **Update from library** de nuevo; no hay acción de retroceso, y `qa-trabajo-2` queda siempre como copia virgen para re-ejecuciones.

## Casos

### ARR-01 · Arranque sin sesión previa

**Prioridad:** P0. **Perfil:** usuario local.

**Antes:** cerrar la app; eliminar la sesión anterior (si existe) lanzando la app con una biblioteca vacía por primera vez; no hay carpetas de trabajo abiertas.

1. Lanzar la aplicación sin argumentos.
2. Observar la ventana única que se abre.

**Esperado:** se abre una ventana `Sin título 1 — OPM` con menús `File Project Edit Pipeline View Help`, sin explorador, sin lienzo de pipeline, sin panel de logs y con los comandos de trabajo deshabilitados o ausentes. No se crea ningún fichero en la carpeta de la sesión.

### ARR-02 · Reanudar la última sesión de trabajo

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** TRB-02 (un trabajo ejecutado).

**Antes:** con `qa-trabajo-1` abierto como ventana de trabajo y su ejecución terminada.

1. Cerrar la ventana de la aplicación por completo.
2. Volver a lanzar la aplicación sin argumentos.

**Esperado:** se reabre la ventana de trabajo de `qa-trabajo-1` con la definición, los valores y los estados de sus pasos tal como estaban (mismo paso `ok`/`pending` que antes de cerrar). La restauración caduca a la hora: retrasar el reloj o esperar más de 1 h y relanzar abre ventana standalone sin error.

### ARR-03 · Sesión inválida (trabajo borrado)

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** ARR-02.

**Antes:** la sesión apunta a `qa-trabajo-1`.

1. Borrar `qa-trabajo-1` completa desde un gestor de ficheros externo.
2. Lanzar la aplicación sin argumentos.

**Esperado:** se abre ventana standalone y se muestra un mensaje de error visible que indica que el trabajo no se pudo abrir; la sesión queda limpia (un segundo arranque posterior no repite el error).

### BIB-01 · Crear un pipeline y guardarlo en la biblioteca

**Prioridad:** P0. **Perfil:** usuario local. **Depende de:** ARR-01.

1. Menú `Project > New pipeline`.
2. Crear un paso con **Create step**: id `qa_paso`, nombre `Paso QA`, comando `echo hola > {salida}`, salida `salida` con path `salida.txt`.
3. `Project > Save pipeline` y escribir el nombre `qa-libre`.
4. Reabrir con `Project > Open pipeline` y elegir `qa-libre`.

**Esperado:** el título muestra `qa-libre.pipeline`; la barra de herramientas muestra `qa-libre.pipeline · <biblioteca-qa>`; en `biblioteca-qa` existe un único fichero `qa-libre.pipeline` (JSON con `format_version: 1`, nombre, pasos y layout). Reabrir restaura el paso con su comando y su salida sin arrastrar ninguna carpeta de trabajo.

### BIB-02 · Conflicto de nombre al guardar

**Prioridad:** P0. **Perfil:** usuario local. **Depende de:** BIB-01.

**Antes:** `qa-libre` existe en la biblioteca; un documento de diseño abierto con cambios sin guardar (p. ej. renombrar el paso a `Paso QA 2`).

1. `Project > Save pipeline` escribiendo `qa-libre`.
2. Observar el diálogo de decisión.
3. Repetir eligiendo cada rama: Sobrescribir, Guardar como (`qa-libre-2`) y Cancelar.

**Esperado:** nunca se sobrescribe en silencio: aparece una elección explícita (Sobrescribir / Guardar como otro nombre / Cancelar). Sobrescribir actualiza `qa-libre.pipeline`; Guardar como crea `qa-libre-2.pipeline` y deja `qa-libre` intacto; Cancelar no escribe nada y el documento sigue con cambios pendientes (● en el título).

### BIB-03 · Importar YAML o JSON a la biblioteca

**Prioridad:** P0. **Perfil:** usuario local. **Depende de:** BIB-01.

1. `Project > Import pipeline` y seleccionar `examples/first-run.json`.
2. Observar la biblioteca.
3. Importar de nuevo el mismo archivo.

**Esperado:** la importación crea un documento nativo con el nombre del archivo (p. ej. `first-run.pipeline`) en la biblioteca y abre la ventana de diseño; el archivo de origen no se modifica. La segunda importación con el mismo nombre pide decisión (no sobrescribe en silencio) tal como en BIB-02.

### BIB-04 · Exportar como YAML y JSON

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** BIB-01.

1. Con `qa-libre` abierto en diseño, `Project > Export pipeline`.
2. Guardar como `/tmp/qa-opm-exports/qa-libre.yml`.
3. Repetir guardando `/tmp/qa-opm-exports/qa-libre.json`.

**Esperado:** se crean los dos archivos en el destino elegido con los pasos del documento; el contenido es un solo documento de definición (sin valores, estado ni rutas de trabajo). Exportar no cambia la biblioteca ni el documento abierto.

### BIB-05 · Diseñar sin carpeta de trabajo

**Prioridad:** P0. **Perfil:** usuario local. **Depende de:** ARR-01.

1. Abrir `Project > Open pipeline` y elegir `qa-libre`.
2. Recorrer la ventana: barra de herramientas, lienzo, panel de paso.

**Esperado:** la ventana de diseño muestra lienzo con las tarjetas de pasos, panel de paso y contadores de validación; NO muestra explorador, ni panel de logs, ni botones Run/Stop. La barra de herramientas muestra el nombre del documento y la ubicación de la biblioteca. Los cambios muestran `●` en el título hasta que se guarda.

### BIB-06 · Edición estructural del grafo

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** BIB-05.

1. En diseño, crear un segundo paso `qa_suma` con comando `cat {entrada} > {salida2}`.
2. Conectar la salida del primer paso a la entrada del segundo (arrastrando el enlace de salida a entrada).
3. Arrastrar una tarjeta para cambiar el layout.
4. Desconectar la dependencia seleccionándola y pulsando Suprimir.
5. Guardar y reabrir.

**Esperado:** los cambios aparecen en el lienzo y marcan el documento como sin guardar; tras guardar y reabrir persisten pasos, comandos, enlaces y posición de las tarjetas. Eliminar la dependencia no borra el enlace de salida a entrada.

### BIB-07 · Validación en diseño

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** BIB-05.

1. Editar un paso y poner un comando con un placeholder sin clave, p. ej. `echo {no_existe}`.
2. Observar los contadores de validación.
3. Corregir el comando y volver a observar.

**Esperado:** el contador muestra 1 blocker con el mensaje `Placeholder has no input or parameter: no_existe` localizado en el paso; al corregir vuelve a 0 blockers / 0 warnings sin recargar la ventana.

### TRB-01 · Crear un trabajo nuevo desde la biblioteca

**Prioridad:** P0. **Perfil:** usuario local. **Depende de:** BIB-03 (o BIB-01 con `qa-libre`).

1. `Project > New work` y elegir el pipeline `first-run` (o `qa-libre`).
2. Elegir/crear la carpeta `/tmp/qa-opm-works/qa-trabajo-1`.
3. Observar la ventana de trabajo y el contenido de la carpeta.

**Esperado:** se abre una ventana de trabajo con explorador, lienzo de solo lectura, panel de paso con valores editables y panel de logs. La carpeta contiene `Input/`, `steps/`, `outputs/` y `.pipeline-manager/run.json` y NO contiene `project.config.json`, `config.txt`, `pipeline.project.json` ni ficheros `params/state/visual-layout/validation` (ver PER-01). `run.json` guarda la definición embebida con `format_version: 1`.

### TRB-02 · Ejecutar el pipeline

**Prioridad:** P0. **Perfil:** usuario local. **Depende de:** TRB-01.

1. Sin seleccionar pasos, pulsar `Run selected`.
2. Observar lienzo, panel de logs y carpeta de trabajo.

**Esperado:** los pasos se ejecutan en orden de dependencias (`create_message` y después `write_report`); cada tarjeta pasa por `running` a `ok`; el log de cada paso aparece en el panel; en la carpeta existen `Input/message.txt`, `outputs/report.txt` con `Hello from Open Pipeline Manager` y `mode=demo`, `.pipeline-manager/logs/*.log` y un `.done` por paso. `run.json` refleja los estados `ok` con código de salida 0.

### TRB-03 · Reabrir un trabajo existente

**Prioridad:** P0. **Perfil:** usuario local. **Depende de:** TRB-02.

1. Cerrar la ventana de `qa-trabajo-1`.
2. `Project > Open work` y elegir `qa-trabajo-1`.
3. Repetir tras renombrar o borrar el fichero de la biblioteca de origen.

**Esperado:** la ventana restaura definición, valores y estados desde `run.json` sin necesitar la biblioteca; los pasos siguen `ok` y no se re-ejecutan al pulsar `Run selected`. Abrir una carpeta sin `run.json` muestra un error claro que remite a **New work**.

### TRB-04 · Aislamiento entre trabajos

**Prioridad:** P0. **Perfil:** usuario local. **Depende de:** TRB-01.

1. Crear `qa-trabajo-2` con **New work** desde el mismo pipeline.
2. En `qa-trabajo-2`, cambiar el valor del parámetro `message` a `hola desde trabajo 2` y ejecutar.
3. Volver a `qa-trabajo-1` y abrir el paso con valores.

**Esperado:** `qa-trabajo-2` tiene su propio `outputs/report.txt` con `hola desde trabajo 2`; `qa-trabajo-1` conserva sus valores, sus estados y su informe sin cambios; comparten únicamente la definición de origen.

### TRB-05 · Adoptar una definición revisada

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** TRB-02, BIB-05.

1. En diseño (biblioteca), cambiar el comando de `write_report` (p. ej. añadir un comentario `# v2`) y guardar.
2. En la ventana de trabajo de `qa-trabajo-1`, `Project > Update from library`.
3. Observar el diálogo de resultado, los estados y los valores.

**Esperado:** aparece un resumen de la adopción (pasos cambiados/eliminados/iguales); el paso con comando cambiado pierde su `.done` y vuelve a `pending`; los pasos intactos conservan `ok`; los valores de claves aún declaradas se conservan y los de claves eliminadas desaparecen; la biblioteca no se reescribe. Tras re-ejecutar, el trabajo queda de nuevo `ok`.

### TRB-06 · Detener una ejecución

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** TRB-01.

1. Editar el valor de un paso para que tarde (p. ej. comando temporal `sleep 30` vía diseño + adopt, o un paso de prueba) y ejecutarlo.
2. Pulsar `Stop` mientras corre.

**Esperado:** el paso en curso se cancela con estado `Cancelled`, código de salida −15 y mensaje `Execution cancelled by user`; los pasos pendientes no arrancan; los pasos `ok` previos no cambian.

### TRB-07 · Paso con error y dependientes omitidos

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** TRB-01.

1. En diseño, cambiar el comando de `create_message` a `exit 3` (o equivalente) y adoptar en el trabajo.
2. Ejecutar sin selección.

**Esperado:** el paso queda `Error` con código 3 en su tarjeta y en `run.json`; el paso dependiente se omite con el motivo `Dependency is not complete: create_message`; nada se marca `ok` sin `.done`.

### TRB-08 · Trabajo movido de carpeta

**Prioridad:** P2. **Perfil:** usuario local. **Depende de:** TRB-02.

1. Cerrar la ventana del trabajo.
2. Mover `qa-trabajo-1` a otra ruta (p. ej. `/tmp/qa-opm-works/movido/`) desde un gestor de ficheros.
3. `Project > Open work` sobre la nueva ruta.

**Esperado:** el trabajo abre con la misma definición, valores y estados; no hay que editar ninguna ruta guardada.

### TRB-09 · Editar valores y persistencia

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** TRB-02.

1. En la ventana de trabajo, cambiar el valor `message` del paso `create_message` a `hola qa`.
2. Ejecutar de nuevo y abrir `outputs/report.txt`.
3. Cerrar y reabrir el trabajo.

**Esperado:** el informe contiene `hola qa`; el valor persiste tras reabrir (está en `run.json`, no en la biblioteca); la definición de la biblioteca conserva su valor por defecto original.

### EDI-01 · Ficheros en pestañas

**Prioridad:** P1. **Perfil:** usuario local.

1. Con un trabajo abierto, hacer doble clic en un `.txt`, un `.json` y un `.yml` del explorador.
2. Editar el `.txt` y pulsar `File > Save`.

**Esperado:** cada archivo abre en una pestaña con highlighting según su tipo; el marcador de cambios (`●`) aparece al editar y desaparece al guardar; el contenido en disco cambia.

### EDI-02 · Cerrar pestaña con cambios

**Prioridad:** P2. **Perfil:** usuario local. **Depende de:** EDI-01.

1. Editar una pestaña sin guardar.
2. Cerrar la pestaña y pulsar Cancelar en la confirmación.
3. Repetir y confirmar el cierre.

**Esperado:** Cancelar conserva la pestaña y sus cambios; confirmar cierra descartando (o guardando, según la opción elegida) y no deja el `●` colgado.

### EDI-03 · Imagen en página dedicada

**Prioridad:** P2. **Perfil:** usuario local.

1. Doble clic en un `.png` del explorador.

**Esperado:** la imagen abre en su página dedicada (no como texto) y puede cerrarse desde su pestaña.

### EXP-01 · Gestión de ficheros en el explorador

**Prioridad:** P1. **Perfil:** usuario local.

1. Crear la carpeta `qa-carpeta` y el fichero `qa-nota.txt` desde los botones del explorador.
2. Renombrar `qa-nota.txt` a `qa-nota-2.txt`.
3. Borrar `qa-carpeta` y confirmar.

**Esperado:** las tres operaciones se reflejan en el árbol y en disco; el borrado pide confirmación y Cancelar no borra; no se pueden crear/rrenombrar con nombres fuera de la carpeta del trabajo (rutas con `..` se rechazan con error claro).

### EXP-02 · Mover con arrastre

**Prioridad:** P2. **Perfil:** usuario local. **Depende de:** EXP-01.

1. Arrastrar `qa-nota-2.txt` a una carpeta del explorador.
2. Con el fichero abierto en una pestaña, moverlo otra vez.

**Esperado:** el fichero cambia de ubicación en el árbol; la pestaña abierta sigue apuntando al fichero movido (se re-objetiva) y sus cambios se guardan en la nueva ruta.

### PER-01 · Estructura de persistencia del trabajo

**Prioridad:** P0. **Perfil:** usuario local. **Depende de:** TRB-02.

1. Abrir `qa-trabajo-1` en un gestor de ficheros.
2. Listar el contenido de la carpeta y de `.pipeline-manager/`.

**Esperado:** solo existen `Input/`, `steps/`, `outputs/` y `.pipeline-manager/` con `run.json`, `logs/` y `done/`; no hay `project.config.json`, `config.txt`, `pipeline.project.json`, `params.json`, `state.json`, `visual-layout.json` ni `validation.json`. `run.json` contiene `format_version: 1`, la definición embebida, valores y estados.

### PER-02 · Rechazo de versiones de formato

**Prioridad:** P1. **Perfil:** usuario local. **Depende de:** TRB-01, BIB-01.

1. Copiar `qa-libre.pipeline` a `qa-malo.pipeline` y editar el JSON poniendo `"format_version": 999`.
2. Intentar abrirlo con `Project > Open pipeline`.
3. Repetir editando el `format_version` de la definición embebida dentro de un `run.json` y abriendo ese trabajo.

**Esperado:** en ambos casos aparece un error visible que indica la versión no soportada; el fichero editado queda intacto (no se reescribe ni se "repara"); la app sigue operativa para abrir otros documentos.

## Registro y criterios de salida

Anotar cada caso o variante en el [registro de ejecución](registro-ejecucion.md). Estados: Pendiente, Pasado, Fallido, Bloqueado o No aplica con justificación.

Criterios de salida: P0 pasados y cualquier fallo o bloqueo restante resuelto o con decisión registrada del responsable. Un caso Bloqueado no cuenta como Pasado. Las repeticiones conservan el fallo inicial y añaden la nueva evidencia.

## Fuentes

- [Manual de usuario](../../MANUAL.md) — pantallas y comportamientos documentados.
- [Diseño de persistencia](../PIPELINE_STORAGE.md) y [decisiones](../../DECISIONS.md) — biblioteca, `run.json` y semántica de adopción.
- Suites automatizadas `core/` y `ui/` — contratos de validación, ejecución y persistencia que sustentan los criterios.
