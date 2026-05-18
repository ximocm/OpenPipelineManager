<h1 align="center">Open Pipeline Manager</h1>

<p align="center">
  A local-first visual workbench for configuring, validating, and running declarative pipelines.
</p>

<p align="center">
  <img alt="Python" src="https://img.shields.io/badge/Python-3.12%2B-3776AB">
  <img alt="FastAPI" src="https://img.shields.io/badge/FastAPI-backend-009688">
  <img alt="React" src="https://img.shields.io/badge/React-18-61DAFB">
  <img alt="TypeScript" src="https://img.shields.io/badge/TypeScript-5-3178C6">
  <img alt="License" src="https://img.shields.io/badge/License-Attribution%20Non--Commercial-blue">
</p>

Open Pipeline Manager helps you turn shell-based workflows into visible, editable pipeline projects. It provides a VS Code-style project explorer, a connected step canvas, structured inputs/options/outputs, validation, execution controls, and live logs while keeping all runtime state inside the opened project folder.

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

- Visual pipeline canvas with ordered step links and output-to-input links.
- Project explorer with create, upload, rename, delete, move, and file tabs.
- Text, FASTA, YAML, JSON, shell, and image viewing/editing where applicable.
- Step editor for command, environment, working directory, inputs, outputs, options, and dependencies.
- Pipeline variable insertion, for example `INPUT_FILE={input_file}`.
- Selector options with predefined values.
- Runtime validation before execution.
- Local command execution with per-step logs, status, and `.done` markers.
- Session restore for the last opened project.

## Quick Start

Start the backend:

```bash
cd backend
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
uvicorn app.main:app --reload
```

Start the frontend:

```bash
cd frontend
npm install
npm run dev
```

Open the Vite URL shown in the terminal, usually `http://localhost:5173`.

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
- missing required inputs,
- file/folder inputs that do not exist,
- command placeholders without matching inputs/options/outputs,
- missing placeholder values,
- invalid numeric bounds,
- selector values outside predefined options,
- invalid `source_step` or `source_output` links.

Blockers should be fixed before running. Warnings indicate incomplete or risky configuration.

## Project Structure

```text
backend/
  app/
    api/          FastAPI routes
    models/       Pydantic schemas
    services/     parsing, validation, storage, execution, file tree
  tests/          pytest suite
frontend/
  src/
    api/          HTTP client
    features/     project tree, canvas, step panel, logs panel
    types/        shared TypeScript types
examples/         sample pipeline definitions
MANUAL.md         full user and developer manual
THIRD_PARTY_NOTICES.md direct dependency attribution checklist
LICENSE           custom attribution non-commercial license
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

Backend checks:

```bash
cd backend
source .venv/bin/activate
pytest
```

Frontend checks:

```bash
cd frontend
npm run build
```

## Documentation

- [Manual](MANUAL.md): full usage guide, pipeline authoring, validation, and troubleshooting.
- [Examples](examples/): runnable YAML and JSON pipeline definitions.
- [Git Guidelines](GITGUIDELINES.md): branch, test, changelog, release, and repository protection policy.
- [Changelog](CHANGELOG.md): notable changes by version.
- [Security Policy](SECURITY.md): supported versions and vulnerability reporting guidance.

## Versioning

The current project version is tracked in [VERSION](VERSION). User-visible releases should update `VERSION`, `CHANGELOG.md`, and package manifests such as `frontend/package.json` when applicable.

## Security

Pipeline commands execute locally through the backend. Only import and run pipeline files you trust. Do not commit secrets, `.env` files, local project runtime state, or machine-specific absolute paths.

## License

Open Pipeline Manager is distributed under the custom [Attribution Non-Commercial License](LICENSE). Non-commercial use is permitted with attribution. Commercial use requires explicit permission from the copyright holder.

Third-party dependency notices are tracked in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
