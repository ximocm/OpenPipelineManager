from __future__ import annotations

from app.models.pipeline import PipelineStep
from app.services.project_tree import (
    PROJECT_CONFIG_FILENAME,
    create_project_path,
    delete_project_path,
    ensure_project_scaffold,
    ensure_step_folders,
    move_project_path,
    rename_project_path,
    upload_project_file,
    validate_project_folder,
    preview_file,
    safe_segment,
    write_project_file,
)
from app.services.storage import ProjectStore


def test_project_scaffold_creates_workspace_dirs(tmp_path):
    ensure_project_scaffold(tmp_path)

    assert (tmp_path / "Input").is_dir()
    assert (tmp_path / "steps").is_dir()
    assert (tmp_path / "outputs").is_dir()
    assert (tmp_path / "config.txt").is_file()
    assert (tmp_path / PROJECT_CONFIG_FILENAME).is_file()


def test_step_folders_are_created_from_ids(tmp_path):
    ensure_project_scaffold(tmp_path)
    ensure_step_folders(tmp_path, ["prepare-data"])

    assert (tmp_path / "steps" / "prepare-data" / "input").is_dir()
    assert (tmp_path / "steps" / "prepare-data" / "work").is_dir()
    assert (tmp_path / "steps" / "prepare-data" / "output").is_dir()


def test_file_create_rename_move_delete(tmp_path):
    ensure_project_scaffold(tmp_path)
    create_project_path(tmp_path, "Input/sample.txt", "hello")

    renamed = rename_project_path(tmp_path, "Input/sample.txt", "renamed.txt")
    assert renamed.relative_to(tmp_path).as_posix() == "Input/renamed.txt"

    moved = move_project_path(tmp_path, "Input/renamed.txt", "outputs")
    assert moved.relative_to(tmp_path).as_posix() == "outputs/renamed.txt"

    delete_project_path(tmp_path, "outputs/renamed.txt")
    assert not (tmp_path / "outputs" / "renamed.txt").exists()


def test_write_project_file_updates_text_content(tmp_path):
    ensure_project_scaffold(tmp_path)
    create_project_path(tmp_path, "Input/config.yml", "old")

    written = write_project_file(tmp_path, "Input/config.yml", "new")

    assert written.relative_to(tmp_path).as_posix() == "Input/config.yml"
    assert (tmp_path / "Input" / "config.yml").read_text(encoding="utf-8") == "new"


def test_create_project_path_rejects_existing_path(tmp_path):
    ensure_project_scaffold(tmp_path)
    create_project_path(tmp_path, "Input/sample.txt", "hello")

    try:
        create_project_path(tmp_path, "Input/sample.txt", "again")
    except FileExistsError as exc:
        assert "already exists" in str(exc)
    else:
        raise AssertionError("Expected duplicate path to be rejected")


def test_preview_opens_fasta_as_text(tmp_path):
    ensure_project_scaffold(tmp_path)
    create_project_path(tmp_path, "Input/core.fasta", ">seq1\nACGT\n")

    preview = preview_file(tmp_path, "Input/core.fasta")

    assert preview.type == "text"
    assert preview.content == ">seq1\nACGT\n"


def test_preview_keeps_binary_files_closed(tmp_path):
    ensure_project_scaffold(tmp_path)
    (tmp_path / "Input" / "data.bin").write_bytes(b"\x00\x01\x02")

    preview = preview_file(tmp_path, "Input/data.bin")

    assert preview.type == "binary"


def test_preview_opens_png_as_image(tmp_path):
    ensure_project_scaffold(tmp_path)
    png_header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"
    (tmp_path / "Input" / "plot.png").write_bytes(png_header)

    preview = preview_file(tmp_path, "Input/plot.png")

    assert preview.type == "image"
    assert preview.media_type == "image/png"
    assert preview.content


def test_upload_file_rejects_path_escape(tmp_path):
    ensure_project_scaffold(tmp_path)

    try:
        upload_project_file(tmp_path, "..", "bad.txt", "aGVsbG8=")
    except ValueError as exc:
        assert "outside" in str(exc)
    else:
        raise AssertionError("Expected path escape to be rejected")


def test_validate_project_folder_requires_project_config(tmp_path):
    try:
        validate_project_folder(tmp_path)
    except ValueError as exc:
        assert PROJECT_CONFIG_FILENAME in str(exc)
    else:
        raise AssertionError("Expected missing project config to be rejected")

    ensure_project_scaffold(tmp_path)
    validate_project_folder(tmp_path)


