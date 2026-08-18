from __future__ import annotations

import pytest

from app.models.pipeline import PipelineConfig
from app.services.validation import ValidationService


def test_detects_duplicate_ids(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A"}, {"id": "a", "name": "A again"}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(issue.field == "id" and issue.severity == "blocker" for issue in issues)


def test_detects_missing_dependency(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "b", "name": "B", "dependencies": ["missing"]}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any("does not exist" in issue.message for issue in issues)


def test_detects_cycles(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {"id": "a", "name": "A", "dependencies": ["b"]},
                {"id": "b", "name": "B", "dependencies": ["a"]},
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any("cycle" in issue.message.lower() for issue in issues)


def test_required_input_missing_is_blocker(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "inputs": [{"key": "input_file", "type": "file", "required": True}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(issue.field == "input_file" and issue.severity == "blocker" for issue in issues)


def test_optional_input_missing_is_warning(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "inputs": [{"key": "input_file", "type": "file", "required": False}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(issue.field == "input_file" and issue.severity == "warning" for issue in issues)


def test_numeric_range_validation(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "parameters": [{"key": "threshold", "type": "decimal", "min": 0, "max": 1}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path, {"a": {"threshold": 2}})

    assert any("above maximum" in issue.message for issue in issues)


def test_select_option_validation(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "parameters": [{"key": "method", "type": "select", "options": ["fast"]}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path, {"a": {"method": "slow"}})

    assert any("Invalid option" in issue.message for issue in issues)


def test_selector_type_alias_is_validated_as_select(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "parameters": [{"key": "method", "type": "selector", "options": ["fast"]}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path, {"a": {"method": "slow"}})

    assert pipeline.steps[0].parameters[0].type == "select"
    assert any("Invalid option" in issue.message for issue in issues)


def test_placeholder_without_definition_is_blocker(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", "command": "echo {missing}"}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(issue.field == "command" for issue in issues)


def test_rejects_absolute_working_directory_path(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", "working_directory": "/tmp"}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.field == "working_directory"
        and issue.severity == "blocker"
        and "absolute" in issue.message
        for issue in issues
    )


def test_rejects_parent_working_directory_path(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", "working_directory": ".."}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.field == "working_directory"
        and issue.severity == "blocker"
        and "outside" in issue.message
        for issue in issues
    )


def test_rejects_absolute_input_path(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "inputs": [{"key": "input_file", "type": "file", "required": True, "default": "/etc/passwd"}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.field == "input_file"
        and issue.severity == "blocker"
        and "absolute" in issue.message
        for issue in issues
    )


def test_rejects_nested_traversal_input_path(tmp_path):
    (tmp_path / "safe").mkdir()
    outside_file = tmp_path.parent / f"{tmp_path.name}-outside.txt"
    outside_file.write_text("outside", encoding="utf-8")
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "inputs": [
                        {
                            "key": "input_file",
                            "type": "file",
                            "required": True,
                            "default": f"safe/../../{outside_file.name}",
                        }
                    ],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.field == "input_file"
        and issue.severity == "blocker"
        and "outside" in issue.message
        for issue in issues
    )


def test_rejects_unsafe_output_path(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "outputs": [{"key": "result_file", "path": "../outside.txt"}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.field == "result_file"
        and issue.severity == "blocker"
        and "outside" in issue.message
        for issue in issues
    )


def test_accepts_valid_relative_input_and_output_paths(tmp_path):
    (tmp_path / "Input").mkdir()
    (tmp_path / "outputs").mkdir()
    (tmp_path / "Input" / "data.txt").write_text("data", encoding="utf-8")
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "inputs": [{"key": "input_file", "type": "file", "required": True, "default": "Input/data.txt"}],
                    "outputs": [{"key": "result_file", "path": "outputs/result.txt"}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert not [issue for issue in issues if issue.severity == "blocker"]


def test_preserves_leading_and_trailing_whitespace_in_input_paths(tmp_path):
    input_path = " input.txt "
    (tmp_path / input_path).write_text("data", encoding="utf-8")
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "inputs": [{"key": "input_file", "type": "file", "required": True, "default": input_path}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert not [issue for issue in issues if issue.severity == "blocker"]


def test_rejects_whitespace_only_input_paths(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "inputs": [{"key": "input_file", "type": "file", "required": True, "default": "   "}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.field == "input_file"
        and issue.severity == "blocker"
        and "only whitespace" in issue.message
        for issue in issues
    )


def test_reports_symlink_loop_as_working_directory_blocker(tmp_path):
    loop_path = tmp_path / "loop"
    try:
        loop_path.symlink_to(loop_path.name)
    except OSError:
        pytest.skip("Symbolic links are not available on this platform")
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", "working_directory": loop_path.name}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.field == "working_directory"
        and issue.severity == "blocker"
        and "cannot be resolved" in issue.message
        for issue in issues
    )


def test_allows_external_program_paths_in_command(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "command": "/shared/tools/run-analysis.sh --version",
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert not [issue for issue in issues if issue.severity == "blocker"]
