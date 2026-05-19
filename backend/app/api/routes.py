from __future__ import annotations

import asyncio
import hmac
import secrets
from pathlib import Path
from typing import Any

from fastapi import APIRouter, Depends, Header, HTTPException, Request
from fastapi.responses import StreamingResponse
from pydantic import BaseModel, Field

from app.models.pipeline import PipelineStep
from app.models.state import StepRuntimeState
from app.services.execution import ExecutionManager, sse_message
from app.services.project_tree import (
    build_tree,
    create_project_path,
    delete_project_path,
    list_filesystem,
    list_directories,
    move_project_path,
    preview_file,
    rename_project_path,
    upload_project_file,
    write_project_file,
)
from app.services.storage import ProjectStore


router = APIRouter(prefix="/api")
store = ProjectStore()
executor = ExecutionManager(store)
csrf_token = secrets.token_urlsafe(32)


def require_csrf_token(x_opm_csrf_token: str | None = Header(default=None)) -> None:
    if not x_opm_csrf_token or not hmac.compare_digest(x_opm_csrf_token, csrf_token):
        raise HTTPException(status_code=403, detail="Invalid or missing CSRF token")


class PathRequest(BaseModel):
    path: str = "."


class FileCreateRequest(BaseModel):
    path: str
    content: str = ""
    directory: bool = False


class FileWriteRequest(BaseModel):
    path: str
    content: str = ""


class FileRenameRequest(BaseModel):
    path: str
    name: str


class FileMoveRequest(BaseModel):
    source: str
    target_directory: str


class FileDeleteRequest(BaseModel):
    path: str


class UploadedFilePayload(BaseModel):
    name: str
    content_base64: str


class FileUploadRequest(BaseModel):
    target_directory: str = "."
    files: list[UploadedFilePayload]


class ParamsRequest(BaseModel):
    values: dict[str, Any] = Field(default_factory=dict)


class SelectionRequest(BaseModel):
    selected: bool


class LayoutRequest(BaseModel):
    positions: dict[str, dict[str, float]]


class DependenciesRequest(BaseModel):
    dependencies: list[str] = Field(default_factory=list)


class RunSelectedRequest(BaseModel):
    step_ids: list[str] | None = None


@router.get("/security/csrf-token")
def get_csrf_token() -> dict[str, str]:
    return {"token": csrf_token}


@router.post("/projects")
def create_project(request: PathRequest) -> dict[str, Any]:
    return store.create_project(request.path).model_dump(mode="json", by_alias=True)


@router.post("/projects/open")
def open_project(request: PathRequest) -> dict[str, Any]:
    try:
        return store.open_project(request.path).model_dump(mode="json", by_alias=True)
    except FileNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.get("/projects/current")
def current_project() -> dict[str, Any]:
    return store.snapshot().model_dump(mode="json", by_alias=True)


@router.get("/projects/tree")
def project_tree() -> dict[str, Any]:
    try:
        return build_tree(store.current_project()).model_dump(mode="json")
    except RuntimeError:
        return {"name": "", "path": ".", "type": "directory", "children": []}


@router.get("/filesystem/directories")
def browse_directories(path: str | None = None) -> dict[str, Any]:
    try:
        return list_directories(path)
    except FileNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except PermissionError as exc:
        raise HTTPException(status_code=403, detail=str(exc)) from exc
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.get("/filesystem/files")
def browse_files(path: str | None = None, extensions: str | None = None) -> dict[str, Any]:
    try:
        parsed_extensions = None
        if extensions:
            parsed_extensions = {
                item if item.startswith(".") else f".{item}"
                for item in (part.strip().lower() for part in extensions.split(","))
                if item
            }
        return list_filesystem(path, parsed_extensions)
    except FileNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except PermissionError as exc:
        raise HTTPException(status_code=403, detail=str(exc)) from exc
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.post("/projects/files")
def create_file(request: FileCreateRequest) -> dict[str, str]:
    try:
        root = store.current_project()
        candidate = create_project_path(root, request.path, request.content, request.directory)
        return {"path": str(candidate.relative_to(root))}
    except FileExistsError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.put("/projects/files")
def write_file(request: FileWriteRequest) -> dict[str, str]:
    try:
        root = store.current_project()
        target = write_project_file(root, request.path, request.content)
        return {"path": str(target.relative_to(root))}
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.patch("/projects/files/rename")
def rename_file(request: FileRenameRequest) -> dict[str, str]:
    try:
        root = store.current_project()
        target = rename_project_path(root, request.path, request.name)
        return {"path": str(target.relative_to(root))}
    except FileExistsError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.post("/projects/files/move")
def move_file(request: FileMoveRequest) -> dict[str, str]:
    try:
        root = store.current_project()
        target = move_project_path(root, request.source, request.target_directory)
        return {"path": str(target.relative_to(root))}
    except FileExistsError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.api_route("/projects/files", methods=["DELETE"])
def delete_file(request: FileDeleteRequest) -> dict[str, str]:
    try:
        delete_project_path(store.current_project(), request.path)
        return {"path": request.path}
    except FileNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.post("/projects/files/delete")
