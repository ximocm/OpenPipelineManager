from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from app.models.pipeline import PipelineConfig, PipelineStep, ValidationIssue
from app.models.state import ProjectSnapshot, StepRuntimeState
from app.services.pipeline_paths import PipelinePathError, relative_project_path, resolve_pipeline_path
from app.services.pipeline_parser import parse_pipeline_file
from app.services.project_tree import ensure_project_scaffold, ensure_step_folders, safe_segment, validate_project_folder
from app.services.validation import ValidationService

LAYOUT_START_X = 80.0
LAYOUT_START_Y = 120.0
LAYOUT_GAP_X = 330.0


class ProjectStore:
    def __init__(self) -> None:
        self.current_path: Path | None = None
        self.pipeline_config: PipelineConfig | None = None
        self.params: dict[str, dict[str, Any]] = {}
        self.state: dict[str, StepRuntimeState] = {}
        self.visual_layout: dict[str, dict[str, float]] = {}
        self.validation: list[ValidationIssue] = []
        self.validator = ValidationService()

    def create_project(self, path: str | Path) -> ProjectSnapshot:
        requested_path = Path(path).expanduser()
        project_path = (requested_path.parent.resolve() / safe_segment(requested_path.name)).resolve()
        ensure_project_scaffold(project_path)
        return self._load_project(project_path)

    def open_project(self, path: str | Path) -> ProjectSnapshot:
        project_path = Path(path).expanduser().resolve()
        validate_project_folder(project_path)
        return self._load_project(project_path)

    def _load_project(self, project_path: Path) -> ProjectSnapshot:
        self.current_path = project_path
        self._ensure_manager_dirs()
        self._load_runtime_files()
        self._load_snapshot()
        return self.snapshot()

    def current_project(self) -> Path:
        if self.current_path is None:
            raise RuntimeError("No project is open")
        assert self.current_path is not None
        return self.current_path

    def manager_dir(self) -> Path:
        return self.current_project() / ".pipeline-manager"

    def logs_dir(self) -> Path:
        return self.manager_dir() / "logs"

    def done_dir(self) -> Path:
        return self.manager_dir() / "done"

    def import_pipeline(self, path: str | Path) -> PipelineConfig:
        pipeline_path = Path(path).expanduser()
        if not pipeline_path.is_absolute():
            pipeline_path = self.current_project() / pipeline_path
        known_step_ids = set(self.params) | set(self.state) | set(self.visual_layout)
        if self.pipeline_config is not None:
            known_step_ids.update(step.id for step in self.pipeline_config.steps)
        self.pipeline_config = parse_pipeline_file(pipeline_path)
        self._resolve_input_sources()
        self._connect_linear_pipeline_if_unconnected()
        current_step_ids = {step.id for step in self.pipeline_config.steps}
        self._prune_removed_step_runtime(current_step_ids, known_step_ids)
        self.visual_layout = self._default_visual_layout(self.pipeline_config)
        ensure_step_folders(self.current_project(), [step.id for step in self.pipeline_config.steps])
        self._initialize_pipeline_state()
        self.validate()
        self.save_all()
        return self.pipeline_config

    def create_step(self, step: PipelineStep) -> PipelineStep:
        if self.pipeline_config is None:
            self.pipeline_config = PipelineConfig(steps=[])

        step_id = self._normalize_step_id(step.id)
        if any(existing.id == step_id for existing in self.pipeline_config.steps):
            raise ValueError(f"Step already exists: {step_id}")

        existing_steps = self.pipeline_config.steps
        dependencies = step.dependencies or ([existing_steps[-1].id] if existing_steps else [])
        normalized_step = self._normalized_step(step, step_id, dependencies)

        self.pipeline_config.steps.append(normalized_step)
        self._resolve_input_sources()
        normalized_step = self.pipeline_config.steps[-1]
        ensure_step_folders(self.current_project(), [normalized_step.id])
        self.params[normalized_step.id] = self._runtime_params(normalized_step)
        self.state[normalized_step.id] = StepRuntimeState()
        self.visual_layout[normalized_step.id] = self._default_step_position(len(self.pipeline_config.steps) - 1)
        self.validate()
        self.save_all()
        return normalized_step

    def update_step(self, step_id: str, step: PipelineStep) -> PipelineStep:
        pipeline = self.get_pipeline()
        current_index = next((index for index, item in enumerate(pipeline.steps) if item.id == step_id), None)
        if current_index is None:
            raise KeyError(f"Unknown step: {step_id}")

        new_step_id = self._normalize_step_id(step.id)
        if any(existing.id == new_step_id and existing.id != step_id for existing in pipeline.steps):
            raise ValueError(f"Step already exists: {new_step_id}")

        normalized_step = self._normalized_step(step, new_step_id, step.dependencies)
        pipeline.steps[current_index] = normalized_step

        if new_step_id != step_id:
            for item in pipeline.steps:
                item.dependencies = [new_step_id if dependency == step_id else dependency for dependency in item.dependencies]
                for input_spec in item.inputs:
                    if input_spec.source_step == step_id:
                        input_spec.source_step = new_step_id

        self._resolve_input_sources()
        normalized_step = pipeline.steps[current_index]

        if new_step_id != step_id:
            existing_params = self.params.pop(step_id, {})
            existing_params.update(self.params.pop(new_step_id, {}))
            self.params[new_step_id] = self._runtime_params(normalized_step, existing_params)
            self.state[new_step_id] = self.state.pop(step_id, StepRuntimeState())
            self.visual_layout[new_step_id] = self.visual_layout.pop(step_id, self._default_step_position(current_index))
            if self.current_path is not None:
                old_log = self.log_path(step_id)
                new_log = self.log_path(new_step_id)
                if old_log.exists() and not new_log.exists():
                    old_log.rename(new_log)
                old_done = self.done_path(step_id)
                new_done = self.done_path(new_step_id)
                if old_done.exists() and not new_done.exists():
                    old_done.rename(new_done)
        else:
            self.params[new_step_id] = self._runtime_params(
                normalized_step,
                self.params.get(new_step_id, {}),
            )
            self.state.setdefault(new_step_id, StepRuntimeState())
            self.visual_layout.setdefault(new_step_id, self._default_step_position(current_index))

        ensure_step_folders(self.current_project(), [normalized_step.id])
        self.validate()
        self.save_all()
        return normalized_step

    def set_step_dependencies(self, step_id: str, dependencies: list[str]) -> PipelineStep:
        step = self.get_step(step_id)
        step_ids = {item.id for item in self.get_pipeline().steps}
        normalized_dependencies = list(dict.fromkeys(dependencies))
        if step_id in normalized_dependencies:
            raise ValueError("A step cannot depend on itself")
        unknown = [dependency for dependency in normalized_dependencies if dependency not in step_ids]
        if unknown:
            raise ValueError(f"Unknown dependency: {unknown[0]}")

        step.dependencies = normalized_dependencies
        self.validate()
        self.save_all()
        return step

    def get_pipeline(self) -> PipelineConfig:
        if self.pipeline_config is None:
            raise ValueError("No pipeline has been imported")
        return self.pipeline_config

    def get_step(self, step_id: str) -> PipelineStep:
        for step in self.get_pipeline().steps:
            if step.id == step_id:
                return step
        raise KeyError(f"Unknown step: {step_id}")

    def update_step_params(self, step_id: str, values: dict[str, Any]) -> None:
        step = self.get_step(step_id)
        updated_params = dict(self.params.get(step_id, {}))
        updated_params.update(values)
        self.params[step_id] = self._runtime_params(step, updated_params)
        self.validate()
        self.save_all()

    def set_step_selected(self, step_id: str, selected: bool) -> None:
        self.get_step(step_id)
        self.state.setdefault(step_id, StepRuntimeState()).selected = selected
        self.save_all()

    def update_layout(self, positions: dict[str, dict[str, float]]) -> None:
        if self.current_path is None:
            return
        self.visual_layout.update(positions)
        self.save_all()

    def set_step_state(self, step_id: str, state: StepRuntimeState) -> None:
        self.state[step_id] = state
        self.save_all()

    def is_step_ok(self, step: PipelineStep) -> bool:
        done_marker = self.done_dir() / f"{step.id}.done"
        try:
            working_directory = resolve_pipeline_path(self.current_project(), step.working_directory)
            output_paths = [
                resolve_pipeline_path(self.current_project(), output.path, base=working_directory)
                for output in step.outputs
            ]
        except PipelinePathError:
            return False
        outputs_exist = all(output_path.exists() for output_path in output_paths)
        return done_marker.exists() and outputs_exist

    def validate(self) -> list[ValidationIssue]:
        if self.pipeline_config is None:
            self.validation = []
            if self.current_path is None:
                return self.validation
        else:
            self._resolve_input_sources()
            self.validation = self.validator.validate_pipeline(
                self.pipeline_config,
                self.current_project(),
                self.params,
                ok_checker=self.is_step_ok,
            )
        self._save_json("validation.json", [issue.model_dump() for issue in self.validation])
        return self.validation

    def log_path(self, step_id: str) -> Path:
        safe_id = step_id.replace("/", "_")
        return self.logs_dir() / f"{safe_id}.log"

    def done_path(self, step_id: str) -> Path:
        safe_id = step_id.replace("/", "_")
        return self.done_dir() / f"{safe_id}.done"

    def snapshot(self) -> ProjectSnapshot:
        if self.current_path is None:
            return ProjectSnapshot(
                project_path="",
                pipeline=None,
                params={},
                state={},
                visual_layout={},
                validation=[],
            )
        return ProjectSnapshot(
            project_path=str(self.current_project()),
            pipeline=self.pipeline_config,
            params=self.params,
            state=self.state,
            visual_layout=self.visual_layout,
            validation=self.validation,
        )

    def save_all(self) -> None:
        self._ensure_manager_dirs()
        self._save_json("params.json", self.params)
        self._save_json("state.json", {key: value.model_dump() for key, value in self.state.items()})
        self._save_json("visual-layout.json", self.visual_layout)
        snapshot_path = self.current_project() / "pipeline.project.json"
        snapshot_path.write_text(
            json.dumps(self.snapshot().model_dump(mode="json", by_alias=True), indent=2),
            encoding="utf-8",
        )

    def _initialize_pipeline_state(self) -> None:
        if self.pipeline_config is None:
            return
        for step in self.pipeline_config.steps:
            self.params[step.id] = self._runtime_params(step, self.params.get(step.id, {}))
            runtime_state = self.state.setdefault(step.id, StepRuntimeState())
            if self.is_step_ok(step):
                runtime_state.status = "ok"
            elif runtime_state.status == "ok":
                runtime_state.status = "pending"
                runtime_state.exit_code = None
                runtime_state.message = ""

    def _ensure_manager_dirs(self) -> None:
        self.manager_dir().mkdir(exist_ok=True)
        self.logs_dir().mkdir(exist_ok=True)
        self.done_dir().mkdir(exist_ok=True)

    def _load_runtime_files(self) -> None:
        self.params = self._read_json("params.json", {})
        raw_state = self._read_json("state.json", {})
        self.state = {key: StepRuntimeState.model_validate(value) for key, value in raw_state.items()}
        self.visual_layout = self._read_json("visual-layout.json", {})
        raw_validation = self._read_json("validation.json", [])
        self.validation = [ValidationIssue.model_validate(item) for item in raw_validation]

    def _default_visual_layout(self, pipeline: PipelineConfig) -> dict[str, dict[str, float]]:
        return {
            step.id: self._default_step_position(index)
            for index, step in enumerate(pipeline.steps)
        }

    def _default_step_position(self, index: int) -> dict[str, float]:
        return {
            "x": LAYOUT_START_X + index * LAYOUT_GAP_X,
            "y": LAYOUT_START_Y,
        }

    def _normalize_step_id(self, step_id: str) -> str:
        requested_step_id = step_id.strip()
        if not requested_step_id:
            raise ValueError("Step id is required")
        return safe_segment(requested_step_id)

    def _normalized_step(self, step: PipelineStep, step_id: str, dependencies: list[str]) -> PipelineStep:
        working_directory = step.working_directory if step.working_directory != "" else f"steps/{step_id}/work"
        return step.model_copy(
            update={
                "id": step_id,
                "name": step.name.strip() or step_id,
                "environment": step.environment.strip(),
                "working_directory": working_directory,
                "dependencies": list(dict.fromkeys(dependencies)),
            }
        )

    def _runtime_params(
        self,
        step: PipelineStep,
        existing: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        allowed_keys = {item.key for item in [*step.inputs, *step.parameters]}
        values = step.default_values()
        if existing:
            values.update({key: value for key, value in existing.items() if key in allowed_keys})
        return values

    def _prune_removed_step_runtime(
        self,
        current_step_ids: set[str],
        known_step_ids: set[str],
    ) -> None:
        self.params = {
            step_id: values
            for step_id, values in self.params.items()
            if step_id in current_step_ids
        }
        self.state = {
            step_id: value
            for step_id, value in self.state.items()
            if step_id in current_step_ids
        }
        self.visual_layout = {
            step_id: position
            for step_id, position in self.visual_layout.items()
            if step_id in current_step_ids
        }
        for step_id in known_step_ids - current_step_ids:
            self.log_path(step_id).unlink(missing_ok=True)
            self.done_path(step_id).unlink(missing_ok=True)

    def _resolve_input_sources(self) -> None:
        if self.pipeline_config is None:
            return

        step_map = self.pipeline_config.step_by_id()
        for step in self.pipeline_config.steps:
            dependencies = list(step.dependencies)
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

                try:
                    source_value = relative_project_path(
                        self.current_project(),
                        step.working_directory,
                        source_step.working_directory,
                        source_output.path,
                    )
                except PipelinePathError:
                    source_value = None
                if source_value is not None:
                    input_spec.default = source_value
                    self.params.setdefault(step.id, {})[input_spec.key] = source_value
                if source_step.id not in dependencies:
                    dependencies.append(source_step.id)

            step.dependencies = dependencies

    def _connect_linear_pipeline_if_unconnected(self) -> None:
        if self.pipeline_config is None or any(step.dependencies for step in self.pipeline_config.steps):
            return
        for index, step in enumerate(self.pipeline_config.steps[1:], start=1):
            step.dependencies = [self.pipeline_config.steps[index - 1].id]

    def _load_snapshot(self) -> None:
        snapshot_path = self.current_project() / "pipeline.project.json"
        if not snapshot_path.exists():
            self.pipeline_config = None
            return
        data = json.loads(snapshot_path.read_text(encoding="utf-8"))
        snapshot = ProjectSnapshot.model_validate(data)
        self.pipeline_config = snapshot.pipeline
        self.params = snapshot.params or self.params
        self.state = snapshot.state or self.state
        self.visual_layout = snapshot.visual_layout or self.visual_layout
        self.validation = snapshot.validation or self.validation
        self._resolve_input_sources()
        self._initialize_pipeline_state()

    def _read_json(self, name: str, default: Any) -> Any:
        path = self.manager_dir() / name
        if not path.exists():
            return default
        return json.loads(path.read_text(encoding="utf-8"))

    def _save_json(self, name: str, data: Any) -> None:
        self._ensure_manager_dirs()
        path = self.manager_dir() / name
        path.write_text(json.dumps(data, indent=2), encoding="utf-8")
