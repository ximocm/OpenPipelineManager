# Open Pipeline Manager Manual

## 1. Overview

Open Pipeline Manager is a local application for creating, importing, editing, validating, and running pipelines. A pipeline is a list of steps. Each step can declare a command, input files or folders, output paths, configurable options, dependencies, and an execution environment.

The app is local-first: pipeline commands run on the machine where the backend is running. Treat imported pipelines as executable code.

## 2. Starting the Application

Start the backend:

```bash
cd backend
source .venv/bin/activate
uvicorn app.main:app --reload
```

Start the frontend:

```bash
cd frontend
npm run dev
```

Open the frontend URL shown by Vite, usually `http://localhost:5173`.

## 3. Projects

When the app opens, choose one of two actions:

- **Create**: choose a parent folder, enter a project name, and create a new project structure.
- **Open**: select an existing folder that contains `project.config.json`.

A project contains:

```text
ProjectFolder/
  Input/                 Uploaded or source input files
  steps/                 Per-step work folders
  outputs/               Shared outputs
  project.config.json    Project marker file
  config.txt             Editable notes/config
  pipeline.project.json  Current imported pipeline snapshot
  .pipeline-manager/     Runtime state, logs, validation, done markers
```

The left explorer supports creating files/folders, uploading files, renaming, deleting, and moving items by drag and drop. Double click a text, FASTA, YAML, JSON, shell, or image file to open it in a center tab.

## 4. Importing a Pipeline

Use **Import pipeline** and select a `.yml`, `.yaml`, or `.json` file. The import creates step folders and stores the imported configuration in `pipeline.project.json`.

Example:

```yaml
steps:
  - id: prepare_input
    name: Prepare input
    command: "bash scripts/prepare_input.sh"
```

If no dependencies are defined, the app connects steps in their file order.

## 5. Editing Steps

Select a step and use the edit button. You can configure:

- **Step id**: stable identifier used in dependencies.
- **Name**: display name.
- **Environment**: runtime wrapper, for example `conda:analysis`, `module:tools`, or `shell:source env.sh`.
- **Working directory**: directory where the command runs.
- **Command**: shell command with placeholders such as `{input_file}` or `{report_file}`.
- **Inputs**: required or optional values, usually files/folders.
- **Options**: text, number, boolean, or selector parameters.
- **Outputs**: expected output paths produced by the step.

## 6. Command Placeholders and Pipeline Variables

Placeholders in `command` must match an input key, option key, or output key:

```yaml
command: "cat {input_file} > {report_file}"
```

Commands can be multiline:

```yaml
command: |
  INPUT_FILE={input_file} \
  REPORT_FILE={report_file} \
  bash scripts/run.sh
```

In the step editor, each input, option, and output row has a **Pipeline var** button. It inserts an environment-style assignment into the command:

```text
INPUT_FILE={input_file}
METHOD={method}
REPORT_FILE={report_file}
```

Use this when scripts should receive values through environment variables.

## 7. Inputs, Outputs, and Previous Results

Inputs can be literal paths:

```yaml
inputs:
  - key: input_file
    type: file
    default: Input/data.csv
    required: true
```

Inputs can also come from a previous step output:

```yaml
inputs:
  - key: normalized_data
    type: file
    source_step: normalize_dataset
    source_output: normalized_data
```

Outputs should use stable keys:

```yaml
outputs:
  - key: normalized_data
    label: Normalized CSV
    path: work/normalized/data.csv
```

The canvas shows solid lines for pipeline order and dashed lines for output-to-input links.

## 8. Options and Selectors

Options are declared under `parameters`. Use `type: select` or `type: selector` for predefined choices:

```yaml
parameters:
  - key: method
    label: Method
    type: selector
    default: fast
    options:
      - fast
      - accurate
```

The backend normalizes `selector` to `select`.

## 9. Validation

Validation checks whether the pipeline is internally consistent before execution. It detects:

- duplicate step IDs,
- missing dependencies,
- dependency cycles,
- missing required inputs,
- file/folder inputs that do not exist,
- command placeholders without matching inputs/options/outputs,
- missing placeholder values,
- invalid numeric min/max values,
- selector values outside predefined options,
- invalid `source_step` or `source_output` references.

Blockers should be fixed before running. Warnings indicate incomplete or risky configuration.

## 10. Running Pipelines

Use **Run selected** to run selected steps. If no steps are selected, the app runs pending steps in dependency order. Use each step card or the right panel to run one step.

Execution state is stored in:

```text
.pipeline-manager/
  logs/
  done/
  state.json
  params.json
  validation.json
```

A step is considered `ok` only when its `.done` marker exists and all declared outputs exist.

## 11. Example Pipelines

- `examples/pipeline.yml`: small demo pipeline.
- `examples/pipeline.json`: JSON equivalent demo.
- `examples/generic-analysis.pipeline.yml`: generic four-step pipeline with explicit inputs, outputs, options, and previous-output links.

## 12. Troubleshooting

- If **Open** fails, make sure the folder contains `project.config.json`.
- If validation complains about a placeholder, add an input, option, or output with the same key.
- If a step finishes but is marked error, check whether every declared output path was created.
- If PNG/FASTA/text files do not open, double click them in the explorer to open a center tab.
- If execution hangs, inspect `.pipeline-manager/logs/<step-id>.log`.
