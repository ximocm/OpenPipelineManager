from __future__ import annotations

import sys

import pytest

from app.models.pipeline import PipelineConfig
from app.services.pipeline_paths import relative_project_path
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


def test_rejects_windows_absolute_input_path_on_every_platform(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "inputs": [
                        {"key": "input_file", "type": "file", "required": True, "default": "C:\\outside.txt"}
                    ],
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


@pytest.mark.skipif(sys.platform == "win32", reason="Backslashes are path separators on Windows")
def test_preserves_posix_backslashes_in_input_paths(tmp_path):
    input_path = "input\\file.txt"
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


@pytest.mark.skipif(sys.platform == "win32", reason="Backslashes are path separators on Windows")
def test_preserves_posix_backslashes_when_linking_step_outputs(tmp_path):
    relative = relative_project_path(tmp_path, "consume", "produce", "result\\file.txt")

    assert relative == "../produce/result\\file.txt"


def test_source_link_relpath_failure_becomes_validation_blocker(tmp_path, monkeypatch):
    def fail_relpath(*args, **kwargs):
        raise ValueError("paths are on different drives")

    monkeypatch.setattr("app.services.pipeline_paths.os.path.relpath", fail_relpath)
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "produce",
                    "name": "Produce",
                    "outputs": [{"key": "result", "path": "result.txt"}],
                },
                {
                    "id": "consume",
                    "name": "Consume",
                    "inputs": [
                        {
                            "key": "result_file",
                            "type": "file",
                            "source_step": "produce",
                            "source_output": "result",
                        }
                    ],
                },
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.step_id == "consume"
        and issue.field == "result_file"
        and issue.severity == "blocker"
        and "cannot be made relative" in issue.message
        for issue in issues
    )


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


@pytest.mark.parametrize(
    ("inputs", "parameters", "outputs"),
    [
        ([{"key": "shared", "type": "text"}], [{"key": "shared", "type": "text"}], []),
        ([{"key": "shared", "type": "text"}], [], [{"key": "shared", "path": "result.txt"}]),
        ([], [{"key": "shared", "type": "text"}], [{"key": "shared", "path": "result.txt"}]),
    ],
    ids=["input-and-parameter", "input-and-output", "parameter-and-output"],
)
def test_duplicate_keys_across_step_fields_are_blockers(tmp_path, inputs, parameters, outputs):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {"id": "a", "name": "A", "inputs": inputs, "parameters": parameters, "outputs": outputs}
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(issue.field == "shared" and issue.severity == "blocker" for issue in issues)


@pytest.mark.parametrize("key", ["", "   "], ids=["empty", "whitespace"])
@pytest.mark.parametrize(
    ("field_name", "field_value"),
    [
        ("inputs", {"key": "", "type": "text"}),
        ("parameters", {"key": "", "type": "text"}),
    ],
    ids=["input", "parameter"],
)
def test_empty_step_field_keys_are_blockers(tmp_path, key, field_name, field_value):
    field_value = {**field_value, "key": key}
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", field_name: [field_value]}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(issue.severity == "blocker" and "key" in issue.message.lower() for issue in issues)


@pytest.mark.parametrize("path", ["", "   "], ids=["empty", "whitespace"])
def test_empty_output_paths_are_blockers(tmp_path, path):
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", "outputs": [{"key": "result", "path": path}]}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(issue.field == "result" and issue.severity == "blocker" for issue in issues)


def test_unkeyed_output_remains_valid_after_model_round_trip(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", "outputs": [{"path": "result.txt"}]}]}
    )
    reloaded = PipelineConfig.model_validate(pipeline.model_dump(mode="json"))

    issues = ValidationService().validate_pipeline(reloaded, tmp_path)

    assert not [issue for issue in issues if issue.severity == "blocker"]


def test_whitespace_only_output_key_is_blocker(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", "outputs": [{"key": "   ", "path": "result.txt"}]}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(issue.severity == "blocker" and "output key" in issue.message.lower() for issue in issues)


@pytest.mark.parametrize(
    ("spec", "provided_value", "expected_message"),
    [
        ({"key": "title", "type": "text", "required": True}, None, "required"),
        ({"key": "count", "type": "integer"}, "not-a-number", "numeric"),
        ({"key": "count", "type": "integer"}, 1.5, "integer"),
        ({"key": "ratio", "type": "decimal", "min": 0, "max": 1}, 2, "maximum"),
        ({"key": "mode", "type": "select", "options": ["fast"]}, "slow", "invalid option"),
    ],
    ids=["required", "non-numeric", "fractional-integer", "out-of-range", "invalid-select"],
)
def test_non_file_inputs_use_value_spec_validation(tmp_path, spec, provided_value, expected_message):
    pipeline = PipelineConfig.model_validate({"steps": [{"id": "a", "name": "A", "inputs": [spec]}]})
    values = {} if provided_value is None else {"a": {spec["key"]: provided_value}}

    issues = ValidationService().validate_pipeline(pipeline, tmp_path, values)

    assert any(
        issue.field == spec["key"]
        and issue.severity == "blocker"
        and expected_message in issue.message.lower()
        for issue in issues
    )


@pytest.mark.parametrize("value", [True, False, float("nan"), float("inf"), float("-inf")])
def test_non_numeric_values_and_non_finite_numbers_are_blockers(tmp_path, value):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "parameters": [{"key": "threshold", "type": "decimal"}],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path, {"a": {"threshold": value}})

    assert any(issue.field == "threshold" and issue.severity == "blocker" for issue in issues)