def test_project_store_open_requires_existing_project(tmp_path):
    store = ProjectStore()

    try:
        store.open_project(tmp_path)
    except ValueError as exc:
        assert PROJECT_CONFIG_FILENAME in str(exc)
    else:
        raise AssertionError("Expected open project to reject folders without project config")

    store.create_project(tmp_path)
    snapshot = store.open_project(tmp_path)
    assert snapshot.project_path == str(tmp_path)


def test_safe_segment_replaces_spaces_with_underscores():
    assert safe_segment("My Pipeline Project") == "My_Pipeline_Project"
    assert safe_segment(" bad / name ") == "bad_name"


def test_create_project_sanitizes_folder_name(tmp_path):
    store = ProjectStore()
    snapshot = store.create_project(tmp_path / "My Pipeline Project")

    assert snapshot.project_path == str(tmp_path / "My_Pipeline_Project")
    assert (tmp_path / "My_Pipeline_Project" / PROJECT_CONFIG_FILENAME).is_file()


def test_import_pipeline_creates_left_to_right_layout(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    pipeline_path = tmp_path / "pipeline.yml"
    pipeline_path.write_text(
        """
steps:
  - id: first
    name: First
  - id: second
    name: Second
  - id: third
    name: Third
""",
        encoding="utf-8",
    )

    store.import_pipeline(pipeline_path)

    assert store.get_step("first").dependencies == []
    assert store.get_step("second").dependencies == ["first"]
    assert store.get_step("third").dependencies == ["second"]
    assert store.snapshot().visual_layout == {
        "first": {"x": 80.0, "y": 120.0},
        "second": {"x": 410.0, "y": 120.0},
        "third": {"x": 740.0, "y": 120.0},
    }


def test_import_pipeline_resolves_input_sources(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    pipeline_path = tmp_path / "pipeline.yml"
    pipeline_path.write_text(
        """
steps:
  - id: produce
    name: Produce
    outputs:
      - key: result
        path: output/result.txt
  - id: consume
    name: Consume
    working_directory: analysis
    inputs:
      - key: result_file
        type: file
        source_step: produce
        source_output: result
""",
        encoding="utf-8",
    )

    store.import_pipeline(pipeline_path)

    consume = store.get_step("consume")
    assert consume.inputs[0].default == "../output/result.txt"
    assert consume.dependencies == ["produce"]


def test_create_step_appends_to_pipeline_with_default_dependency(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)

    first = store.create_step(PipelineStep.model_validate({"id": "first", "name": "First", "command": "echo ok"}))
    second = store.create_step(
        PipelineStep.model_validate(
            {
                "id": "second",
                "name": "Second",
                "command": "echo {mode}",
                "parameters": [{"key": "mode", "type": "select", "default": "fast", "options": ["fast", "slow"]}],
            }
        )
    )

    assert first.dependencies == []
    assert second.dependencies == ["first"]
    assert store.params["second"] == {"mode": "fast"}
    assert (tmp_path / "steps" / "second" / "work").is_dir()
    assert store.snapshot().visual_layout["second"] == {"x": 410.0, "y": 120.0}


def test_set_step_dependencies_updates_pipeline(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.create_step(PipelineStep.model_validate({"id": "first", "name": "First"}))
    store.create_step(PipelineStep.model_validate({"id": "second", "name": "Second"}))

    updated = store.set_step_dependencies("second", [])

    assert updated.dependencies == []
    assert store.get_step("second").dependencies == []


def test_update_step_renames_references_and_keeps_runtime_data(tmp_path):
    store = ProjectStore()
    store.create_project(tmp_path)
    store.create_step(PipelineStep.model_validate({"id": "first", "name": "First"}))
    store.create_step(
        PipelineStep.model_validate(
            {
                "id": "second",
                "name": "Second",
                "environment": "conda:old",
                "parameters": [{"key": "mode", "type": "select", "default": "fast", "options": ["fast", "slow"]}],
            }
        )
    )
    store.params["second"]["mode"] = "slow"

    updated = store.update_step(
        "second",
        PipelineStep.model_validate(
            {
                "id": "final",
                "name": "Final",
                "environment": "conda:new",
                "command": "echo {mode}",
                "dependencies": ["first"],
                "parameters": [{"key": "mode", "type": "select", "default": "fast", "options": ["fast", "slow"]}],
            }
        ),
    )

    assert updated.id == "final"
    assert updated.environment == "conda:new"
    assert store.get_step("final").dependencies == ["first"]
    assert "second" not in store.params
    assert store.params["final"]["mode"] == "slow"
