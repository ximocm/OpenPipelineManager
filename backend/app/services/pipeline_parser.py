from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import yaml

from app.models.pipeline import PipelineConfig


def parse_pipeline_data(data: dict[str, Any]) -> PipelineConfig:
    return PipelineConfig.model_validate(data)


def parse_pipeline_file(path: Path | str) -> PipelineConfig:
    pipeline_path = Path(path)
    suffix = pipeline_path.suffix.lower()
    raw = pipeline_path.read_text(encoding="utf-8")

    if suffix == ".json":
        data = json.loads(raw)
    elif suffix in {".yml", ".yaml"}:
        data = yaml.safe_load(raw) or {}
    else:
        raise ValueError(f"Unsupported pipeline format: {pipeline_path.suffix}")

    if not isinstance(data, dict):
        raise ValueError("Pipeline file must contain an object at the top level")
    return parse_pipeline_data(data)