@pytest.mark.parametrize("field_name", ["inputs", "parameters"])
@pytest.mark.parametrize("value_source", ["default", "submitted"])
@pytest.mark.parametrize("invalid_value", ["true", "false", "yes", "no", 0, 1])
def test_invalid_boolean_defaults_and_submitted_values_are_blockers(
    tmp_path, field_name, value_source, invalid_value
):
    spec = {"key": "enabled", "type": "boolean"}
    params = {}
    if value_source == "default":
        spec["default"] = invalid_value
    else:
        params = {"a": {"enabled": invalid_value}}
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", field_name: [spec]}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path, params)

    assert any(
        issue.field == "enabled" and issue.severity == "blocker" and "boolean" in issue.message.lower()
        for issue in issues
    )


@pytest.mark.parametrize("field_name", ["inputs", "parameters"])
@pytest.mark.parametrize("value_source", ["default", "submitted"])
@pytest.mark.parametrize("boolean_value", [True, False])
def test_native_boolean_defaults_and_submitted_values_are_valid(
    tmp_path, field_name, value_source, boolean_value
):
    spec = {"key": "enabled", "type": "boolean"}
    params = {}
    if value_source == "default":
        spec["default"] = boolean_value
    else:
        params = {"a": {"enabled": boolean_value}}
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", field_name: [spec]}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path, params)

    assert not [issue for issue in issues if issue.field == "enabled" and issue.severity == "blocker"]


@pytest.mark.parametrize("field_name", ["inputs", "parameters"])
def test_inverted_numeric_ranges_are_blockers(tmp_path, field_name):
    spec = {"key": "threshold", "type": "decimal", "min": 10, "max": 1, "default": 5}
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", field_name: [spec]}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.field == "threshold" and issue.severity == "blocker" and "minimum" in issue.message.lower()
        for issue in issues
    )


@pytest.mark.parametrize("field_name", ["inputs", "parameters"])
@pytest.mark.parametrize("bound_name", ["min", "max"])
@pytest.mark.parametrize("bound_value", [float("nan"), float("inf"), float("-inf")])
def test_non_finite_numeric_bounds_are_blockers(tmp_path, field_name, bound_name, bound_value):
    spec = {"key": "threshold", "type": "decimal", bound_name: bound_value}
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", field_name: [spec]}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert any(
        issue.field == "threshold" and issue.severity == "blocker" and "finite" in issue.message.lower()
        for issue in issues
    )


@pytest.mark.parametrize("field_name", ["inputs", "parameters"])
@pytest.mark.parametrize("value_source", ["default", "submitted"])
@pytest.mark.parametrize("value", [float("nan"), float("inf"), float("-inf")])
def test_non_finite_numeric_defaults_and_submitted_values_are_blockers(
    tmp_path, field_name, value_source, value
):
    spec = {"key": "threshold", "type": "decimal"}
    params = {}
    if value_source == "default":
        spec["default"] = value
    else:
        params = {"a": {"threshold": value}}
    pipeline = PipelineConfig.model_validate(
        {"steps": [{"id": "a", "name": "A", field_name: [spec]}]}
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path, params)

    assert any(
        issue.field == "threshold" and issue.severity == "blocker" and "finite" in issue.message.lower()
        for issue in issues
    )


def test_valid_non_file_input_and_parameter_values_do_not_create_blockers(tmp_path):
    pipeline = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "a",
                    "name": "A",
                    "inputs": [
                        {"key": "title", "type": "text", "required": True, "default": "report"},
                        {"key": "count", "type": "integer", "min": 1, "max": 5, "default": 3},
                        {"key": "ratio", "type": "decimal", "min": 0, "max": 1, "default": 0.25},
                        {"key": "mode", "type": "select", "options": ["fast", "safe"], "default": "fast"},
                        {"key": "enabled", "type": "boolean", "required": True, "default": False},
                    ],
                    "parameters": [
                        {"key": "retries", "type": "integer", "min": 0, "max": 3, "default": 2},
                        {"key": "dry_run", "type": "boolean", "default": True},
                    ],
                    "outputs": [
                        {"key": "result", "path": "outputs/result.txt"},
                        {"path": "outputs/unnamed.txt"},
                    ],
                }
            ]
        }
    )

    issues = ValidationService().validate_pipeline(pipeline, tmp_path)

    assert not [issue for issue in issues if issue.severity == "blocker"]
