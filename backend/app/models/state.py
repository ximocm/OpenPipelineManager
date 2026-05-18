from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field

from app.models.pipeline import PipelineConfig, ValidationIssue


StepStatus = Literal["pending", "scheduled", "running", "ok", "error", "cancelled"]


class StepRuntimeState(BaseModel):
    status: StepStatus = "pending"
    selected: bool = False
    exit_code: int | None = None
    last_run_at: str | None = None
    message: str = ""


class ProjectSnapshot(BaseModel):
    project_path: str
    pipeline: PipelineConfig | None = None
    params: dict[str, dict[str, Any]] = Field(default_factory=dict)
    state: dict[str, StepRuntimeState] = Field(default_factory=dict)
    visual_layout: dict[str, dict[str, float]] = Field(default_factory=dict)
    validation: list[ValidationIssue] = Field(default_factory=list)


class TreeNode(BaseModel):
    name: str
    path: str
    type: Literal["file", "directory"]
    children: list["TreeNode"] = Field(default_factory=list)


class FilePreview(BaseModel):
    path: str
    type: Literal["text", "image", "binary", "missing"]
    content: str | None = None
    media_type: str | None = None


class ExecutionStatus(BaseModel):
    running: bool = False
    current_step_id: str | None = None
    stop_requested: bool = False
