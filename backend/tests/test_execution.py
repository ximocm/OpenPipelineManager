from __future__ import annotations

import ast
import shlex
import sys

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


def test_build_command_quotes_placeholder_values_with_shell_metacharacters(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": "tool --input {input_file}",
                    "inputs": [
                        {
                            "key": "input_file",
                            "type": "file",
                            "default": "Input/sample file.txt; touch injected",
                        }
                    ],
                }
            ]
        }
    )
    step = store.get_step("a")

    command = ExecutionManager(store).build_command(step)

    assert command == "tool --input 'Input/sample file.txt; touch injected'"


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


def test_placeholder_value_executes_as_one_shell_argument(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    input_value = "Input/sample file.txt; touch injected"
    (tmp_path / "Input").mkdir(exist_ok=True)
    (tmp_path / input_value).write_text("ok", encoding="utf-8")
    script = "import pathlib, sys; pathlib.Path('argv.txt').write_text(repr(sys.argv[1:]), encoding='utf-8')"
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": f"{shlex.quote(sys.executable)} -c {shlex.quote(script)} {{input_file}}",
                    "inputs": [{"key": "input_file", "type": "file", "required": True, "default": input_value}],
                }
            ]
        }
    )
    manager = ExecutionManager(store)

    manager.run_step("a")
    assert manager.thread is not None
    manager.thread.join(timeout=5)

    assert store.state["a"].status == "ok"
    assert ast.literal_eval((tmp_path / "argv.txt").read_text(encoding="utf-8")) == [input_value]
    assert not (tmp_path / "injected").exists()


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
