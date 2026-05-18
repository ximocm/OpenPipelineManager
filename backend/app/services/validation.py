from __future__ import annotations

import re
import posixpath
from collections import Counter
from pathlib import Path
from typing import Any, Callable

from app.models.pipeline import PipelineConfig, PipelineStep, ValidationIssue, ValueSpec


PLACEHOLDER_RE = re.compile(r"{([A-Za-z_][A-Za-z0-9_-]*)}")


def has_value(value: Any) -> bool:
    return value is not None and value != "" and value != []


def effective_step_values(step: PipelineStep, params: dict[str, Any] | None = None) -> dict[str, Any]:
    values = step.default_values()
    if params:
        values.update(params)
    return values


class ValidationService:
    def validate_pipeline(
        self,
        pipeline: PipelineConfig,
        project_path: Path,
        params: dict[str, dict[str, Any]] | None = None,
        ok_checker: Callable[[PipelineStep], bool] | None = None,
    ) -> list[ValidationIssue]:
        params = params or {}
        issues: list[ValidationIssue] = []
        ids = [step.id for step in pipeline.steps]
        counts = Counter(ids)

        for step_id, count in counts.items():
            if count > 1:
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step_id,
                        field="id",
                        message=f"Duplicate step id: {step_id}",
                    )
                )

        step_map = pipeline.step_by_id()
        issues.extend(self._detect_cycles(pipeline))

        for step in pipeline.steps:
            issues.extend(self._validate_source_refs(step, step_map))
            step_params = self._source_input_values(pipeline, step)
            step_params.update(params.get(step.id, {}))
            issues.extend(self.validate_step(step, project_path, step_params))

            for dependency in step.dependencies:
                dependency_step = step_map.get(dependency)
                if dependency_step is None:
                    issues.append(
                        ValidationIssue(
                            severity="blocker",
                            step_id=step.id,
                            field="dependencies",
                            message=f"Dependency does not exist: {dependency}",
                        )
                    )
                    continue

        return issues

    def _validate_source_refs(
        self,
        step: PipelineStep,
        step_map: dict[str, PipelineStep],
    ) -> list[ValidationIssue]:
        issues: list[ValidationIssue] = []
        for input_spec in step.inputs:
            if not input_spec.source_step and not input_spec.source_output:
                continue
            if not input_spec.source_step or not input_spec.source_output:
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step.id,
                        field=input_spec.key,
                        message=f"Input source must include source_step and source_output: {input_spec.key}",
                    )
                )
                continue

            source_step = step_map.get(input_spec.source_step)
            if source_step is None:
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step.id,
                        field=input_spec.key,
                        message=f"Input source step does not exist: {input_spec.source_step}",
                    )
                )
                continue

            source_output = next(
                (
                    output
                    for output in source_step.outputs
                    if output.key == input_spec.source_output or output.path == input_spec.source_output
                ),
                None,
            )
            if source_output is None:
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step.id,
                        field=input_spec.key,
                        message=f"Input source output does not exist: {input_spec.source_output}",
                    )
                )
        return issues

    def _source_input_values(self, pipeline: PipelineConfig, step: PipelineStep) -> dict[str, str]:
        values: dict[str, str] = {}
        step_map = pipeline.step_by_id()

        for input_spec in step.inputs:
            if not input_spec.source_step or not input_spec.source_output:
                continue

            source_step = step_map.get(input_spec.source_step)
            if source_step is None:
                continue

            source_output = next(
                (
                    output
                    for output in source_step.outputs
                    if output.key == input_spec.source_output or output.path == input_spec.source_output
                ),
                None,
            )
            if source_output is None:
                continue

            values[input_spec.key] = relative_project_path(
                step.working_directory,
                source_step.working_directory,
                source_output.path,
            )

        return values

    def validate_step(
        self,
        step: PipelineStep,
        project_path: Path,
        params: dict[str, Any] | None = None,
    ) -> list[ValidationIssue]:
        issues: list[ValidationIssue] = []
        values = effective_step_values(step, params)
        working_directory = project_path / step.working_directory

        if not working_directory.exists():
            issues.append(
                ValidationIssue(
                    severity="blocker",
                    step_id=step.id,
                    field="working_directory",
                    message=f"Working directory does not exist: {step.working_directory}",
                )
            )

        for spec in step.inputs:
            value = values.get(spec.key)
            if spec.type in {"file", "folder"}:
                if has_value(value):
                    if spec.source_step and spec.source_output:
                        continue
                    candidate = working_directory / str(value)
                    exists = candidate.exists()
                    expected_dir = spec.type == "folder"
                    type_matches = candidate.is_dir() if expected_dir else candidate.is_file()
                    if not exists or not type_matches:
                        issues.append(
                            ValidationIssue(
                                severity="blocker" if spec.required else "warning",
                                step_id=step.id,
                                field=spec.key,
                                message=f"{spec.label or spec.key} does not exist: {value}",
                            )
                        )
                elif spec.required:
                    issues.append(
                        ValidationIssue(
                            severity="blocker",
                            step_id=step.id,
                            field=spec.key,
                            message=f"Required input is missing: {spec.label or spec.key}",
                        )
                    )
                else:
                    issues.append(
                        ValidationIssue(
                            severity="warning",
                            step_id=step.id,
                            field=spec.key,
                            message=f"Optional input is not set: {spec.label or spec.key}",
                        )
                    )

        for spec in step.parameters:
            issues.extend(self._validate_value_spec(step.id, spec, values.get(spec.key)))

        declared_keys = {item.key for item in [*step.inputs, *step.parameters]}
        declared_keys.update(output.key for output in step.outputs if output.key)
        for placeholder in sorted(set(PLACEHOLDER_RE.findall(step.command))):
            if placeholder not in declared_keys:
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step.id,
                        field="command",
                        message=f"Placeholder has no input or parameter: {placeholder}",
                    )
                )
            elif not has_value(values.get(placeholder)):
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step.id,
                        field=placeholder,
                        message=f"Placeholder has no value: {placeholder}",
                    )
                )

        return issues

    def _validate_value_spec(
        self,
        step_id: str,
        spec: ValueSpec,
        value: Any,
    ) -> list[ValidationIssue]:
        issues: list[ValidationIssue] = []

        if spec.required and not has_value(value):
            return [
                ValidationIssue(
                    severity="blocker",
                    step_id=step_id,
                    field=spec.key,
                    message=f"Required parameter is missing: {spec.label or spec.key}",
                )
            ]

        if not has_value(value):
            return issues

        if spec.type in {"integer", "decimal"}:
            try:
                numeric = float(value)
            except (TypeError, ValueError):
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step_id,
                        field=spec.key,
                        message=f"Value must be numeric: {spec.label or spec.key}",
                    )
                )
                return issues

            if spec.type == "integer" and int(numeric) != numeric:
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step_id,
                        field=spec.key,
                        message=f"Value must be an integer: {spec.label or spec.key}",
                    )
                )
            if spec.minimum is not None and numeric < spec.minimum:
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step_id,
                        field=spec.key,
                        message=f"Value is below minimum {spec.minimum}: {spec.label or spec.key}",
                    )
                )
            if spec.maximum is not None and numeric > spec.maximum:
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step_id,
                        field=spec.key,
                        message=f"Value is above maximum {spec.maximum}: {spec.label or spec.key}",
                    )
                )

        if spec.type == "select" and spec.options and value not in spec.options:
            issues.append(
                ValidationIssue(
                    severity="blocker",
                    step_id=step_id,
                    field=spec.key,
                    message=f"Invalid option for {spec.label or spec.key}: {value}",
                )
            )

        return issues

    def _detect_cycles(self, pipeline: PipelineConfig) -> list[ValidationIssue]:
        issues: list[ValidationIssue] = []
        step_map = pipeline.step_by_id()
        visiting: set[str] = set()
        visited: set[str] = set()

        def visit(step_id: str, trail: list[str]) -> None:
            if step_id in visiting:
                cycle = " -> ".join([*trail, step_id])
                issues.append(
                    ValidationIssue(
                        severity="blocker",
                        step_id=step_id,
                        field="dependencies",
                        message=f"Dependency cycle detected: {cycle}",
                    )
                )
                return
            if step_id in visited:
                return

            step = step_map.get(step_id)
            if step is None:
                return

            visiting.add(step_id)
            for dependency in step.dependencies:
                visit(dependency, [*trail, step_id])
            visiting.remove(step_id)
            visited.add(step_id)

        for step in pipeline.steps:
            visit(step.id, [])

        return issues


def relative_project_path(target_working_directory: str, source_working_directory: str, source_output_path: str) -> str:
    target = normalize_project_path(target_working_directory)
    source = normalize_project_path(posixpath.join(source_working_directory or ".", source_output_path))
    relative = posixpath.relpath(source, start=target)
    return "." if relative == "." else relative


def normalize_project_path(path: str) -> str:
    normalized = posixpath.normpath((path or ".").replace("\\", "/"))
    return "." if normalized in {"", "."} else normalized
