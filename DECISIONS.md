# Architecture Decisions

This file records compatibility and behavior decisions that affect pipeline schemas or user-visible execution.

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
