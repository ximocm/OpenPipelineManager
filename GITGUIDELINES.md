# Git Guidelines

This repository is public, so the default workflow should favor reviewable changes, repeatable checks, and a protected `main` branch.

## Branch Model

- `main` is the stable public branch. Do not commit or push directly to it.
- `dev` is the integration branch for active development.
- Feature and fix branches should branch from `dev` and use descriptive names such as `feat/pipeline-editor`, `fix/parser-cycle-check`, or `docs/release-process`.
- Normal flow: feature branch -> pull request into `dev` -> release pull request from `dev` into `main`.
- Hotfixes may branch from `main`, but they still need a pull request, passing checks, and a follow-up merge back into `dev`.
- The `Sync main back to dev` workflow opens a `main` -> `dev` sync pull request after `main` or `dev` changes when `dev` does not contain the latest `main` history. Merge these sync PRs with **Create a merge commit**; squash or rebase merges do not preserve the `main` history in `dev`. Configure the repository secret `OPM_AUTOMATION_TOKEN` with a fine-grained PAT that has `contents` read/write and `pull requests` read/write permissions if generated sync PRs should trigger the normal PR CI checks automatically.

## Required Checks

Run the relevant test suite before opening a pull request and again before merging if the branch changed.

Backend:

```bash
cd backend
pytest
```

Frontend:

```bash
cd frontend
npm run build
```

Every pull request must list the commands that were run. If a check cannot run, document the blocker clearly.

## Version And Changelog Policy

The current project version is tracked in `VERSION`. Frontend package releases must keep `frontend/package.json` and `frontend/package-lock.json` aligned with that version.

Update `CHANGELOG.md` for:

- public releases,
- user-visible behavior changes,
- API, pipeline schema, or persisted state format changes,
- dependency or security updates with user impact,
- migrations or compatibility notes.

Do not bump the version or duplicate changelog entries for every individual commit. During development, collect release notes under `Unreleased`; assign a version and date only when preparing a release.

Use semantic versioning:

- `MAJOR` for incompatible pipeline, API, or storage changes,
- `MINOR` for backward-compatible features,
- `PATCH` for backward-compatible fixes, documentation corrections, and security fixes.

## Commit Messages

Use concise imperative messages. Conventional Commit prefixes are preferred:

- `feat: add pipeline parser`
- `fix: validate missing dependency links`
- `test: cover execution ordering`
- `docs: document release workflow`
- `chore: update dependency metadata`

Keep commits focused. If a commit mixes unrelated backend, frontend, and documentation changes, split it before review when practical.

## Pull Requests

Each pull request should include:

- a short summary,
- the test commands run and their result,
- linked issues when applicable,
- version and changelog notes when required,
- screenshots or recordings for UI changes.

Merge only after CI is green and review comments are resolved.

## Recommended GitHub Protection

Configure `main` with a branch protection rule or repository ruleset:

- require pull requests before merging,
- require the CI checks `backend-tests` and `frontend-build`,
- require branches to be up to date before merging,
- require conversation resolution,
- require linear history,
- block force pushes,
- block branch deletion,
- do not allow bypassing the rule, including administrators, unless an emergency procedure is documented.

Configure `dev` with at least required CI checks and blocked force pushes/deletion. This keeps the integration branch usable while still protecting it from accidental history rewrites.

## Public Repository Security Baseline

- Keep GitHub Actions permissions minimal; workflows should default to read-only repository contents unless a job explicitly needs more.
- Enable Dependabot alerts, Dependabot security updates, secret scanning, and push protection where GitHub makes them available for the repository.
- Do not commit secrets, local runtime state, `.env` files, logs containing credentials, or machine-specific absolute paths.
- Treat imported pipelines as local code execution. Review validation output before running commands from untrusted sources.
- Keep dependency lockfiles committed when present.
