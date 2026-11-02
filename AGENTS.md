# Repository Guidelines

## Project Structure & Module Organization

This repository implements Open Pipeline Manager as a native desktop application: a Rust `core/` workspace plus a GTK4 `ui/` package with the `opm` binary, as decided in `DECISIONS.md`.

- `core/`: cargo workspace (resolver 2) with the `opm-document`, `opm-parser`, `opm-theme`, `opm-fs`, `opm-pipeline`, `opm-exec`, and `opm-store` crates: document model, Markdown parsing, theme, filesystem and project operations, pipeline model and validation rules, execution engine, and persistence.
- `ui/`: GTK4 package `opm-ui` with the `opm` binary: editor subsystem (editor, tabs, menus, explorer, find), pipeline canvas and step dialogs, runs/logs and validation panels, project dialogs.
- `scripts/`: release validation and packaging: `validate-release.py`, `test_release.py`, `package-linux.py`, `package-windows.py`, and `package-macos.py`.
- `.github/workflows/`: `ci.yml`, `release.yml`, and the `main` → `dev` sync workflow.
- `docs/`: `CI.md` and `PUBLISHING.md` for the release process.
- Root documents: `README.md`, `MANUAL.md`, `BUILDING.md`, `CHANGELOG.md`, `DECISIONS.md`, `AGENTS.md`, `GITGUIDELINES.md`, `LICENSE`, `NOTICE`, and `THIRD_PARTY_NOTICES.md`.
- `examples/`: sample `pipeline.yml` and `pipeline.json` files.

Keep generated runtime data in `.pipeline-manager/` inside user projects, not in source directories.

## Build, Test, and Development Commands

Run every command from the repository root. There is no cargo workspace at the root, so always pass `--manifest-path`.

Build the release binary:

```bash
cargo build --locked --release --manifest-path ui/Cargo.toml
```

Test and validate:

```bash
cargo test --locked --manifest-path core/Cargo.toml --workspace
cargo test --locked --manifest-path ui/Cargo.toml
python3 scripts/test_release.py
```

Run in development:

```bash
cargo run --locked --manifest-path ui/Cargo.toml
cargo run --locked --manifest-path ui/Cargo.toml -- path/to/project
```

Use the cargo test commands for the Rust crates and `python3 scripts/test_release.py` for release-tag and packaging rules. `BUILDING.md` documents prerequisites per operating system; `docs/CI.md` documents the CI matrix.

## Required Quality Gate

Always run the relevant tests before handing work back, opening a pull request, or merging. At minimum:

```bash
cargo test --locked --manifest-path core/Cargo.toml --workspace
cargo test --locked --manifest-path ui/Cargo.toml
cargo build --locked --release --manifest-path ui/Cargo.toml
python3 scripts/test_release.py
```

If a command cannot run because dependencies, credentials, or local services are missing, state the exact blocker and do not present the change as verified.

For public release, user-visible behavior, API/schema, dependency, or security changes, update the project version and `CHANGELOG.md` before merge. This is not required for every individual commit or internal fixup commit; it is required when the change should be visible in release notes.

## Coding Style & Naming Conventions

Use Rust (edition 2021) with the conventions already established in the workspace. Prefer small modules with explicit responsibilities. Use `snake_case` for files, functions, variables, and test names, and keep tests next to the code they cover.

Files cloned and adapted from minmark keep their provenance header; modifications stay under the Open Pipeline Manager contributors' copyright. Follow the GTK4 patterns already present in `ui/src` for widgets, dialogs, and event handling.

## Testing Guidelines

Required coverage includes YAML/JSON parsing, duplicate IDs, missing dependencies, cycles, placeholder validation, required inputs, parameter ranges, invalid select options, `.done` plus output state detection, dependency-aware execution ordering, execution quoting and injection safety, and project-tree safeguards. The ported test contracts live in the `core/` crates.

Name Rust tests after the behavior they cover. Keep fixtures small and prefer example pipeline files under `examples/` when they are useful across tests. Release-tag and packaging rules are covered by `python3 scripts/test_release.py`.

## Commit & Pull Request Guidelines

No readable Git history is available in this checkout, so use concise imperative commits. Conventional Commit prefixes are preferred, for example `feat: add pipeline parser` or `test: cover cycle detection`.

Use `dev` as the integration branch. Do not commit or push directly to `main`; merge to `main` only through a reviewed pull request after CI passes. Pull requests should include a short summary, test commands run, linked issues when applicable, version/changelog notes when required, and screenshots or recordings for UI changes. See `GITGUIDELINES.md` for the full branch, release, and repository protection policy.

## Security & Configuration Tips

Treat pipeline commands as local user-controlled execution. Never hardcode project paths, assume a scripting language, or execute imported pipelines without visible validation. Keep secrets out of examples, logs, and committed config files.

For the public repository baseline, keep branch protection enabled on `main`, require CI before merge, disable force pushes and branch deletion for protected branches, and keep GitHub security features enabled where available.
