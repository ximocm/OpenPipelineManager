from __future__ import annotations

import ast
import shlex
import sys

import pytest

from app.models.pipeline import PipelineConfig, PipelineStep
from app.services.execution import ExecutionBlockedError, ExecutionManager
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


def test_build_command_does_not_allow_params_to_override_output_paths(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": "tool --out {result_file}",
                    "outputs": [{"key": "result_file", "path": "safe.txt"}],
                }
            ]
        }
    )
    step = store.get_step("a")

    command = ExecutionManager(store).build_command(step, {"result_file": "../outside.txt"})

    assert command == "tool --out safe.txt"


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


def test_build_command_preserves_already_quoted_placeholder_boundaries(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": 'tool --input "{input_file}" --label \'{sample_label}\'',
                    "inputs": [{"key": "input_file", "type": "file", "default": "Input/sample file.txt; touch injected"}],
                    "parameters": [{"key": "sample_label", "type": "text", "default": "alpha's sample"}],
                }
            ]
        }
    )
    step = store.get_step("a")

    command = ExecutionManager(store).build_command(step)

    assert command == 'tool --input "Input/sample file.txt; touch injected" --label \'alpha\'\\\'\'s sample\''
    assert shlex.split(command) == [
        "tool",
        "--input",
        "Input/sample file.txt; touch injected",
        "--label",
        "alpha's sample",
    ]


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


def test_quoted_placeholder_value_executes_as_one_shell_argument(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    input_value = "Input/sample $HOME file.txt; touch injected"
    (tmp_path / "Input").mkdir(exist_ok=True)
    (tmp_path / input_value).write_text("ok", encoding="utf-8")
    script = "import pathlib, sys; pathlib.Path('quoted-argv.txt').write_text(repr(sys.argv[1:]), encoding='utf-8')"
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": f'{shlex.quote(sys.executable)} -c {shlex.quote(script)} "{{input_file}}"',
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
    assert ast.literal_eval((tmp_path / "quoted-argv.txt").read_text(encoding="utf-8")) == [input_value]
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


def test_step_ok_preserves_output_path_whitespace(tmp_path):
    output_path = " result.txt "
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {"id": "a", "name": "A", "outputs": [{"path": output_path}]},
            ]
        }
    )
    step = store.get_step("a")

    store.done_path("a").write_text("done", encoding="utf-8")
    (tmp_path / output_path.strip()).write_text("wrong file", encoding="utf-8")
    assert store.is_step_ok(step) is False

    (tmp_path / output_path).write_text("expected file", encoding="utf-8")
    assert store.is_step_ok(step) is True


@pytest.mark.skipif(sys.platform == "win32", reason="Backslashes are path separators on Windows")
def test_step_ok_preserves_posix_backslashes_in_output_paths(tmp_path):
    output_path = "result\\file.txt"
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {"id": "a", "name": "A", "outputs": [{"path": output_path}]},
            ]
        }
    )
    step = store.get_step("a")

    store.done_path("a").write_text("done", encoding="utf-8")
    (tmp_path / "result" / "file.txt").parent.mkdir(parents=True)
    (tmp_path / "result" / "file.txt").write_text("wrong file", encoding="utf-8")
    assert store.is_step_ok(step) is False

    (tmp_path / output_path).write_text("expected file", encoding="utf-8")
    assert store.is_step_ok(step) is True


def test_step_ok_returns_false_for_symlink_loop_output(tmp_path):
    loop_path = tmp_path / "loop"
    try:
        loop_path.symlink_to(loop_path.name)
    except OSError:
        pytest.skip("Symbolic links are not available on this platform")
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {"id": "a", "name": "A", "outputs": [{"path": loop_path.name}]},
            ]
        }
    )
    step = store.get_step("a")
    store.done_path("a").write_text("done", encoding="utf-8")

    assert store.is_step_ok(step) is False


def test_step_ok_ignores_unsafe_outside_project_outputs(tmp_path):
    outside_file = tmp_path.parent / f"{tmp_path.name}-outside-output.txt"
    outside_file.write_text("ok", encoding="utf-8")
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {"id": "a", "name": "A", "outputs": [{"path": f"../{outside_file.name}"}]},
            ]
        }
    )
    step = store.get_step("a")

    store.done_path("a").write_text("done", encoding="utf-8")

    assert store.is_step_ok(step) is False


def test_execution_blocks_unsafe_working_directory_before_starting_process(tmp_path):
    outside_dir = tmp_path.parent / f"{tmp_path.name}-outside-work"
    outside_dir.mkdir()
    marker = outside_dir / "ran.txt"
    store = ProjectStore()
    store.create_project(tmp_path)
    store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "working_directory": f"../{outside_dir.name}",
                    "command": "printf ran > ran.txt",
                }
            ]
        }
    )
    manager = ExecutionManager(store)

    try:
        manager.run_step("a")
    except ExecutionBlockedError as exc:
        assert any(issue.field == "working_directory" for issue in exc.blockers)
    else:
        raise AssertionError("Expected unsafe working directory to block execution")

    assert manager.thread is None
    assert not marker.exists()


def test_update_step_removes_stale_output_params_before_validation_and_execution(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.create_step(
        PipelineStep.model_validate(
            {
                "id": "a",
                "name": "A",
                "working_directory": ".",
                "command": "tool --out {result_file}",
                "outputs": [{"key": "result_file", "path": "../outside.txt"}],
            }
        )
    )
    store.params["a"]["result_file"] = "../outside.txt"

    updated = store.update_step(
        "a",
        PipelineStep.model_validate(
            {
                "id": "a",
                "name": "A",
                "working_directory": ".",
                "command": "tool --out {result_file}",
                "outputs": [{"key": "result_file", "path": "safe.txt"}],
            }
        ),
    )

    assert "result_file" not in store.params["a"]
    assert not [issue for issue in store.validation if issue.severity == "blocker"]
    assert ExecutionManager(store).build_command(updated, store.params["a"]) == "tool --out safe.txt"

    reloaded_store = ProjectStore()
    reloaded_store.open_project(tmp_path)
    reloaded_step = reloaded_store.get_step("a")
    assert "result_file" not in reloaded_store.params["a"]
    assert ExecutionManager(reloaded_store).build_command(
        reloaded_step,
        reloaded_store.params["a"],
    ) == "tool --out safe.txt"


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
