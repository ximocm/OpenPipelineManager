from __future__ import annotations

import time

from fastapi.testclient import TestClient

from app.api import routes
from app.main import app
from app.models.pipeline import PipelineConfig


def configure_pipeline(tmp_path):
    routes.store.create_project(tmp_path)
    marker = tmp_path / "csrf_marker.txt"
    routes.store.pipeline_config = PipelineConfig.model_validate(
        {
            "steps": [
                {
                    "id": "csrf_step",
                    "name": "CSRF Step",
                    "command": f"printf csrf-triggered > {marker.name}",
                }
            ]
        }
    )
    return marker


def test_run_selected_rejects_form_post_without_csrf_token(tmp_path):
    marker = configure_pipeline(tmp_path)
    client = TestClient(app)

    response = client.post(
        "/api/steps/run-selected",
        headers={"Origin": "http://attacker.example", "Content-Type": "application/x-www-form-urlencoded"},
        content=b"",
    )

    assert response.status_code == 403
    time.sleep(0.1)
    assert not marker.exists()


def test_run_selected_requires_json_body_even_with_csrf_token(tmp_path):
    marker = configure_pipeline(tmp_path)
    client = TestClient(app)
    token = client.get("/api/security/csrf-token").json()["token"]

    response = client.post("/api/steps/run-selected", headers={"X-OPM-CSRF-Token": token})

    assert response.status_code == 422
    time.sleep(0.1)
    assert not marker.exists()


def test_run_selected_accepts_trusted_frontend_csrf_header(tmp_path):
    marker = configure_pipeline(tmp_path)
    client = TestClient(app)
    token = client.get("/api/security/csrf-token").json()["token"]

    response = client.post(
        "/api/steps/run-selected",
        headers={"X-OPM-CSRF-Token": token},
        json={"step_ids": ["csrf_step"]},
    )

    assert response.status_code == 200
    for _ in range(20):
        if marker.exists():
            break
        time.sleep(0.1)
    assert marker.read_text(encoding="utf-8") == "csrf-triggered"
