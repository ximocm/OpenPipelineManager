# Changelog

All notable changes to Open Pipeline Manager are tracked here.

The format follows Keep a Changelog-style sections, and the project uses semantic versioning.

## Unreleased

### Changed

- Replaced the project persistence format with the pipeline-storage design: reusable pipelines are now named `<name>.pipeline` documents (UTF-8 JSON, `format_version: 1`) in the application library, and each working folder keeps one `.pipeline-manager/run.json` with the definition snapshot, values, and step state. Designing works without a work folder; work folders no longer contain `project.config.json`, `config.txt`, `pipeline.project.json`, or the separate `params`/`state`/`visual-layout`/`validation` runtime files. This is a clean break: old test projects are not read or migrated (see `docs/PIPELINE_STORAGE.md` and `DECISIONS.md`). The project version is now 0.3.0 for the next release.
- Replaced the three old sample pipeline definitions with one self-contained `examples/first-run.json` and a fresh-work walkthrough. Recorded that the planned storage replacement needs no backward compatibility or migration for disposable test projects; the application persistence format is not changed yet.
- Updated the desktop dependencies to GTK4 bindings 0.11.5, pulldown-cmark 0.13.6, and serde_yml 0.0.13, keeping the core and UI lockfiles aligned for reproducible builds. The project version is now 0.2.1 for the next release.
- Updated GitHub Actions checkout, cache, and artifact actions; pinned the sync workflow checkout to the same reviewed commit as CI and releases.
- Grouped Dependabot updates for shared Rust dependencies across core and UI, and grouped workflow action updates to avoid fragmented dependency pull requests.

### Added

- Library and work flows: New/Open pipeline with Save and Save As (naming conflicts always require an explicit choice), Import/Export as YAML/JSON sharing formats, New work/Open work from a library pipeline and a working directory, and an explicit Update from library action that reconciles values and invalidates completion state only for changed steps. The library location (`OPM_PIPELINES_DIR` or the per-OS data directory) is surfaced in the app.

### Fixed

- `File > Save` and `Ctrl+S` now dispatch by the visible page (like `Close Tab`): in a Design window the Pipeline page saves the design document to the library instead of silently routing to the active text tab and generating an `untitled*.txt`; images and the read-only Pipeline page of work windows have nothing to save.
- Adapted YAML model deserialization to the updated parser while preserving pipeline aliases, quoted boolean values, non-finite numeric validation, and invalid-document errors.
- Made cancellation test commands replace their shell process so CI does not intermittently wait on an unsupported descendant process holding an output pipe open.

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
