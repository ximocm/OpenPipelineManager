# Open Pipeline Manager Manual

## 1. Overview

Open Pipeline Manager is a local application for designing, importing, editing, validating, and running pipelines. A pipeline is a list of steps. Each step can declare a command, input files or folders, output paths, configurable options, dependencies, and an execution environment.

Pipelines are designed and kept in the application library as named `.pipeline` documents; they run in separate working folders that hold data, outputs, and execution state. The app is local-first: pipeline commands run on the local machine. Treat imported pipelines as executable code.

## 2. Starting the Application

Launch the desktop application:

```bash
cargo run --locked --manifest-path ui/Cargo.toml
```

or run a built binary directly, for example `./target/release/opm`. Per-OS
prerequisites are documented in `BUILDING.md`. On Windows, running pipelines
additionally requires a POSIX shell (Git for Windows provides bash).

Starting without arguments opens the start page. Choose **Open pipeline…**,
**New pipeline…** or **Import pipeline…** for reusable designs, or **Open work
folder…** / **New work…** for project execution. **Browse a folder…** opens the
file explorer without requiring a pipeline. File editing remains available
through **Open file…** and **New text file**.

Recent work is offered as a button for explicit reopening while the existing
one-hour session window is valid. A missing or invalid recent work folder is
reported on the start page. Supplying a file or folder on the command line
opens it directly. **Ctrl+W** closes the start window; **Ctrl+S** has nothing to
save there.

## 3. Pipelines, Library, and Work Folders

The application separates *designing* a pipeline from *running* it.

**Design.** Use **New pipeline** to start an empty pipeline or **Open pipeline**
to edit one already saved in the library. Designing works without any working
folder: edit the graph and declarations, then **Save pipeline** (or **Save as
pipeline** to duplicate) with a name. A pipeline document is stored as a single
native file in the application library:

```text
<library>/first-run.pipeline      One named pipeline: steps, declarations,
                                  defaults, and canvas layout (UTF-8 JSON)
```

The library location is shown in the app and resolves to `OPM_PIPELINES_DIR` if
set, otherwise the per-OS data directory (`~/.local/share/opm/pipelines` on
Linux, `%APPDATA%\opm\pipelines` on Windows, `~/Library/Application Support/opm/pipelines`
on macOS). Saving over an existing name always asks explicitly (overwrite,
save under another name, or cancel).

**Execution.** Use **New work** to pick a library pipeline and a working
directory, or **Open work** to resume a working directory that contains
`.pipeline-manager/run.json`. A work folder holds data and execution state only:

```text
WorkFolder/
  Input/                    Uploaded or source input files
  steps/                    Per-step work folders
  outputs/                  Shared outputs
  .pipeline-manager/
    run.json                Work configuration: definition snapshot, values,
                            and step execution state
    logs/                   Per-step logs
    done/                   Per-step completion markers
```

Each work folder embeds its own snapshot of the pipeline it was created from.
Editing the library never changes existing work; use **Update from library** in
a work window to explicitly adopt a revised definition (changed steps lose
their completion state, kept parameter values are reconciled). Work keeps
working even if the library file was moved or deleted.

The left explorer supports creating files/folders, uploading files, renaming, deleting, and moving items by drag and drop. Double click a text, FASTA, YAML, JSON, shell, or image file to open it in a center tab.

## 4. Importing and Exporting Pipelines

Use **Import pipeline** to copy a native `.pipeline` document or turn an external
`.yml`, `.yaml`, or `.json` definition into a named library document. Native imports
preserve their graph and layout. Imports never silently overwrite: if
the name already exists, the app asks. Use **Export pipeline** to share a
pipeline as YAML or JSON — exporting is an explicit action and does not create
a second configuration the app needs to operate.

Imported example:

```yaml
steps:
  - id: prepare_input
    name: Prepare input
    command: "bash scripts/prepare_input.sh"
```

If no dependencies are defined, the app connects steps in their file order.

