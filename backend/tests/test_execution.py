from __future__ import annotations

from app.models.pipeline import PipelineConfig
from app.services.execution import ExecutionManager
from app.services.storage import ProjectStore


def test_build_command_replaces_placeholders(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": "echo {input_file} {threshold}",
                    "inputs": [{"key": "input_file", "type": "file", "default": "input.txt"}],
                    "parameters": [{"key": "threshold", "type": "decimal", "default": 0.5}],
                }
            ]
        }
    )
    step = store.get_step("a")

    command = ExecutionManager(store).build_command(step)

    assert command == "echo input.txt 0.5"


def test_build_command_wraps_conda_environment(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "environment": "conda:analysis",
                    "command": "python script.py",
                }
            ]
        }
    )
    step = store.get_step("a")

    command = ExecutionManager(store).build_command(step)

    assert command == "conda run -n analysis python script.py"


def test_build_command_replaces_output_placeholders(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": "tool --out {result_file}",
                    "outputs": [{"key": "result_file", "path": "outputs/result.txt"}],
                }
            ]
        }
    )
    step = store.get_step("a")

    command = ExecutionManager(store).build_command(step)

    assert command == "tool --out outputs/result.txt"


def test_build_command_keeps_multiline_commands(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": 'INPUT="{input_file}" \\\nOUTPUT="{result_file}" \\\nbash run.sh',
                    "inputs": [{"key": "input_file", "type": "file", "default": "input.txt"}],
                    "outputs": [{"key": "result_file", "path": "outputs/result.txt"}],
                }
            ]
        }
    )
    step = store.get_step("a")

    command = ExecutionManager(store).build_command(step)

    assert command == 'INPUT="input.txt" \\\nOUTPUT="outputs/result.txt" \\\nbash run.sh'


def test_build_command_errors_for_missing_placeholder(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": "echo {input_file}",
                    "inputs": [{"key": "input_file", "type": "file"}],
                }
            ]
        }
    )

    step = store.get_step("a")

    try:
        ExecutionManager(store).build_command(step)
    except ValueError as exc:
        assert "input_file" in str(exc)
    else:
        raise AssertionError("Expected missing placeholder error")


def test_step_ok_requires_done_marker_and_outputs(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {"id": "a", "name": "A", "outputs": [{"path": "out.txt"}]},
            ]
        }
    )
    step = store.get_step("a")

    assert store.is_step_ok(step) is False
    store.done_path("a").write_text("done", encoding="utf-8")
    assert store.is_step_ok(step) is False
    (tmp_path / "out.txt").write_text("ok", encoding="utf-8")
    assert store.is_step_ok(step) is True


def test_execution_order_respects_dependencies(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {"id": "a", "name": "A"},
                {"id": "b", "name": "B", "dependencies": ["a"]},
                {"id": "c", "name": "C", "dependencies": ["b"]},
            ]
        }
    )

    order = ExecutionManager(store)._topological_subset(["c"])

    assert order == ["a", "b", "c"]
