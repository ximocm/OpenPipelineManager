from __future__ import annotations

import posixpath
from pathlib import Path, PurePosixPath, PureWindowsPath


class PipelinePathError(ValueError):
    pass


def resolve_pipeline_path(project_path: Path, requested_path: str | Path, *, base: Path | None = None) -> Path:
    project_root = project_path.resolve()
    base_path = project_root if base is None else base.resolve()

    if not _is_inside_project(base_path, project_root):
        raise PipelinePathError(f"Pipeline path base is outside the current project: {base_path}")

    raw_path = str(requested_path).strip() or "."
    if "\x00" in raw_path:
        raise PipelinePathError("Pipeline path contains an invalid null byte")

    normalized_path = raw_path.replace("\\", "/")
    if _is_absolute_path(raw_path, normalized_path):
        raise PipelinePathError(f"Pipeline path must be project-relative, not absolute: {raw_path}")

    resolved = (base_path / normalized_path).resolve()
    if not _is_inside_project(resolved, project_root):
        raise PipelinePathError(f"Pipeline path is outside the current project: {raw_path}")

    return resolved


def relative_project_path(target_working_directory: str, source_working_directory: str, source_output_path: str) -> str:
    target = normalize_project_path(target_working_directory)
    source = normalize_project_path(posixpath.join(source_working_directory or ".", source_output_path))
    relative = posixpath.relpath(source, start=target)
    return "." if relative == "." else relative


def normalize_project_path(path: str) -> str:
    normalized = posixpath.normpath((path or ".").replace("\\", "/"))
    return "." if normalized in {"", "."} else normalized


def _is_absolute_path(raw_path: str, normalized_path: str) -> bool:
    windows_path = PureWindowsPath(raw_path)
    return (
        PurePosixPath(normalized_path).is_absolute()
        or windows_path.is_absolute()
        or bool(windows_path.drive)
        or bool(windows_path.root)
    )


def _is_inside_project(path: Path, project_root: Path) -> bool:
    return path == project_root or path.is_relative_to(project_root)
