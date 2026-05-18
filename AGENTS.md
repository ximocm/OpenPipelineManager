# Repository Guidelines

## Project Structure & Module Organization

This repository implements the MVP described in `Prompt.md`: a local-first visual pipeline manager with a FastAPI backend and React/Vite frontend.

- `backend/app/`: FastAPI application code.
- `backend/app/api/`: REST and SSE endpoints.
- `backend/app/models/`: Pydantic schemas for projects, pipeline steps, parameters, validation, and execution state.
- `backend/app/services/`: parsing, validation, command building, persistence, logs, and execution orchestration.
- `backend/tests/`: pytest coverage for parser, validation, state detection, and execution ordering.
- `frontend/src/`: React + TypeScript app.
- `frontend/src/features/`: feature folders such as `project-tree/`, `pipeline-canvas/`, `step-panel/`, and `logs-panel/`.
- `examples/`: sample `pipeline.yml` and `pipeline.json` files.

Keep generated runtime data in `.pipeline-manager/` inside user projects, not in source directories.

## Build, Test, and Development Commands

Backend setup and run:

```bash
cd backend
python -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
pytest
uvicorn app.main:app --reload
```

Frontend setup and run:

```bash
cd frontend
npm install
npm run dev
```

Use `pytest` for backend tests and the frontend package scripts for linting, formatting, and tests once `package.json` exists.

## Required Quality Gate

Always run the relevant tests before handing work back, opening a pull request, or merging. At minimum:

```bash
cd backend
pytest
```

```bash
cd frontend
npm run build
```

If a command cannot run because dependencies, credentials, or local services are missing, state the exact blocker and do not present the change as verified.

For public release, user-visible behavior, API/schema, dependency, or security changes, update the project version and `CHANGELOG.md` before merge. This is not required for every individual commit or internal fixup commit; it is required when the change should be visible in release notes.

## Coding Style & Naming Conventions

Use Python 3.12+ with type hints and Pydantic models for structured data. Prefer small service modules with explicit responsibilities. Use `snake_case` for Python files, functions, variables, and test names.

Use React with TypeScript for the frontend. Name components in `PascalCase`, hooks as `useThing`, and feature directories in kebab case. Keep API types in `frontend/src/types/` or next to the feature that owns them.

## Testing Guidelines

Required backend tests include YAML/JSON parsing, duplicate IDs, missing dependencies, cycles, placeholder validation, required inputs, parameter ranges, invalid select options, `.done` plus output state detection, and dependency-aware execution ordering.

Name backend tests `test_<behavior>.py`. Keep fixtures small and prefer example pipeline files under `examples/` when they are useful across tests.

## Commit & Pull Request Guidelines

No readable Git history is available in this checkout, so use concise imperative commits. Conventional Commit prefixes are preferred, for example `feat: add pipeline parser` or `test: cover cycle detection`.

Use `dev` as the integration branch. Do not commit or push directly to `main`; merge to `main` only through a reviewed pull request after CI passes. Pull requests should include a short summary, test commands run, linked issues when applicable, version/changelog notes when required, and screenshots or recordings for UI changes. See `GITGUIDELINES.md` for the full branch, release, and repository protection policy.

## Security & Configuration Tips

Treat pipeline commands as local user-controlled execution. Never hardcode project paths, assume a scripting language, or execute imported pipelines without visible validation. Keep secrets out of examples, logs, and committed config files.

For the public repository baseline, keep branch protection enabled on `main`, require CI before merge, disable force pushes and branch deletion for protected branches, and keep GitHub security features enabled where available.
