# Changelog

All notable changes to Open Pipeline Manager are tracked here.

The format follows Keep a Changelog-style sections, and the project uses semantic versioning.

## Unreleased

### Changed

- Add a collapsible inspector with a header close button and View / Ctrl+Shift+I toggle. Selecting a card reopens it at its previous width, while background refresh leaves it hidden.
- Cards can move in both Design and Work, including during execution. Work positions save immediately to its embedded `run.json` copy without modifying the library or another work folder.
- Add a plain Toggle Logs / Console action to View, with Ctrl+J, matching Toggle Explorer without a checkbox or toolbar button. Hiding the pane retains its size and contents and does not stop execution or logging.
- Stop exposing an automatic untitled document. The first file reuses the internal blank buffer; explicit New and modified drafts remain separate. Closing the last file in Work returns to the pipeline.
- Move pipeline, document and image tabs into the center pane header, replacing the repeated Pipeline/Workspace title while preserving pane widths and split positions.
- Refine the file navigator with a compact folder header, native creation actions, symbolic chevrons/icons and a subtle active-document highlight. Show All Files, Show Hidden Files and Refresh Files now live in View; filters remain independent per window and survive refresh.

## 0.3.0 - 2026-11-12

### Added

- Native start page with Open/New/Import pipeline, Open/New work folder, folder browsing, file actions, and an explicit recent-work shortcut. Launching with a file or folder argument still opens it directly.
- A screenshot gallery and refreshed README, using real GTK windows and a reproducible four-step sequence-analysis demo. Added an isolated capture script for light and dark themes.
- Tools and Pipeline Marketplace for browsing the public OPM Registry, importing local tool JSON, installing native pipelines, and explicitly updating installed definitions. Installed tools remain available offline; importing a definition does not install its external program or execute it.
- Searchable, collapsible Tools palette and repeatable canvas brush. Each placed step embeds its command, declarations, unique ID, output paths, tool version and provenance. Optional arguments and input counts are configured before placement; Escape returns to selection.
- Native pipeline library and Design/Work flows: New/Open/Save/Save As, YAML/JSON import and export, New/Open work, and explicit Update from library with value reconciliation and completion-state invalidation for changed steps.
- Canvas keyboard selection, run check toggling, pan, zoom and Fit; four functional connection ports support both vertical and horizontal workflows.
- Log Follow and Wrap controls, accessible control names, inline field validation, and reproducible visual review sessions.

### Changed

- Refined the native GTK4 workspace with quieter panes, theme-aware cards and continuous links, distinct arrowheads, a grouped inspector and a scrollable step configuration dialog. Narrow windows and large text keep controls reachable through scrolling.
- Bare launch now presents the start page. Valid recent work is offered for explicit reopening using the existing one-hour session window, instead of opening automatically.
- Default step placement advances top-to-bottom; explicit saved layouts continue to support either orientation. Links choose facing ports from node positions without new orientation metadata.
- Replaced project persistence with named UTF-8 JSON `.pipeline` library documents and a single `.pipeline-manager/run.json` snapshot per work folder (`format_version: 1`). Runtime logs and completion artifacts remain separate. Library changes are adopted explicitly. **This is a clean break:** legacy project markers and separate params/state/layout files are not read or migrated.
- Replaced the old sample definitions with the self-contained `examples/first-run.json` and its fresh-work walkthrough.
- Updated GTK4 bindings to 0.11.5, pulldown-cmark to 0.13.6, and serde_yml to 0.0.13; added the ureq/Rustls HTTPS client for public registry browsing. Core and UI lockfiles remain aligned.
- Updated and pinned CI actions, grouped Dependabot updates, and documented the main-to-dev synchronization flow and automation token.

### Fixed

- File > Save and Ctrl+S dispatch by the visible page: Design saves its native pipeline; images, read-only Work pipeline views and the start page do not create empty text files.
- YAML deserialization retains pipeline aliases, quoted boolean values, non-finite numeric validation and clear invalid-document errors after dependency updates.
- Stale dependency selection no longer retains an active borrow during graph refresh; port hit testing chooses the nearest card where hit areas overlap.
- Cancellation test commands replace their shell process so unsupported descendants do not intermittently hold output pipes open in CI.

## 0.2.0 - 2026-11-02

### Changed

- Reimplemented Open Pipeline Manager on the minmark desktop stack: a Rust `core/` workspace (`opm-document`, `opm-parser`, `opm-theme`, `opm-fs`, `opm-pipeline`, `opm-exec`, `opm-store`) plus a GTK4 `ui/` package shipping the `opm` binary. Pipeline semantics, the persistence schema (`.pipeline-manager/*`, `pipeline.project.json`), and the opened-project layout are unchanged; projects created by earlier versions open, save, and reopen without data loss (see `DECISIONS.md`).
- The text editor is now minmark's editor, cloned and adapted: tabbed documents with dirty-state guards and close confirmations, find/replace, markdown editing with Visual Mode, and new syntax highlighting for YAML, JSON, shell, and FASTA files; image files open in a dedicated viewer.
- The canvas now renders three link types: ordered step links, output-to-input links, and editable dependency links.
- Windows pipeline execution requires a POSIX shell (Git for Windows provides bash); the engine refuses with a clear error before starting anything when none is found. Placeholder substitution keeps its single-argument, injection-safe quoting guarantees.
- Quality gates are now `cargo test`/`cargo build` for core and UI plus `python3 scripts/test_release.py`, replacing the pytest and npm checks.

### Added

- Multi-OS CI and release pipeline adapted from minmark: build/test/package matrix for Linux (x86_64), Windows (x86_64, ZIP+MSI), and macOS (arm64, tar.gz+dmg), checksummed artifacts, release-policy regression suite, and draft GitHub releases (`docs/CI.md`, `docs/PUBLISHING.md`).
- `BUILDING.md` with per-OS build prerequisites and packaging instructions.
- `NOTICE` and `third-party/minmark-Apache-2.0.txt` recording the Apache-2.0 provenance of the cloned minmark sources; every package stages them.
- Project explorer moves files and folders by drag and drop (with tab retargeting and dirty-buffer confirmations) and offers a Refresh action that also flags externally changed open files; generated outputs appear in the tree after each run.
- External file modifications resolve through a three-way dialog: reload from disk, overwrite, or cancel.

### Removed

- The HTTP surface (REST endpoints, CSRF tokens, SSE streaming) of the former FastAPI backend; execution events now flow over an in-process channel with the same behavioral contracts.

### Fixed

- Blocked execution requests when requested steps or dependencies have blocker validation issues, and shell-quoted placeholder values before running commands.
- Blocked unsafe pipeline working directories, inputs, and outputs from resolving outside the opened project, while keeping validation and execution aligned on their exact path values.
- Reconciled persisted step state after reloads, removed orphaned runtime data on pipeline imports, and kept source links valid when steps are renamed.
- Blocked ambiguous field keys, empty output paths, invalid input values, inverted numeric ranges, non-finite numbers, and non-boolean values before pipeline execution.
- Prevented uploads from overwriting existing project files, confirmed discarding dirty editor tabs, and made recursive folder deletion explicit.

## 0.1.1 - 2026-05-18

### Fixed

- Added CSRF protection to execution routes to prevent cross-origin form posts from triggering local pipeline command execution.

## 0.1.0 - 2026-05-18

### Added

- Initial MVP with FastAPI backend, React/Vite frontend, pipeline parsing, validation, execution state, project tree, visual canvas, step editor, and logs panel.
