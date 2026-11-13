# Proposed pipeline library and storage simplification

Recorded: 2026-11-03. **Status: implemented.** This document records the
requested direction and the storage design now in the application. See the
"pipeline-storage implementation" entry in `DECISIONS.md` for the settled
encoding, library location, and update semantics.

Scope clarification: the current projects are disposable tests. The replacement
does not need backward compatibility, a legacy reader, or migration tooling.
Replace the test examples with one example built from scratch.

## Goal

Keep reusable pipelines in the application's `pipelines/` directory, named by
the user. Keep input data, working files, outputs, and execution state in a
separate working directory. Designing a pipeline must not require creating a
working project first.

The target is one pipeline document and, for each working directory, one
support document. Users should not have to coordinate several configuration
files or keep an imported file and its export synchronized.

## Why the current files exist

| Current file | Current purpose | Proposed replacement |
| --- | --- | --- |
| `config.txt` | Generated explanatory text about project folders; not executable configuration. | Stop generating it; show this information in the app/manual. |
| `project.config.json` | Project marker, name, format version, creation time, and folder conventions. Opening a project currently requires it. | Remove the separate marker requirement; keep necessary work metadata in `run.json`. |
| `pipeline.project.json` | Snapshot containing the pipeline, parameters, step state, canvas layout, validation, and project path. | Reusable definition/layout in `.pipeline`; work-specific snapshot, values, and state in `run.json`. |
| `.pipeline-manager/params.json`, `state.json`, `visual-layout.json`, `validation.json` | Additional persisted copies of runtime/editor data also present in the project snapshot. | Consolidate runtime data in `run.json`, put reusable layout in `.pipeline`, and recalculate validation. |
| `rnaseq-demo.pipeline.yml`, `tour-export` | User-reported disposable test files, not present in this checkout. A `.pipeline.yml` suffix is currently treated as YAML. | Discard the old test setup when available and use the new example; no conversion is required. |

Logs, step completion markers, input files, scripts, and outputs are execution
artifacts or resources, not additional configuration documents. Global UI
preferences such as window/explorer state remain application preferences and
do not belong in individual pipeline or work documents.

## Proposed documents

```text
OpenPipelineManager/
  pipelines/
    first-run.pipeline

chosen-working-directory/
  Input/
  steps/
  outputs/
  .pipeline-manager/
    run.json
    logs/
    done/
```

### Library: `pipelines/<name>.pipeline`

- One canonical, editable document per reusable pipeline: format version,
  pipeline name, steps, commands, dependencies, input/parameter declarations,
  defaults, outputs, and canvas layout. Optional pipeline notes belong here too.
- Proposed encoding: UTF-8 JSON under the `.pipeline` extension. Using a single
  native encoding avoids needing a companion YAML/JSON file or guessing the
  encoding from content. The extension and envelope require parser support;
  renaming a current export alone does not implement the new format.
- YAML (`.yaml`/`.yml`) and JSON (`.json`) remain import/export formats. Import
  creates a named library document; export is an explicit sharing action.
  Neither creates a second document that the app needs to operate.
- Exclude work-specific input selections, current parameter overrides, status,
  logs, completion markers, and machine-specific project paths. Resolve relative
  execution paths against the selected working directory.
- Preserve the existing command and placeholder safety rules. A definition file
  alone does not bundle referenced scripts or install its required tools.

### Working directory: `.pipeline-manager/run.json`

- One support document: format version, necessary work metadata, source pipeline
  identity, the exact definition/layout selected for this work, input/parameter
  overrides, and step execution state.
- The embedded definition is the work's execution snapshot, not a second
  editable library pipeline. Saving work must never rewrite the library file.
- Editing the library must not silently change existing work. Explicitly
  adopting a revised definition must reconcile parameters and invalidate affected
  completion state before execution can resume.
- Reopening work must remain possible if the original library file was moved,
  renamed, edited, or deleted. The working directory's actual location determines
  its root; moving it must not require editing stored absolute project paths.
