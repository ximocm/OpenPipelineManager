from __future__ import annotations

from typing import Any

from fastapi.testclient import TestClient

from app.api import routes
from app.main import app
from app.models.pipeline import PipelineConfig
from app.services.execution import ExecutionManager
from app.services.storage import ProjectStore


def configure_api_pipeline(tmp_path, pipeline: dict[str, Any]) -> TestClient:
    routes.store = ProjectStore()
    routes.executor = ExecutionManager(routes.store)
    routes.store.create_project(tmp_path)
    routes.store.pipeline_config = PipelineConfig.model_validate(pipeline)
    return TestClient(app)


def csrf_headers(client: TestClient) -> dict[str, str]:
    token = client.get("/api/security/csrf-token").json()["token"]
    return {"X-OPM-CSRF-Token": token}


def test_run_step_with_blocker_validation_returns_400_without_thread(tmp_path):
    marker = tmp_path / "should_not_run.txt"
    client = configure_api_pipeline(
        tmp_path,
        {
            "steps": [
                {
                    "id": "blocked",
                    "name": "Blocked",
                    "command": f"printf started > {marker.name}",
                    "inputs": [{"key": "input_file", "type": "file", "required": True}],
                }
            ]
        },
    )

    response = client.post("/api/steps/blocked/run", headers=csrf_headers(client))

    assert response.status_code == 400
    detail = response.json()["detail"]
    assert "Resolve these blockers" in detail["message"]
    assert detail["blockers"][0]["step_id"] == "blocked"
    assert detail["blockers"][0]["field"] == "input_file"
    assert routes.executor.thread is None
    assert not marker.exists()


def test_run_selected_reports_dependency_blockers_without_thread(tmp_path):
    marker = tmp_path / "target_should_not_run.txt"
    client = configure_api_pipeline(
        tmp_path,
        {
            "steps": [
                {
                    "id": "prepare",
                    "name": "Prepare",
                    "command": "printf prepare",
                    "inputs": [{"key": "input_file", "type": "file", "required": True}],
                },
                {
                    "id": "target",
                    "name": "Target",
                    "command": f"printf target > {marker.name}",
                    "dependencies": ["prepare"],
                },
            ]
        },
    )

    response = client.post("/api/steps/run-selected", headers=csrf_headers(client), json={"step_ids": ["target"]})

    assert response.status_code == 400
    detail = response.json()["detail"]
    assert any(blocker["step_id"] == "prepare" and blocker["field"] == "input_file" for blocker in detail["blockers"])
    assert routes.executor.thread is None
    assert not marker.exists()