def delete_file_post(request: FileDeleteRequest) -> dict[str, str]:
    return delete_file(request)


@router.post("/projects/files/upload")
def upload_files(request: FileUploadRequest) -> dict[str, list[str]]:
    try:
        root = store.current_project()
        uploaded = [
            str(upload_project_file(root, request.target_directory, item.name, item.content_base64).relative_to(root))
            for item in request.files
        ]
        return {"paths": uploaded}
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.get("/files/preview")
def file_preview(path: str) -> dict[str, Any]:
    return preview_file(store.current_project(), path).model_dump(mode="json")


@router.post("/pipeline/import")
def import_pipeline(request: PathRequest) -> dict[str, Any]:
    try:
        pipeline = store.import_pipeline(request.path)
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    return pipeline.model_dump(mode="json", by_alias=True)


@router.get("/pipeline")
def get_pipeline() -> dict[str, Any]:
    try:
        return store.get_pipeline().model_dump(mode="json", by_alias=True)
    except ValueError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@router.get("/pipeline/validation")
def get_validation() -> list[dict[str, Any]]:
    return [issue.model_dump(mode="json") for issue in store.validate()]


@router.post("/pipeline/export")
def export_pipeline(request: PathRequest) -> dict[str, str]:
    pipeline = store.get_pipeline()
    target = Path(request.path)
    if not target.is_absolute():
        target = store.current_project() / target
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(pipeline.model_dump_json(indent=2, by_alias=True), encoding="utf-8")
    return {"path": str(target)}


@router.post("/pipeline/layout")
def save_layout(request: LayoutRequest) -> dict[str, Any]:
    store.update_layout(request.positions)
    return {"visual_layout": store.visual_layout}


@router.post("/pipeline/steps")
def create_pipeline_step(request: PipelineStep) -> dict[str, Any]:
    try:
        step = store.create_step(request)
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    return step.model_dump(mode="json", by_alias=True)


@router.put("/pipeline/steps/{step_id}")
def update_pipeline_step(step_id: str, request: PipelineStep) -> dict[str, Any]:
    try:
        step = store.update_step(step_id, request)
    except KeyError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    return step.model_dump(mode="json", by_alias=True)


@router.patch("/pipeline/steps/{step_id}/dependencies")
def update_step_dependencies(step_id: str, request: DependenciesRequest) -> dict[str, Any]:
    try:
        step = store.set_step_dependencies(step_id, request.dependencies)
    except KeyError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc
    return step.model_dump(mode="json", by_alias=True)


@router.get("/steps/{step_id}")
def get_step(step_id: str) -> dict[str, Any]:
    try:
        step = store.get_step(step_id)
    except KeyError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    return {
        "step": step.model_dump(mode="json", by_alias=True),
        "params": store.params.get(step_id, {}),
        "state": store.state.get(step_id, StepRuntimeState()).model_dump(mode="json"),
        "validation": [issue.model_dump(mode="json") for issue in store.validate() if issue.step_id == step_id],
    }


@router.post("/steps/{step_id}/params")
def update_step_params(step_id: str, request: ParamsRequest) -> dict[str, Any]:
    try:
        store.update_step_params(step_id, request.values)
    except KeyError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    return {"params": store.params.get(step_id, {}), "validation": [issue.model_dump(mode="json") for issue in store.validation]}


@router.post("/steps/{step_id}/selection")
def update_step_selection(step_id: str, request: SelectionRequest) -> dict[str, Any]:
    try:
        store.set_step_selected(step_id, request.selected)
    except KeyError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except ValueError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    return {"state": store.state[step_id].model_dump(mode="json")}


@router.post("/steps/{step_id}/run", dependencies=[Depends(require_csrf_token)])
def run_step(step_id: str) -> dict[str, Any]:
    try:
        return executor.run_step(step_id).model_dump(mode="json")
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.post("/steps/run-selected", dependencies=[Depends(require_csrf_token)])
def run_selected(request: RunSelectedRequest) -> dict[str, Any]:
    try:
        return executor.run_selected(request.step_ids).model_dump(mode="json")
    except Exception as exc:
        raise HTTPException(status_code=400, detail=str(exc)) from exc


@router.post("/steps/stop", dependencies=[Depends(require_csrf_token)])
def stop_execution() -> dict[str, Any]:
    return executor.stop().model_dump(mode="json")


@router.get("/logs/{step_id}")
def get_logs(step_id: str) -> dict[str, str]:
    path = store.log_path(step_id)
    return {"step_id": step_id, "content": path.read_text(encoding="utf-8") if path.exists() else ""}


@router.get("/execution/status")
def execution_status() -> dict[str, Any]:
    return executor.status().model_dump(mode="json")


@router.get("/execution/events")
async def execution_events(request: Request) -> StreamingResponse:
    async def stream():
        while not await request.is_disconnected():
            event = executor.next_event(timeout=0.25)
            if event:
                yield sse_message(event)
            await asyncio.sleep(0.1)

    return StreamingResponse(stream(), media_type="text/event-stream")
