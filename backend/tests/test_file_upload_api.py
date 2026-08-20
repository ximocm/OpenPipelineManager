from __future__ import annotations

from fastapi.testclient import TestClient

from app.api import routes
from app.main import app


def test_upload_returns_conflict_for_existing_file(tmp_path):
    routes.store.create_project(tmp_path)
    existing = tmp_path / "Input" / "existing.txt"
    existing.write_text("keep", encoding="utf-8")

    response = TestClient(app).post(
        "/api/projects/files/upload",
        json={"target_directory": "Input", "files": [{"name": "existing.txt", "content_base64": "bmV3"}]},
    )

    assert response.status_code == 409
    assert existing.read_text(encoding="utf-8") == "keep"


def test_multi_file_upload_preflights_conflicts_before_writing(tmp_path):
    routes.store.create_project(tmp_path)
    existing = tmp_path / "Input" / "existing.txt"
    existing.write_text("keep", encoding="utf-8")

    response = TestClient(app).post(
        "/api/projects/files/upload",
        json={
            "target_directory": "Input",
            "files": [
                {"name": "first.txt", "content_base64": "Zmlyc3Q="},
                {"name": "existing.txt", "content_base64": "bmV3"},
            ],
        },
    )

    assert response.status_code == 409
    assert not (tmp_path / "Input" / "first.txt").exists()
    assert existing.read_text(encoding="utf-8") == "keep"


def test_multi_file_upload_rejects_duplicate_destinations_before_writing(tmp_path):
    routes.store.create_project(tmp_path)

    response = TestClient(app).post(
        "/api/projects/files/upload",
        json={
            "target_directory": "Input",
            "files": [
                {"name": "duplicate.txt", "content_base64": "Zmlyc3Q="},
                {"name": " duplicate.txt ", "content_base64": "c2Vjb25k"},
            ],
        },
    )

    assert response.status_code == 409
    assert not (tmp_path / "Input" / "duplicate.txt").exists()
