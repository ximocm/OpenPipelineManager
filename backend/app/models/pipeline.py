from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator


FieldType = Literal["text", "integer", "decimal", "boolean", "select", "file", "folder"]
IssueSeverity = Literal["warning", "blocker"]


class ValueSpec(BaseModel):
    model_config = ConfigDict(populate_by_name=True)

    key: str
    label: str | None = None
    type: FieldType = "text"
    required: bool = False
    default: Any = None
    options: list[str] = Field(default_factory=list)
    minimum: float | None = Field(default=None, alias="min")
    maximum: float | None = Field(default=None, alias="max")
    description: str = ""
    source_step: str | None = None
    source_output: str | None = None

    @field_validator("type", mode="before")
    @classmethod
    def normalize_type(cls, value: Any) -> Any:
        if value == "selector":
            return "select"
        return value


class InputSpec(ValueSpec):
    pass


class ParameterSpec(ValueSpec):
    pass


class OutputSpec(BaseModel):
    key: str = ""
    label: str | None = None
    path: str


class PipelineStep(BaseModel):
    id: str
    name: str
    description: str = ""
    command: str = ""
    environment: str = ""
    working_directory: str = "."
    inputs: list[InputSpec] = Field(default_factory=list)
    outputs: list[OutputSpec] = Field(default_factory=list)
    parameters: list[ParameterSpec] = Field(default_factory=list)
    dependencies: list[str] = Field(default_factory=list)

    def default_values(self) -> dict[str, Any]:
        values: dict[str, Any] = {}
        for item in [*self.inputs, *self.parameters]:
            if item.default is not None:
                values[item.key] = item.default
        for output in self.outputs:
            if output.key:
                values[output.key] = output.path
        return values


class PipelineConfig(BaseModel):
    steps: list[PipelineStep] = Field(default_factory=list)

    def step_by_id(self) -> dict[str, PipelineStep]:
        return {step.id: step for step in self.steps}


class ValidationIssue(BaseModel):
    severity: IssueSeverity
    message: str
    step_id: str | None = None
    field: str | None = None