- Recalculate validation from the snapshot, effective values, and current files;
  do not persist a separate validation document.
- Keep logs and the existing `.done` plus declared-output completion checks.
  Consolidating JSON configuration does not change execution success semantics.
- Save atomically; a failed save must leave the previous complete document
  readable. Reject unsupported schema versions with a visible error.

## User flow

1. **Design:** New/Open pipeline, edit the graph and declarations, then Save with
   a name in the application library. Save As duplicates deliberately and naming
   conflicts require an explicit choice; imports never silently overwrite.
2. **Execution:** Choose a library pipeline and a working directory, select
   input data and parameter values, review validation, then run.
3. **Resume work:** Open a working directory containing `run.json`, restoring
   its definition snapshot, values, and execution state without requiring a
   separate project marker or an available library file.
4. Structural edits belong to Design and are unavailable during execution.
   Library changes affect existing work only through the explicit update action.

## Implementation plan (clean break)

1. Define versioned pipeline/work envelopes and their serializers. Resolve the
   remaining location/encoding choices below before changing persistence.
2. Add the application library and standalone Design workflow; import external
   YAML/JSON definitions and save native `.pipeline` documents.
3. Consolidate work persistence into `run.json` and remove the new-project
   requirement for `project.config.json`. Stop generating `config.txt` and the
   redundant JSON documents for new work.
4. Replace the old project persistence directly. Do not implement legacy-file
   detection, conversion, precedence merging, compatibility flags, or dual
   writes. Old test projects may be discarded and recreated; opening an old
   project must not silently restore its obsolete runtime configuration.
5. Replace the repository's old sample definitions with a single self-contained
   example and demonstrate a fresh work folder. Do not ship generated runtime
   files, output artifacts, multiple equivalent exports, or old project metadata.
6. Update file dialogs, session restore, examples, manual, version, and changelog
   when shipping the behavior. Add round-trip, isolation between work folders,
   interrupted-write, naming-conflict, moved-work, and changed-library
   coverage; retain the existing validation/execution/path safeguards.

The three old definitions under `examples/` have been replaced with
[`examples/first-run.json`](../examples/first-run.json), written from scratch.
It is an importable example for the current application, not an implementation
of the future native format. Its execution is checked in fresh temporary work
folders; the `.pipeline`/`run.json` workflows remain pending.

## Choices still to settle

All three choices are settled by the implementation (see `DECISIONS.md`):

- **Native encoding:** confirmed UTF-8 JSON under `.pipeline` with
  `format_version: 1`; YAML remains an alternative import/export format for
  that single document, not a mandatory companion file.
- **Library location:** `OPM_PIPELINES_DIR` when set (portable builds and
  tests), otherwise the per-OS data directory (`directories` crate:
  `~/.local/share/opm/pipelines`, `%APPDATA%\opm\pipelines`,
  `~/Library/Application Support/opm/pipelines`), otherwise a temp fallback.
  Never derived from the launch working directory; the app surfaces the
  resolved location in the pipeline picker and the Design toolbar.
- **Envelopes and update workflow:** `.pipeline` carries format version, name,
  description, steps, and canvas layout; `run.json` carries the embedded
  definition snapshot, the source identity (informational only), per-step
  values, and per-step state. Adoption is explicit ("Update from library"):
  values reconcile to still-declared keys, steps whose fingerprint changed lose
  completion state and return to pending, removed steps are pruned.

## Completion criteria

- A pipeline can be created, named, saved, and reopened without a work folder.
- The same library document can run in two work folders without sharing values,
  logs, outputs, or completion state.
- New work requires only `run.json` as configuration; no generated `config.txt`,
  `project.config.json`, `pipeline.project.json`, or separate runtime JSON files.
- Import/export does not create a second mandatory pipeline configuration.
- The new example runs from a fresh work folder; legacy project support and
  migration are explicitly out of scope.
- Edits to a library pipeline cannot silently alter work created in the new
  format when that work is resumed.
