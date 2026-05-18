from __future__ import annotations

import json

from app.services.pipeline_parser import parse_pipeline_file


def test_parse_pipeline_yml(tmp_path):
    pipeline_path = tmp_path / "pipeline.yml"
    pipeline_path.write_text(
        """
steps:
  - id: preprocess
    name: Preprocess
    command: "echo {input_file}"
    inputs:
      - key: input_file
        type: file
        required: true
""",
        encoding="utf-8",
    )

    pipeline = parse_pipeline_file(pipeline_path)

    assert pipeline.steps[0].id == "preprocess"
    assert pipeline.steps[0].inputs[0].required is True


def test_parse_pipeline_json(tmp_path):
    pipeline_path = tmp_path / "pipeline.json"
    pipeline_path.write_text(
        json.dumps({"steps": [{"id": "analyze", "name": "Analyze", "command": "echo ok"}]}),
        encoding="utf-8",
    )

    pipeline = parse_pipeline_file(pipeline_path)

    assert pipeline.steps[0].name == "Analyze"
