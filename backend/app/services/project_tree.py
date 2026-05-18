from __future__ import annotations

import base64
import json
import mimetypes
import shutil
from datetime import datetime, timezone
from pathlib import Path

from app.models.state import FilePreview, TreeNode


IGNORED_NAMES = {".pipeline-manager", ".git", "node_modules", "dist", "build", "__pycache__", ".venv"}
IMAGE_SUFFIXES = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"}
DEFAULT_PROJECT_DIRS = ("Input", "steps", "outputs")
PROJECT_CONFIG_FILENAME = "project.config.json"


def list_directories(path: str | None = None) -> dict[str, object]:
    current = Path(path).expanduser() if path else Path.home()
    current = current.resolve()
    if not current.exists():
        raise FileNotFoundError(str(current))
    if not current.is_dir():
        current = current.parent

    directories: list[dict[str, str]] = []
    for child in sorted(current.iterdir(), key=lambda item: item.name.lower()):
        if child.name.startswith("."):
            continue
        try:
            if child.is_dir():
                directories.append({"name": child.name, "path": str(child.resolve())})
        except OSError:
            continue

    parent = current.parent if current.parent != current else None
    return {
        "path": str(current),
        "parent": str(parent) if parent else None,
        "directories": directories,
    }


def list_filesystem(path: str | None = None, extensions: set[str] | None = None) -> dict[str, object]:
    current = Path(path).expanduser() if path else Path.home()
    current = current.resolve()
    if not current.exists():
        raise FileNotFoundError(str(current))
    if not current.is_dir():
        current = current.parent

    directories: list[dict[str, str]] = []
    files: list[dict[str, str]] = []
    for child in sorted(current.iterdir(), key=lambda item: (item.is_file(), item.name.lower())):
        if child.name.startswith("."):
            continue
        try:
            if child.is_dir():
                directories.append({"name": child.name, "path": str(child.resolve()), "type": "directory"})
            elif child.is_file() and (extensions is None or child.suffix.lower() in extensions):
                files.append({"name": child.name, "path": str(child.resolve()), "type": "file"})
        except OSError:
            continue

    parent = current.parent if current.parent != current else None
    return {
        "path": str(current),
        "parent": str(parent) if parent else None,
        "directories": directories,
        "files": files,
    }


def ensure_project_scaffold(root: Path) -> None:
    root = root.resolve()
    root.mkdir(parents=True, exist_ok=True)
    for dirname in DEFAULT_PROJECT_DIRS:
        (root / dirname).mkdir(exist_ok=True)
    project_config_path = root / PROJECT_CONFIG_FILENAME
    if not project_config_path.exists():
        project_config_path.write_text(
            json.dumps(
                {
                    "name": root.name,
                    "version": 1,
                    "created_at": datetime.now(timezone.utc).isoformat(),
                    "folders": {
                        "input": "Input",
                        "steps": "steps",
                        "outputs": "outputs",
                    },
                },
                indent=2,
            ),
            encoding="utf-8",
        )
    config_path = root / "config.txt"
    if not config_path.exists():
        config_path.write_text(
            "Open Pipeline Manager project\n\n"
            "Input files: Input/\n"
            "Step workspaces: steps/<step-id>/\n"
            "Shared outputs: outputs/\n",
            encoding="utf-8",
        )


def validate_project_folder(root: Path) -> None:
    root = root.resolve()
    if not root.exists() or not root.is_dir():
        raise FileNotFoundError(str(root))
    config_path = root / PROJECT_CONFIG_FILENAME
    if not config_path.exists():
        raise ValueError(f"Folder is not an Open Pipeline Manager project: missing {PROJECT_CONFIG_FILENAME}")


def ensure_step_folders(root: Path, step_ids: list[str]) -> None:
    steps_root = root.resolve() / "steps"
    steps_root.mkdir(exist_ok=True)
    for step_id in step_ids:
        safe_id = safe_segment(step_id)
        step_root = steps_root / safe_id
        for dirname in ("input", "work", "output"):
            (step_root / dirname).mkdir(parents=True, exist_ok=True)


def safe_segment(value: str) -> str:
    spaced = "_".join(value.strip().split())
    cleaned = "".join(char if char.isalnum() or char in {"-", "_", "."} else "_" for char in spaced)
    while "__" in cleaned:
        cleaned = cleaned.replace("__", "_")
    return cleaned.strip("._") or "project"


def resolve_project_path(root: Path, requested_path: str) -> Path:
    root = root.resolve()
    requested = requested_path.strip() or "."
    candidate = (root / requested).resolve()
    if candidate != root and root not in candidate.parents:
        raise ValueError("Path is outside the current project")
    return candidate


