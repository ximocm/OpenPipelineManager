# Changelog

All notable changes to Open Pipeline Manager are tracked here.

The format follows Keep a Changelog-style sections, and the project uses semantic versioning.

## Unreleased

### Added

- Added repository governance documentation for tests, branching, release notes, and public security defaults.
- Added CI configuration for backend tests and frontend builds.
- Added Dependabot and security reporting guidance.

## 0.1.1 - 2026-05-18

### Fixed

- Added CSRF protection to execution routes to prevent cross-origin form posts from triggering local pipeline command execution.

## 0.1.0 - 2026-05-18

### Added

- Initial MVP with FastAPI backend, React/Vite frontend, pipeline parsing, validation, execution state, project tree, visual canvas, step editor, and logs panel.
