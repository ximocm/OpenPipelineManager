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
