# Architecture Decisions

This file records compatibility and behavior decisions that affect pipeline schemas or user-visible execution.

## 2026-11-12 — Native start page and explicit recent work

**Status:** Accepted; implemented for 0.3.0.

- Bare launch opens a GTK start page with the existing New/Open/Import pipeline,
  New/Open work, folder browsing and file-editing actions. It is a page in the
  native window stack, not a new project type or persisted document.
- The valid last work folder is offered for explicit reopening rather than
  reopening automatically. The existing `ui.ini` slot, one-hour TTL, validation
  and invalid-session cleanup are retained. Errors appear inline on the page.
- Command-line files and folders bypass the start page. Save on the start page
  does not open a save dialog or write an empty text file; New text reuses the
  initial blank tab. Existing pipeline/work storage formats are unchanged.

This is the requested launch UX change; it supersedes automatic restoration
for bare launches while preserving the stored recent-work data.

## 2026-11-05 — Independent registry tools, Marketplace, and Design brush

**Status:** Accepted

- Tools are independent schema-version-1 JSON descriptors maintained in the
  separate [OPM Registry](https://github.com/ximocm/opm-registry). OPM imports
  them into its per-user `opm/tools` library (`OPM_TOOLS_DIR` overrides it).
  No additional working-directory configuration is introduced.
- Marketplace resolves a public GitHub branch/tag to one immutable commit,
  validates downloaded definitions, and installs them only on an explicit
  action. Requests run off the GTK thread with HTTPS, timeouts and size limits.
  Local JSON import and installed tools work offline. Existing definitions
  require a visible replacement choice; importing never runs code or installs
  third-party programs.
- A Design-only brush expands selected optional groups and file-list counts
  into ordinary pipeline steps. Step IDs and default output paths are unique;
  placement uses world coordinates, and Escape restores selection. Work windows
  retain their existing structural edit restrictions.
- Placed steps embed commands/declarations and record descriptor version and
  provenance. Running saved pipelines therefore does not require the registry
  or tool library, and updating a tool never rewrites existing steps or work.
- Native pipeline import preserves its graph and layout, with the same explicit
  naming-conflict behavior as other imports. Pipeline/work envelope version 1
  remains unchanged because painted steps use the existing step schema.

See [Tools and Marketplace](docs/TOOLS.md) for the user flow.

## 2026-11-03 — pipeline-storage implementation: native library and `run.json`

**Status:** Accepted; implemented. Supersedes the "implementation pending"
status of the entry below and resolves its three open choices.

### Context

The pipeline library and storage proposal (`docs/PIPELINE_STORAGE.md`) fixed the
direction but left three choices open: the native document encoding, the
application library location, and the update/adopt semantics. Implementation
could not start without settling them.

### Decisions

- **Native encoding:** UTF-8 JSON under the `.pipeline` extension with a
  versioned envelope (`format_version: 1`). YAML and JSON remain import/export
  formats only, never companion files. Alternative considered: YAML as the
  native document — rejected to avoid encoding guessing and dual-file sync.
- **Library location:** `OPM_PIPELINES_DIR` when set (portable builds, tests),
  otherwise the per-OS data directory via the `directories` crate
  (`~/.local/share/opm/pipelines`, `%APPDATA%\opm\pipelines`,
  `~/Library/Application Support/opm/pipelines`), otherwise a temp fallback.
  Never derived from the launch working directory; the location is surfaced in
  the app. Alternative considered: executable-adjacent `pipelines/` — rejected
  because installed executables may live in read-only locations.
- **Update semantics:** adopting a revised library definition is explicit
  ("Update from library"); parameter/input values survive only for keys still
  declared, and a step whose definition fingerprint changed (command,
  environment, working directory, inputs, outputs, parameters, dependencies)
  loses its `.done` marker and returns to `pending`. Removed steps are pruned
  entirely. Saving work never rewrites the library document.

### Consequences

- Work folders carry their own definition snapshot in `.pipeline-manager/run.json`
  and remain openable if the library file moves, changes, or disappears.
- The project marker, `config.txt`, `pipeline.project.json`, and the separate
  `params`/`state`/`visual-layout`/`validation` runtime files are gone; a failed
  save leaves the previous complete document readable (atomic writes).
- Unsupported `format_version` values are rejected with a visible error and
  never clobber the file.

## 2026-11-03 — Pipeline library and consolidated work persistence

**Status:** Proposed; implementation pending. The application still uses the
current persistence format; its replacement will be a clean break.

### Context

Reusable pipeline design is currently tied to a project folder. Project metadata,
the imported pipeline snapshot, and runtime JSON files create several documents,
with parameters and state duplicated between them. Source definitions and manual
exports add more visible files without a clear single document to edit.

### Proposed direction

- Keep named reusable pipelines in the application's `pipelines/` library and
  separate Design from Execution. Designing must work without a work folder.
- Use one native `<name>.pipeline` document; retain YAML/JSON import and export.
- Use one `.pipeline-manager/run.json` support document per work folder for the
  execution snapshot, values, and state. Keep logs and completion artifacts separate.
- Stop generating `config.txt`, the separate project marker, and redundant
  snapshot/runtime configuration files once the replacement is implemented.
- Existing projects are disposable tests: no backward compatibility, legacy
  reader, or migration tooling is required. Replace the old test setup with one
  example created from scratch.
- Library edits must never silently change the definition used by work created
  with the new format.

The [storage proposal](docs/PIPELINE_STORAGE.md) records the current file roles,
target structure, implementation plan, acceptance criteria, and unresolved choices
(native encoding, installed application library location, and update semantics).
This entry records future work and does not change the current on-disk schema.
Once implemented, the clean break supersedes the project persistence
compatibility requirement in the 2026-10-27 stack migration decision below.

## 2026-08-20 — Optional output keys and unique placeholder names

**Status:** Accepted

### Context

Pipeline outputs can be used only for completion-state detection, without being substituted into a command. Existing pipelines and backend tests therefore allow an output to omit `key`. Inputs and parameters, by contrast, always need a key to expose their value to validation and command placeholders.

Allowing the same non-empty key in multiple inputs, parameters, or outputs is ambiguous because value precedence and source-output lookup can otherwise select different declarations.

### Decision

- Inputs and parameters must have a non-empty, non-whitespace key.
- Outputs may omit `key` when they are used only for output-state detection.
- An empty output key is treated as omitted because persisted models serialize an omitted key as `""`; a whitespace-only key is invalid.
- Every non-empty key must be unique across a step's inputs, parameters, and outputs.

### Consequences

- Existing unkeyed outputs remain compatible.
- Whitespace-only output keys and duplicate placeholder names produce visible validation blockers.
- Unkeyed outputs cannot be referenced as command placeholders by key, but they continue to participate in `.done` and output-existence checks.

## 2026-08-20 — Strict boolean values

**Status:** Accepted

### Context

Boolean inputs and parameters can arrive from imported YAML or JSON, persisted runtime parameters, and API requests. Coercing strings or numbers creates ambiguous behavior: for example, JavaScript treats the non-empty string `"false"` as truthy even though a user may intend it to mean `false`.

### Decision

- Boolean fields accept only native JSON/Python boolean values: `true` and `false`.
- Strings such as `"true"`, `"false"`, `"yes"`, and `"no"`, and numbers such as `0` and `1`, produce validation blockers.
- Validation does not silently normalize or rewrite submitted boolean values.

### Consequences

- Validation, persistence, frontend rendering, and command substitution agree on the value type.
- Hand-written pipelines with quoted boolean defaults must remove the quotes.
- API clients must send JSON boolean literals rather than string or numeric substitutes.

## 2026-08-20 — Close completed issues after integration into dev

**Status:** Accepted

### Context

GitHub only applies pull-request closing keywords automatically when changes reach the default branch, which is `main` in this repository. Feature work is integrated into `dev` first, so resolved issues otherwise remain open even after their implementation and CI checks have completed.

### Decision

- An issue is considered completed when its resolving pull request has passed the required checks and has been merged into `dev`.
- Resolved issues are closed manually with a comment linking the pull request that implemented them.
- Issues do not remain open solely because the corresponding changes have not yet been promoted from `dev` to `main`.

### Consequences

- The open issue list represents pending development work rather than pending releases.
- Release status must be tracked through branches, pull requests, versions, and the changelog instead of issue state.
- Pull requests targeting `dev` should reference their issue clearly even though GitHub will not close it automatically.

## 2026-08-20 — Reject implicit file overwrites and confirm destructive editor actions

**Status:** Accepted

### Context

Project uploads previously replaced existing files without warning. Closing dirty editor tabs or deleting paths containing them could also discard unsaved changes, and folder deletion did not make its recursive behavior explicit.

### Decision

- File uploads never overwrite an existing path unless a future API adds an explicit overwrite mode.
- Multi-file uploads check the whole batch for existing or duplicate destinations before writing any file.
- Closing a dirty file tab or deleting a path containing dirty tabs requires explicit confirmation before unsaved changes are discarded.
- Directory deletion always warns that the directory and all of its contents will be removed recursively.

### Consequences

- Upload conflicts return HTTP 409 and leave existing content unchanged.
- A conflicting upload batch is rejected before any of its files are written.
- Destructive editor actions require an additional user acknowledgement when unsaved work is affected.

## 2026-10-27 — Migrate OpenPipelineManager to the minmark stack (Rust + GTK4)

**Status:** Accepted (supersedes the stack implications of the FastAPI/React implementation)

### Context

OPM shipped as a Python FastAPI backend plus a React/Vite frontend. The minmark project established the reference desktop stack: a Rust `core/` workspace plus a GTK4 `ui/` package, a cloned editor subsystem, and a multi-OS CI/release pipeline. We want OPM on that same stack, reusing minmark's editor (cloned) and inheriting a similar deployment pipeline. The Python/React implementation stays the behavior ground truth for the port.

### Decision

- OPM becomes a native desktop application: `core/` workspace (`opm-document`, `opm-parser`, `opm-theme`, `opm-fs`, `opm-pipeline`, `opm-exec`, `opm-store`) plus a `ui/` GTK4 package (binary `opm`, application id `app.opm`), version 0.2.0 tracked in the Cargo manifests.
- The text editor is cloned and adapted from minmark's editor subsystem (`editor/`, `tabs/`, `menus/`, `explorer/`, `find.rs`) with per-file provenance headers. Markdown editing and Visual Mode are kept via a cloned parser; YAML/JSON/shell/FASTA syntax highlighting is new UI-owned lexers. minmark's HTML preview, PDF export, compositions, quick-open, project-wide search, and link/citation completion are not ported.
- The persistence schema is unchanged: `.pipeline-manager/*` and `pipeline.project.json` keep the Python field names (`type`, `""` output keys, `{x, y}` layout entries, `exit_code`, `last_run_at`). A project created by the Python app opens, saves, and reopens without losing data or changing shapes. Exported pipelines keep the same JSON schema (`by_alias` field names).
- Non-finite numeric values (NaN, ±inf): validation semantics are preserved exactly — rules 21/22/25 still produce blockers for such values arriving from YAML or constructed in memory. JSON files containing the non-standard `NaN`/`Infinity` tokens that Python's `json` module emitted now fail to parse with an error naming the file, instead of being silently accepted; OPM never writes those tokens.
- Windows pipeline execution requires a POSIX shell (for example Git for Windows' bash). When none is found the engine refuses with a clear error before starting anything. There is no `cmd.exe` fallback: the placeholder quoting guarantee (single-argv substitution, injection safety) is POSIX-only.
- The HTTP surface (REST endpoints, CSRF tokens, SSE streaming) is not ported; execution events flow over an in-process channel. Behavioral contracts are preserved: validation blockers prevent run preflight for the requested dependency closure, exactly as before.

### Consequences

- The pytest/npm quality gates are replaced by cargo gates plus `python3 scripts/test_release.py` (AGENTS.md and GITGUIDELINES.md updated accordingly).
- `examples/` pipelines remain valid inputs; the pipeline schema itself is unchanged.
- The FastAPI/React code is removed at cutover after behavioral parity verification; no compatibility shim remains.

## 2026-10-27 — Documented safety and execution limitations

**Status:** Accepted (documented limitations)

### Context

The security review of the ported core identified windows that remain after hardening the static-content cases (symlinked managed paths, hostile step ids, unsupported placeholder contexts). The remaining ones require a concurrent local actor mutating the project tree while an operation is in flight, or platform API gaps, or match the original execution semantics.

### Decision

- **Concurrent mutation is out of threat model.** OPM is a local-first, single-user desktop app: the project folder is trusted not to be mutated by another process during an operation. Hostile-but-static project content is in scope and rejected. The accepted windows are: ancestor directory swaps between an operation's validation and its use (e.g. import preflight → reservation) and raced-in destination files between existence checks and rename/move commits. Create uses exclusive creation; destinations present at validation time are never overwritten.
- **Batch rollback file identity is Unix-only.** Rollback deletes only files the batch created; on Unix this is enforced by file identity (dev/inode). On Windows the identity API is unavailable, so rollback verifies only that the entry is still a non-symlink regular file.
- **Termination targets the immediate child only.** `stop()` terminates the shell process running the step, matching the original `Popen.terminate()` semantics; descendant processes of that shell can survive. Process-group termination is future work.

### Consequences

- Shared project folders mutated concurrently by other users or processes during an operation are unsupported.
- A future hardening path for the concurrency windows is directory-handle-relative, no-follow filesystem operations (openat-style APIs).
