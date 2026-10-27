from __future__ import annotations

import os
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.api import routes
from app.main import app


def _symlink_or_skip(link: Path, target: Path) -> None:
    try:
        link.symlink_to(target)
    except OSError as exc:
        pytest.skip(f"Symlinks are unavailable in this environment: {exc}")


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


def test_multi_file_upload_rejects_exact_duplicate_destinations_before_writing(tmp_path):
    routes.store.create_project(tmp_path)

    response = TestClient(app).post(
        "/api/projects/files/upload",
        json={
            "target_directory": "Input",
            "files": [
                {"name": "duplicate.txt", "content_base64": "Zmlyc3Q="},
                {"name": "duplicate.txt", "content_base64": "c2Vjb25k="},
            ],
        },
    )

    assert response.status_code == 409
    assert not (tmp_path / "Input" / "duplicate.txt").exists()


@pytest.mark.skipif(
    os.path.normcase("report.txt") != os.path.normcase("REPORT.txt"),
    reason="platform normcase does not fold case for this filesystem",
)
def test_multi_file_upload_rejects_case_equivalent_duplicate_destinations(tmp_path):
    routes.store.create_project(tmp_path)

    response = TestClient(app).post(
        "/api/projects/files/upload",
        json={
            "target_directory": "Input",
            "files": [
                {"name": "report.txt", "content_base64": "Zmlyc3Q="},
                {"name": "REPORT.txt", "content_base64": "c2Vjb25k="},
            ],
        },
    )

    assert response.status_code == 409
    assert not (tmp_path / "Input" / "report.txt").exists()
    assert not (tmp_path / "Input" / "REPORT.txt").exists()


def test_multi_file_upload_invalid_base64_in_later_file_writes_nothing(tmp_path):
    routes.store.create_project(tmp_path)
    sentinel = tmp_path / "Input" / "keep.txt"
    sentinel.write_text("keep", encoding="utf-8")

    response = TestClient(app).post(
        "/api/projects/files/upload",
        json={
            "target_directory": "Input",
            "files": [
                {"name": "first.txt", "content_base64": "Zmlyc3Q="},
                {"name": "broken.txt", "content_base64": "not-base64!!"},
            ],
        },
    )

    assert response.status_code == 400
    assert not (tmp_path / "Input" / "first.txt").exists()
    assert not (tmp_path / "Input" / "broken.txt").exists()
    assert sentinel.read_text(encoding="utf-8") == "keep"


def test_multi_file_upload_writes_every_file(tmp_path):
    routes.store.create_project(tmp_path)

    response = TestClient(app).post(
        "/api/projects/files/upload",
        json={
            "target_directory": "Input",
            "files": [
                {"name": "first.txt", "content_base64": "Zmlyc3Q="},
                {"name": "second.txt", "content_base64": "c2Vjb25k="},
            ],
        },
    )

    assert response.status_code == 200
    assert response.json() == {"paths": ["Input/first.txt", "Input/second.txt"]}
    assert (tmp_path / "Input" / "first.txt").read_bytes() == b"first"
    assert (tmp_path / "Input" / "second.txt").read_bytes() == b"second"


def test_upload_rejects_names_escaping_target_directory(tmp_path):
    routes.store.create_project(tmp_path)
    client = TestClient(app)
    entries_before = sorted(path.name for path in tmp_path.iterdir())

    name_response = client.post(
        "/api/projects/files/upload",
        json={"target_directory": "Input", "files": [{"name": "..", "content_base64": "bmV3"}]},
    )
    directory_response = client.post(
        "/api/projects/files/upload",
        json={"target_directory": "..", "files": [{"name": "escape.txt", "content_base64": "bmV3"}]},
    )

    assert name_response.status_code == 400
    assert directory_response.status_code == 400
    assert sorted(path.name for path in tmp_path.iterdir()) == entries_before
    assert not (tmp_path.parent / "escape.txt").exists()


def test_upload_rejects_dangling_symlink_destination(tmp_path):
    routes.store.create_project(tmp_path)
    link = tmp_path / "Input" / "linked.txt"
    _symlink_or_skip(link, tmp_path / "missing-target.txt")

    response = TestClient(app).post(
        "/api/projects/files/upload",
        json={"target_directory": "Input", "files": [{"name": "linked.txt", "content_base64": "bmV3"}]},
    )

    assert response.status_code == 409
    assert link.is_symlink()


def test_upload_rejects_symlinked_destination_without_touching_target(tmp_path):
    project_root = tmp_path / "project"
    routes.store.create_project(project_root)
    outside = tmp_path / "outside.txt"
    outside.write_text("original", encoding="utf-8")
    link = project_root / "Input" / "linked.txt"
    _symlink_or_skip(link, outside)

    response = TestClient(app).post(
        "/api/projects/files/upload",
        json={"target_directory": "Input", "files": [{"name": "linked.txt", "content_base64": "bmV3"}]},
    )

    assert response.status_code == 409
    assert link.is_symlink()
    assert outside.read_text(encoding="utf-8") == "original"
