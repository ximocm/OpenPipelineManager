# Marketplace y pincel: comprobación automatizada GTK

Fecha: 2026-11-05. Versión: OPM 0.4.0, Linux/X11, renderizado Cairo.
Esta comprobación complementa la [suite manual](README.md); no sustituye su
registro histórico ni presenta los casos manuales pendientes como ejecutados.

Se ejecutó `tests::marketplace_and_brush_graphical_smoke` con bibliotecas nuevas
en `/tmp`, mediante `OPM_TOOLS_DIR`, `OPM_PIPELINES_DIR` y `OPM_QA_DIR`. Solo se
capturaron las ventanas del proceso de prueba. El catálogo se leyó desde
`ximocm/opm-registry` en el commit
`9891ed39f54117335e14936c0221d4394ce583cb`.

| Comprobación | Resultado |
| --- | --- |
| Volver del editor al lienzo usando la configuración del pincel | Pasado; el lienzo recibe el foco. |
| Colocar dos pasos con identificadores y salidas independientes | Pasado; el diseño queda modificado sin guardar. |
| Espacio del lienzo con paleta y panel de detalle | Pasado; no reserva espacio para un explorador ausente. |
| Escape a través del controlador de teclado | Pasado; desactiva el pincel y evita nuevas colocaciones. |
| Catálogo real de GitHub | Pasado; siete definiciones disponibles. |
| Instalar Cutadapt single-end desde el catálogo | Pasado; guarda la procedencia y el commit y refresca la paleta. |
| Instalar la pipeline paired-end | Pasado; la identidad coincide con el nombre de biblioteca y el diseño abierto no cambia. |

La pipeline pintada es un borrador con archivos de entrada todavía sin asignar;
sus avisos de validación son esperados y no se ejecuta durante esta prueba. Las
pruebas de ejecución de core comprueban por separado un paso generado sin acceso
al registro y la protección de los valores frente a interpretación por el shell.

Evidencias:

- [MKT-01: Marketplace](evidencias/MKT-01.png).
- [PIN-01: paleta y pasos pintados](evidencias/PIN-01.png).

Para repetir, usa directorios de biblioteca nuevos bajo `/tmp`, una sesión GTK y
acceso público a GitHub, y ejecuta:

```bash
cargo test --locked --manifest-path ui/Cargo.toml \
  tests::marketplace_and_brush_graphical_smoke -- --ignored --exact --nocapture
```

La prueba exige las tres variables `OPM_*` anteriores y no utiliza las
bibliotecas normales del usuario.
