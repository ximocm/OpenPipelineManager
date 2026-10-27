<h1 align="center">Open Pipeline Manager</h1>

<p align="center">
  A local-first visual workbench for configuring, validating, and running declarative pipelines.
</p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-stable-000000">
  <img alt="GTK4" src="https://img.shields.io/badge/GTK4-native%20desktop-76548F">
  <img alt="License" src="https://img.shields.io/badge/License-Attribution%20Non--Commercial-blue">
</p>

Open Pipeline Manager helps you turn shell-based workflows into visible, editable pipeline projects. It is a native desktop application built on the [minmark](https://github.com/ximocm/minmark) stack: a Rust core workspace plus a GTK4 user interface that embeds minmark's editor (cloned and adapted). It provides a VS Code-style project explorer, a connected step canvas, structured inputs/options/outputs, validation, execution controls, and live logs while keeping all runtime state inside the opened project folder.

## Contents

- [Features](#features)
- [Quick Start](#quick-start)
- [How It Works](#how-it-works)
- [Pipeline Example](#pipeline-example)
- [Validation](#validation)
- [Project Structure](#project-structure)
- [Development](#development)
- [Documentation](#documentation)
- [Versioning](#versioning)
- [License](#license)

## Features

- Visual pipeline canvas with ordered step links, output-to-input links, and editable dependency links.
- Project explorer with create, upload, rename, delete, drag-and-drop move, a Refresh action, and file tabs, including safeguards against silent overwrites and unsaved-edit loss.
- Text editor cloned from minmark: tabbed documents with dirty-state guards, find/replace, markdown editing with Visual Mode, and syntax highlighting for YAML, JSON, shell, and FASTA files; image files open in a viewer.
- Step editor for command, environment, working directory, inputs, outputs, and options; dependencies are edited directly on the canvas.
- Pipeline variable insertion, for example `INPUT_FILE={input_file}`.
- Selector options with predefined values.
- Runtime validation before execution.
- Local command execution with per-step logs, status, and `.done` markers.
- Session restore for the last opened project.

## Quick Start

Prerequisites: a Rust stable toolchain and GTK 4.10 development files. See [BUILDING.md](BUILDING.md) for per-OS instructions (Linux, Windows, macOS).

```bash
cargo build --locked --release --manifest-path ui/Cargo.toml
./target/release/opm
```

For development builds:

```bash
cargo run --locked --manifest-path ui/Cargo.toml
```

## How It Works

1. Create a project or open an existing folder containing `project.config.json`.
2. Import a `pipeline.yml`, `pipeline.yaml`, or `pipeline.json`.
3. Edit steps on the canvas or in the right-side panel.
4. Define inputs, outputs, options, and previous-output links.
5. Fix validation blockers.
6. Run selected steps or run the pending pipeline.

Runtime files are written to the opened project under `.pipeline-manager/`, not to this source repository.

## Pipeline Example

```yaml
steps:
  - id: create_dataset
    name: Create demo dataset
    working_directory: "."
    command: |
      DEMO_DATA={demo_data} \
      bash -c 'mkdir -p "$(dirname "$DEMO_DATA")" && printf "sample,value\nalpha,3\n" > "$DEMO_DATA"'
    inputs: []
    outputs:
      - key: demo_data
        label: Demo CSV
        path: work/input/demo.csv

  - id: analyze_dataset
    name: Analyze dataset
    working_directory: "."
    command: |
      DEMO_DATA={demo_data} \
      REPORT_FILE={report_file} \
      METHOD={method} \
      bash -c 'mkdir -p "$(dirname "$REPORT_FILE")" && printf "method=%s\nrows=%s\n" "$METHOD" "$(wc -l < "$DEMO_DATA")" > "$REPORT_FILE"'
    inputs:
      - key: demo_data
        label: Demo CSV
        type: file
        source_step: create_dataset
        source_output: demo_data
        required: true
    outputs:
      - key: report_file
        label: Report
        path: results/report.txt
    parameters:
      - key: method
        label: Analysis method
        type: selector
        default: fast
        options:
          - fast
          - accurate
    dependencies:
      - create_dataset
```

Inputs can also consume previous outputs:

```yaml
inputs:
  - key: demo_data
    type: file
    source_step: create_dataset
    source_output: demo_data
```

See `examples/pipeline.yml`, `examples/pipeline.json`, and `examples/generic-analysis.pipeline.yml`.

## Validation

Validation checks for:

- duplicate step IDs,
- missing dependencies and dependency cycles,
- unsafe working directories, inputs, or outputs that resolve outside the opened project,
- empty or duplicate input, parameter, and keyed-output names,
- empty output paths,
- missing required inputs,
- file/folder inputs that do not exist,
- command placeholders without matching inputs/options/outputs,
- missing placeholder values,
- invalid or non-finite numeric values and inverted bounds,
- non-boolean values assigned to boolean fields,
- selector values outside predefined options,
- invalid `source_step` or `source_output` links.

Blockers should be fixed before running. Warnings indicate incomplete or risky configuration.

## Project Structure

Source layout (a Rust `core/` workspace plus a GTK4 `ui/` package, mirroring the minmark stack):

```text
core/
  opm-document/   text buffer, undo/redo, search model (cloned from minmark)
  opm-parser/     markdown parsing and highlighting (cloned from minmark)
  opm-theme/      color palette (vendored from minmark)
  opm-fs/         file store, atomic writes, project tree and file operations
  opm-pipeline/   pipeline model, YAML/JSON parsing, validation
  opm-exec/       execution engine with logs, statuses, and .done markers
  opm-store/      project persistence and runtime state
ui/
  src/            GTK4 application: editor, tabs, explorer, canvas, step panel,
                  run controls, logs (bin: opm)
scripts/          multi-OS packaging and release validation
docs/             CI, publishing, and architecture decision records
examples/         sample pipeline definitions
```

Opened projects use this structure:

```text
ProjectFolder/
  Input/
  steps/
  outputs/
  project.config.json
  config.txt
  pipeline.project.json
  .pipeline-manager/
    params.json
    state.json
    visual-layout.json
    validation.json
    logs/
    done/
```

## Development

Core tests:

```bash
cargo test --locked --manifest-path core/Cargo.toml --workspace
```

UI tests and build:

```bash
cargo test --locked --manifest-path ui/Cargo.toml
cargo build --locked --release --manifest-path ui/Cargo.toml
```

Release policy checks:

```bash
python3 scripts/test_release.py
```

## Documentation

- [Manual](MANUAL.md): full usage guide, pipeline authoring, validation, and troubleshooting.
- [Building](BUILDING.md): per-OS prerequisites, build, run, and local packaging.
- [CI](docs/CI.md): CI/release workflows and how to cut a release.
- [Publishing](docs/PUBLISHING.md): packaged artifacts and maintainer notes.
- [Examples](examples/): runnable YAML and JSON pipeline definitions.
- [Git Guidelines](GITGUIDELINES.md): branch, test, changelog, release, and repository protection policy.
- [Changelog](CHANGELOG.md): notable changes by version.
- [Security Policy](SECURITY.md): supported versions and vulnerability reporting guidance.

## Versioning

The project version is tracked in the Cargo manifests: `core/Cargo.toml` (`workspace.package.version`) and `ui/Cargo.toml` (`package.version`), which must match. User-visible releases should update both manifests and `CHANGELOG.md`; `scripts/validate-release.py` enforces the pairing against the release tag.

## Security

Pipeline commands execute locally. Only import and run pipeline files you trust. Do not commit secrets, `.env` files, local project runtime state, or machine-specific absolute paths.

## License

Open Pipeline Manager is distributed under the custom [Attribution Non-Commercial License](LICENSE). Non-commercial use is permitted with attribution. Commercial use requires explicit permission from the copyright holder.

Portions of the editor and core are cloned and adapted from [minmark](https://github.com/ximocm/minmark), © ximocm, licensed under Apache-2.0 (see [NOTICE](NOTICE) and `third-party/minmark-Apache-2.0.txt`). Third-party dependency notices are tracked in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
