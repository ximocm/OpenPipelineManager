from __future__ import annotations

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
