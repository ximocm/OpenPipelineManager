# README gallery — Open Pipeline Manager 0.3.1

These PNGs are captures of mapped native GTK windows on Linux/X11 with GTK
4.14.5 and the Cairo renderer. They have not been repainted or composited into
mockups. Window-manager decorations are excluded. The README's main images
select light or dark captures using GitHub's color preference.

| Image | View |
|---|---|
| [Workspace light](workspace-light.png) / [dark](workspace-dark.png) | Four completed steps, generated files, inspector and actual run output |
| [Start page light](welcome-light.png) / [dark](welcome-dark.png) | Pipeline and work-folder actions, recent work, version 0.3.1 |
| [Empty start page](welcome-empty.png) / [800×600](welcome-small.png) | Fresh launch without recent work |
| [Design](design-light.png) | Native library document, tool palette and connected graph |
| [Configuration](configure-light.png) / [command](configure-command-light.png) | Actual step dialog, typed inputs, required values and numeric bounds |
| [Editor dark](editor-dark.png) / [light](editor-light.png) | Markdown Visual Mode with notes in the demo project |
| [FASTA](fasta-dark.png) | Syntax highlighting of the actual filtered output |
| [Marketplace](marketplace-light.png) | Real public catalog fetched from `ximocm/opm-registry`, `dev` reference |

## Demo and provenance

The [capture fixture](../../scripts/fixtures/readme-pipeline.json) is an ordinary
JSON sharing-format pipeline. The capture harness opens it as a native library
document and creates isolated temporary Work folders. Its four steps prepare
six short FASTA sequences, retain sequences of at least eight bases, count the
remaining reads/bases, and write a report. It uses a POSIX shell, `awk`, `cat`
and `printf`; no bioinformatics program is required to run this demo.

The real output is asserted before images are saved:

```text
reads   bases
4       46
```

The notes file is authored by the harness from the documented demo results and
opened through the normal editor. The tool palette uses the repository's real
FastQC and Cutadapt descriptor fixtures, installed only in a temporary library.
Those programs are not executed. The Marketplace is an optional live network
capture; its content can change as the public registry changes.

## Reproduce

From the repository root, with a graphical desktop and the Rust/GTK build
prerequisites installed:

```sh
python3 scripts/capture-readme.py --output /tmp/opm-readme-captures --marketplace
```

Omit `--marketplace` for a fully offline capture. Use `--theme light` or
`--theme dark` for one theme. Choose a fresh output directory for each run:
the harness deliberately uses create-new library/work semantics.

The script sets isolated `XDG_CONFIG_HOME`, `OPM_PIPELINES_DIR`, `OPM_TOOLS_DIR`
and `OPM_QA_DIR` directories under `/tmp`. It does not use the user's existing
projects or library. Review images before replacing the checked-in gallery.

The ignored `readme_capture::readme_screenshots` GTK test also checks that the
start page does not save a phantom document, New text creates no extra blank
tab, the recent-work action opens the right project, and explicit file paths
bypass the start page. Both light and dark runs passed for this gallery.

Additional checks cover [large text](../qa/evidencias/START-large.png) and an
[unavailable recent work folder](../qa/evidencias/START-unavailable.png).
The [start-page QA record](../qa/start-page.md) lists verification results.