def create_project_path(root: Path, requested_path: str, content: str = "", directory: bool = False) -> Path:
    candidate = resolve_project_path(root, requested_path)
    if candidate.exists():
        raise FileExistsError(f"Path already exists: {candidate.relative_to(root.resolve())}")
    if directory:
        candidate.mkdir(parents=True, exist_ok=True)
    else:
        candidate.parent.mkdir(parents=True, exist_ok=True)
        candidate.write_text(content, encoding="utf-8")
    return candidate


def write_project_file(root: Path, requested_path: str, content: str) -> Path:
    candidate = resolve_project_path(root, requested_path)
    if candidate.exists() and not candidate.is_file():
        raise ValueError("Target is not a file")
    candidate.parent.mkdir(parents=True, exist_ok=True)
    candidate.write_text(content, encoding="utf-8")
    return candidate


def rename_project_path(root: Path, requested_path: str, new_name: str) -> Path:
    source = resolve_project_path(root, requested_path)
    if source == root:
        raise ValueError("Cannot rename the project root")
    if "/" in new_name or "\\" in new_name or not new_name.strip():
        raise ValueError("New name must be a single file or folder name")
    target = source.parent / new_name.strip()
    if target.exists():
        raise FileExistsError(f"Target already exists: {target.name}")
    source.rename(target)
    return target


def move_project_path(root: Path, source_path: str, target_directory: str) -> Path:
    source = resolve_project_path(root, source_path)
    destination_dir = resolve_project_path(root, target_directory)
    if source == root:
        raise ValueError("Cannot move the project root")
    if not destination_dir.is_dir():
        raise ValueError("Target is not a directory")
    if source.is_dir() and (destination_dir == source or source in destination_dir.parents):
        raise ValueError("Cannot move a directory into itself")
    target = destination_dir / source.name
    if target.exists():
        raise FileExistsError(f"Target already exists: {target.relative_to(root.resolve())}")
    shutil.move(str(source), str(target))
    return target


def delete_project_path(root: Path, requested_path: str) -> None:
    candidate = resolve_project_path(root, requested_path)
    if candidate == root:
        raise ValueError("Cannot delete the project root")
    if candidate.name in IGNORED_NAMES:
        raise ValueError(f"Cannot delete managed or ignored folder: {candidate.name}")
    if candidate.is_dir():
        shutil.rmtree(candidate)
    elif candidate.exists():
        candidate.unlink()
    else:
        raise FileNotFoundError(requested_path)


def upload_project_file(root: Path, target_directory: str, name: str, content_base64: str) -> Path:
    destination_dir = resolve_project_path(root, target_directory)
    if not destination_dir.is_dir():
        raise ValueError("Upload target is not a directory")
    if "/" in name or "\\" in name or not name.strip():
        raise ValueError("Uploaded file name must be a single file name")
    target = destination_dir / name.strip()
    target.write_bytes(base64.b64decode(content_base64))
    return target


def build_tree(root: Path, max_depth: int = 5) -> TreeNode:
    root = root.resolve()

    def walk(path: Path, depth: int) -> TreeNode:
        if path.is_file():
            return TreeNode(name=path.name, path=str(path.relative_to(root)), type="file")
        children: list[TreeNode] = []
        if depth < max_depth:
            for child in sorted(path.iterdir(), key=lambda item: (item.is_file(), item.name.lower())):
                if child.name in IGNORED_NAMES:
                    continue
                children.append(walk(child, depth + 1))
        relative = "." if path == root else str(path.relative_to(root))
        return TreeNode(name=path.name, path=relative, type="directory", children=children)

    return walk(root, 0)


def preview_file(root: Path, requested_path: str) -> FilePreview:
    try:
        candidate = resolve_project_path(root, requested_path)
    except ValueError:
        return FilePreview(path=requested_path, type="missing")
    if not candidate.exists() or not candidate.is_file():
        return FilePreview(path=requested_path, type="missing")

    if candidate.suffix.lower() in IMAGE_SUFFIXES:
        media_type = mimetypes.guess_type(candidate.name)[0] or "application/octet-stream"
        encoded = base64.b64encode(candidate.read_bytes()).decode("ascii")
        return FilePreview(path=requested_path, type="image", content=encoded, media_type=media_type)

    raw = candidate.read_bytes()
    if is_text_bytes(raw):
        return FilePreview(path=requested_path, type="text", content=raw.decode("utf-8", errors="replace"))
    return FilePreview(path=requested_path, type="binary")


def is_text_bytes(raw: bytes) -> bool:
    if b"\x00" in raw:
        return False
    try:
        raw.decode("utf-8")
    except UnicodeDecodeError:
        return False
    return True
