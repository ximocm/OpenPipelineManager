# Changelog

All notable changes to Open Pipeline Manager are tracked here.

The format follows Keep a Changelog-style sections, and the project uses semantic versioning.

## Unreleased

### Changed

- Updated backend, frontend, and GitHub Actions dependencies to their latest compatible releases.

### Added

- Added repository governance documentation for tests, branching, release notes, and public security defaults.
- Added CI configuration for backend tests and frontend builds.
- Added Dependabot and security reporting guidance.

### Fixed

- Blocked execution requests when requested steps or dependencies have blocker validation issues, and shell-quoted placeholder values before running commands.
- Blocked unsafe pipeline working directories, inputs, and outputs from resolving outside the opened project, while keeping validation and execution aligned on their exact path values.
- Updated locked Nano ID and PostCSS transitive dependencies to versions without the reported high-severity vulnerabilities.
- Reconciled persisted step state after reloads, removed orphaned runtime data on pipeline imports, and kept source links valid when steps are renamed.
- Blocked ambiguous field keys, empty output paths, invalid input values, inverted numeric ranges, non-finite numbers, and non-boolean values before pipeline execution.
- Prevented uploads from overwriting existing project files, confirmed discarding dirty editor tabs, and made recursive folder deletion explicit.

## 0.1.1 - 2026-05-18

### Fixed

- Added CSRF protection to execution routes to prevent cross-origin form posts from triggering local pipeline command execution.

## 0.1.0 - 2026-05-18

### Added

- Initial MVP with FastAPI backend, React/Vite frontend, pipeline parsing, validation, execution state, project tree, visual canvas, step editor, and logs panel.