## 5. Editing Steps

The workspace keeps tools or project files on the left, the pipeline canvas
in the center, and the step inspector on the right. Drag the dividers to
resize panes; the arrow in the Tools header collapses that pane. Small windows
and large text use scrollbars to keep controls reachable.

On the canvas, drag empty space to pan and use **Ctrl+scroll** or **+ / −**
to zoom. **Fit** (or **Home**) brings the pipeline into view without moving
its saved nodes. **Left / Right** selects steps and **Space** toggles the
selected step's run checkbox. **Alt+arrows** pans with the keyboard. The
information button beside Fit explains the controls and the three link types.
Each card has top, bottom, left and right connection ports. Drag from any port
to another step to add a dependency. Links choose facing ports from the nodes'
positions, supporting vertical and horizontal layouts without an orientation
setting. Select a dependency before dragging its destination port to reconnect
it. These gestures edit dependencies only in Design.
The selected step's command is selectable and wraps in the inspector.

Reusable tool definitions and ready-to-use pipelines are published in the
[OPM Registry](https://github.com/ximocm/opm-registry). Open **Pipeline >
Marketplace…** to browse and add definitions, or use **Import tool JSON…** to
install a local tool file. In Design, choose a tool from the **Tools** palette,
select optional arguments and input counts, then click empty canvas repeatedly
to place steps. Press **Esc** on the canvas or **Selection tool** to stop.

Each placed step has its own ID and outputs and is edited and saved like any
other step. Installing a definition does not install its external program or
change existing pipelines. See [Tools and Marketplace](docs/TOOLS.md) and the
[registry documentation](https://github.com/ximocm/opm-registry#readme) for
installation requirements, independent tool use, and example pipelines.

Structural editing belongs to the Design window: select a step and use the
edit button to create or change steps. A work window shows the same step
detail but only lets you change values (inputs and options); its graph is
read-only. You can configure in Design:

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

The canvas uses continuous lines for all three link types: thin gray lines with chevrons for pipeline order, teal lines with open arrowheads for output-to-input links, and purple lines with filled arrowheads for explicit dependencies (selectable; Delete removes the dependency).

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

The app normalizes `selector` to `select`.

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

The log header offers **Follow** to keep new output in view and **Wrap** to
read long lines. Turn Follow off while reading earlier output. These controls
affect the display only; log files and pipeline commands are unchanged.

Running happens in a work window. Use **Run selected** to run selected steps. If no steps are selected, the app runs pending steps in dependency order. Use each step card or the right panel to run one step. Input selections and option values are per work folder: the same library pipeline can run in several work folders without sharing values, logs, outputs, or completion state.

Work state is stored in:

```text
.pipeline-manager/
  run.json     Definition snapshot, values, and step states
  logs/
  done/
```

A step is considered `ok` only when its `.done` marker exists and all declared outputs exist. If the library pipeline changes later, existing work is unaffected until you explicitly run **Update from library**.

## 11. Example Pipelines

- [`examples/first-run.json`](examples/first-run.json): self-contained two-step
  pipeline that generates a message and writes a report, with configurable
  message and report mode, linked outputs, and explicit dependencies.
- [`Fresh work folder walkthrough`](examples/README.md): create an empty work
  folder, import the single definition, run it, and inspect the expected result.

## 12. Troubleshooting

- If **Open work** fails, make sure the folder contains `.pipeline-manager/run.json`; use **New work** to create one.
- If a `.pipeline` document fails to open, it may come from a newer app version (each document records its format version); the file is never rewritten behind an error.
- If validation complains about a placeholder, add an input, option, or output with the same key.
- If a step finishes but is marked error, check whether every declared output path was created.
- If PNG/FASTA/text files do not open, double click them in the explorer to open a center tab.
- If execution hangs, inspect `.pipeline-manager/logs/<step-id>.log`.
- On Windows, if running a step reports that no POSIX shell is available, install Git for Windows (bash) and reopen the app.
