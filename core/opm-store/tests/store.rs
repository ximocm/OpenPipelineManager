//! Behavior ports of the pinned Python contracts that survive the
//! pipeline-storage clean break (2026-11-03): step editing, `is_step_ok`,
//! rename propagation, import effects (chaining, source resolution, prune),
//! run.json wire fidelity and the managed-path hardening (F2/F3) — all over
//! a work folder identified solely by `.pipeline-manager/run.json`.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use common::temp_dir;
use opm_pipeline::{
    FieldType, IssueSeverity, OutputSpec, PipelineConfig, PipelineStep, Value, ValueSpec,
    parse_pipeline,
};
use opm_store::{PipelineDoc, Position, ProjectStore, StepState, StepStatus, StoreError};

fn open_store(tag: &str) -> (PathBuf, ProjectStore) {
    let parent = temp_dir(tag);
    let store = ProjectStore::new();
    store
        .create_work(&parent.join("proj"), &PipelineDoc::default())
        .expect("work folder");
    (parent, store)
}

fn cleanup(parent: &Path) {
    let _ = fs::remove_dir_all(parent);
}

/// Parse a `.yml`/`.yaml`/`.json` pipeline file and adopt it into the open
/// work (the new import path: `parse → PipelineDoc → adopt_definition`).
fn import_definition(store: &ProjectStore, file: &Path) -> Result<Vec<String>, StoreError> {
    let root = store.project_root()?;
    let mut path = file.to_path_buf();
    if path.is_relative() {
        path = root.join(&path);
    }
    let text = fs::read_to_string(&path).map_err(StoreError::Io)?;
    let pipeline = parse_pipeline(&path.to_string_lossy(), &text)
        .map_err(|err| StoreError::Message(err.to_string()))?;
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("pipeline");
    let mut doc = PipelineDoc::new(name, pipeline);
    doc.normalize_for_import();
    store.adopt_definition(&doc)?;
    Ok(store.steps().iter().map(|step| step.id.clone()).collect())
}

/// `PipelineStep` as Pydantic would build it from a partial dict (wd default
/// `.`, empty everything else).
fn step(id: &str, name: &str) -> PipelineStep {
    PipelineStep {
        id: id.to_string(),
        name: name.to_string(),
        description: String::new(),
        command: String::new(),
        environment: String::new(),
        working_directory: ".".to_string(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        parameters: Vec::new(),
        dependencies: Vec::new(),
    }
}

fn input_spec(key: &str) -> ValueSpec {
    ValueSpec {
        key: key.to_string(),
        label: None,
        kind: FieldType::Text,
        required: false,
        default: None,
        options: Vec::new(),
        min: None,
        max: None,
        description: String::new(),
        source_step: None,
        source_output: None,
    }
}

fn output_spec(key: &str, path: &str) -> OutputSpec {
    OutputSpec {
        key: key.to_string(),
        label: None,
        path: path.to_string(),
    }
}

fn select_spec(key: &str, default: &str, options: &[&str]) -> ValueSpec {
    ValueSpec {
        kind: FieldType::Select,
        default: Some(Value::Str(default.to_string())),
        options: options.iter().map(|o| o.to_string()).collect(),
        ..input_spec(key)
    }
}

fn str_value(value: &str) -> Value {
    Value::Str(value.to_string())
}

// ---------------------------------------------------------------- round trip

/// Hand-written `run.json`: open → save → reopen must keep `exit_code`,
/// `last_run_at`, positions and the wire field names (`type`, `min`/`max`,
/// `"key": ""`, `{"x","y}` layout, Python `StepState` key order).
#[test]
fn run_json_round_trips_wire_fields() {
    let (parent, store) = open_store("round-trip");
    let root = store.project_root().unwrap();

    fs::write(
        root.join(".pipeline-manager/run.json"),
        r#"{
  "format_version": 1,
  "source": { "name": "demo", "library_file": null },
  "definition": {
    "format_version": 1,
    "name": "demo",
    "description": "hand-written envelope",
    "pipeline": {
      "steps": [
        {
          "id": "a",
          "name": "Analyze",
          "description": "",
          "command": "echo {msg}",
          "environment": "conda:base",
          "working_directory": ".",
          "inputs": [
            {
              "key": "msg",
              "label": null,
              "type": "text",
              "required": false,
              "default": "hello",
              "options": [],
              "min": null,
              "max": null,
              "description": "",
              "source_step": null,
              "source_output": null
            }
          ],
          "outputs": [
            { "key": "", "label": null, "path": "out.txt" },
            { "key": "result", "label": "Result", "path": "outputs/r.txt" }
          ],
          "parameters": [],
          "dependencies": []
        }
      ]
    },
    "layout": { "a": { "x": 123.5, "y": 456.25 } },
    "created_at": "2026-10-27T10:11:12.131415+00:00",
    "updated_at": "2026-10-27T10:11:12.131415+00:00"
  },
  "values": { "a": { "msg": "hello" } },
  "state": {
    "a": {
      "status": "error",
      "selected": true,
      "exit_code": 3,
      "last_run_at": "2026-10-27T10:11:12.131415+00:00",
      "message": "Command exited with 3"
    }
  },
  "created_at": "2026-10-27T10:11:12.131415+00:00",
  "updated_at": "2026-10-27T10:11:12.131415+00:00"
}"#,
    )
    .expect("write run.json");

    store.open_work(&root).expect("open hand-written work");
    let snapshot = store.snapshot();
    assert_eq!(snapshot.params["a"]["msg"], str_value("hello"));
    let state = &snapshot.state["a"];
    assert_eq!(state.status, StepStatus::Error);
    assert!(state.selected);
    assert_eq!(state.exit_code, Some(3));
    assert_eq!(
        state.last_run_at.as_deref(),
        Some("2026-10-27T10:11:12.131415+00:00")
    );
    assert_eq!(state.message, "Command exited with 3");
    assert_eq!(
        snapshot.visual_layout["a"],
        Position {
            x: 123.5,
            y: 456.25
        }
    );
    let step_a = &snapshot.pipeline.as_ref().unwrap().steps[0];
    assert_eq!(step_a.inputs[0].kind, FieldType::Text);
    assert_eq!(step_a.inputs[0].default, Some(str_value("hello")));
    assert_eq!(step_a.outputs[0].key, "");
    assert_eq!(store.definition().name, "demo");
    assert_eq!(store.definition().description, "hand-written envelope");

    store.save_run().expect("save");

    // Reopen with a fresh store: every field survives.
    let reopened = ProjectStore::new();
    reopened.open_work(&root).expect("reopen");
    let snapshot = reopened.snapshot();
    let state = &snapshot.state["a"];
    assert_eq!(state.status, StepStatus::Error);
    assert_eq!(state.exit_code, Some(3));
    assert_eq!(
        state.last_run_at.as_deref(),
        Some("2026-10-27T10:11:12.131415+00:00")
    );
    assert!(state.selected);
    assert_eq!(
        snapshot.visual_layout["a"],
        Position {
            x: 123.5,
            y: 456.25
        }
    );
    assert_eq!(snapshot.params["a"]["msg"], str_value("hello"));
    assert_eq!(
        snapshot.pipeline.as_ref().unwrap().steps[0].outputs[1].key,
        "result"
    );

    // Raw file: single document, Python wire names, StepState key order.
    let raw = fs::read_to_string(root.join(".pipeline-manager/run.json")).unwrap();
    let run: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(run["format_version"], serde_json::json!(1));
    assert_eq!(run["values"]["a"]["msg"], serde_json::json!("hello"));
    let input = &run["definition"]["pipeline"]["steps"][0]["inputs"][0];
    assert_eq!(input["type"], serde_json::json!("text"));
    assert_eq!(input["min"], serde_json::Value::Null);
    assert_eq!(input["max"], serde_json::Value::Null);
    assert!(input.get("source_step").is_some());
    assert_eq!(
        run["definition"]["pipeline"]["steps"][0]["outputs"][0]["key"],
        serde_json::json!("")
    );
    assert_eq!(
        run["definition"]["pipeline"]["steps"][0]["working_directory"],
        serde_json::json!(".")
    );
    let layout_a = run["definition"]["layout"]["a"].as_object().unwrap();
    assert_eq!(layout_a.keys().collect::<Vec<_>>(), ["x", "y"]);
    assert_eq!(layout_a["x"], serde_json::json!(123.5));
    assert_eq!(layout_a["y"], serde_json::json!(456.25));
    assert_eq!(run["state"]["a"]["exit_code"], serde_json::json!(3));
    assert_eq!(run["state"]["a"]["status"], serde_json::json!("error"));
    let pos = |key: &str| raw.find(&format!("\"{key}\"")).expect("key present");
    assert!(
        pos("status") < pos("selected")
            && pos("selected") < pos("exit_code")
            && pos("exit_code") < pos("last_run_at")
            && pos("last_run_at") < pos("message"),
        "state keys out of order: {raw}"
    );

    // No legacy document exists next to run.json (clean break).
    for legacy in [
        "pipeline.project.json",
        ".pipeline-manager/params.json",
        ".pipeline-manager/state.json",
        ".pipeline-manager/visual-layout.json",
        ".pipeline-manager/validation.json",
    ] {
        assert!(!root.join(legacy).exists(), "unexpected legacy {legacy}");
    }

    cleanup(&parent);
}

// -------------------------------------------------------------------- import

/// Fully unconnected imports chain linearly and get the default left-to-right
/// layout (80/410/740 at y=120).
#[test]
fn import_chains_unconnected_steps_and_builds_default_layout() {
    let (parent, store) = open_store("import-linear");
    let root = store.project_root().unwrap();
    fs::write(
        root.join("pipeline.yml"),
        "steps:\n  - id: first\n    name: First\n  - id: second\n    name: Second\n  - id: third\n    name: Third\n",
    )
    .unwrap();

    let ids = import_definition(&store, Path::new("pipeline.yml")).unwrap();
    assert_eq!(ids, ["first", "second", "third"]);

    let steps = store.steps();
    assert!(steps[0].dependencies.is_empty());
    assert_eq!(steps[1].dependencies, ["first"]);
    assert_eq!(steps[2].dependencies, ["second"]);

    // El fallback apila los pasos sin layout de arriba a abajo, en columna.
    let layout = store.snapshot().visual_layout;
    assert_eq!(layout["second"].x, layout["first"].x);
    assert!(layout["second"].y > layout["first"].y);
    assert!(layout["third"].y > layout["second"].y);

    cleanup(&parent);
}

/// Source-linked inputs resolve to the work-relative path from the
/// consuming step's wd (`../output/result.txt`) and append the implicit
/// dependency.
#[test]
fn import_resolves_input_sources_and_appends_implicit_dependency() {
    let (parent, store) = open_store("import-sources");
    let root = store.project_root().unwrap();
    fs::write(
        root.join("pipeline.yml"),
        r#"steps:
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
"#,
    )
    .unwrap();

    import_definition(&store, Path::new("pipeline.yml")).unwrap();

    let consume = store.step("consume").unwrap();
    assert_eq!(
        consume.inputs[0].default,
        Some(str_value("../output/result.txt"))
    );
    assert_eq!(consume.dependencies, ["produce"]);
    assert_eq!(
        store.params_for("consume").get("result_file"),
        Some(&str_value("../output/result.txt"))
    );

    cleanup(&parent);
}

/// Importing a replacement prunes values/state of removed steps and deletes
/// their log and done files (storage.py `_prune_removed_step_runtime`).
#[test]
fn import_prunes_removed_steps_and_deletes_their_artifacts() {
    let (parent, store) = open_store("import-prune");
    let root = store.project_root().unwrap();
    let mut obsolete = step("obsolete", "Obsolete");
    obsolete.command = "echo old".to_string();
    store.create_step(obsolete).unwrap();
    store.append_log("obsolete", "old log");
    store.write_done_marker("obsolete", "2026-10-27T00:00:00.000001+00:00");
    assert!(store.log_path("obsolete").exists());
    assert!(store.done_path("obsolete").exists());

    fs::write(
        root.join("replacement.yml"),
        "steps:\n  - id: current\n    name: Current\n",
    )
    .unwrap();
    import_definition(&store, Path::new("replacement.yml")).unwrap();

    assert_eq!(store.params_for("current"), BTreeMap::new());
    let params = store.snapshot().params;
    assert_eq!(params.keys().collect::<Vec<_>>(), ["current"]);
    let state = store.snapshot().state;
    assert_eq!(state.keys().collect::<Vec<_>>(), ["current"]);
    let layout = store.snapshot().visual_layout;
    assert_eq!(layout.keys().collect::<Vec<_>>(), ["current"]);
    assert!(!store.log_path("obsolete").exists());
    assert!(!store.done_path("obsolete").exists());

    let reopened = ProjectStore::new();
    reopened.open_work(&root).unwrap();
    let snapshot = reopened.snapshot();
    assert_eq!(snapshot.params.keys().collect::<Vec<_>>(), ["current"]);
    assert_eq!(snapshot.state.keys().collect::<Vec<_>>(), ["current"]);
    assert_eq!(
        snapshot.visual_layout.keys().collect::<Vec<_>>(),
        ["current"]
    );

    cleanup(&parent);
}

// ----------------------------------------------------------------- update_step

/// Renaming a step rewrites downstream `source_step` references + implicit
/// dependencies, keeps the resolved default, renames the log/done artifacts
/// and survives a reload.
#[test]
fn update_step_rename_propagates_source_refs_and_artifacts() {
    let (parent, store) = open_store("rename-refs");
    let root = store.project_root().unwrap();
    // As in `test_project_tree.py:377`: the consuming wd must exist, otherwise
    // validation legitimately reports it as a blocker.
    fs::create_dir(root.join("analysis")).unwrap();
    fs::write(
        root.join("pipeline.yml"),
        r#"steps:
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
"#,
    )
    .unwrap();
    import_definition(&store, Path::new("pipeline.yml")).unwrap();
    store.append_log("produce", "old produce log");
    store.write_done_marker("produce", "2026-10-27T00:00:00.000001+00:00");

    let mut producer = store.step("produce").unwrap();
    producer.id = "prepare".to_string();
    store.update_step("produce", producer).unwrap();

    let consumer = store.step("consume").unwrap();
    assert_eq!(consumer.inputs[0].source_step.as_deref(), Some("prepare"));
    assert_eq!(
        consumer.inputs[0].default,
        Some(str_value("../output/result.txt"))
    );
    assert_eq!(consumer.dependencies, ["prepare"]);
    assert!(store.step("produce").is_none());
    assert_eq!(store.log_content("prepare"), "old produce log");
    assert!(!store.log_path("produce").exists());
    assert!(store.done_path("prepare").exists());
    assert!(!store.done_path("produce").exists());
    let issues = store.validate();
    assert!(
        !issues
            .iter()
            .any(|issue| issue.severity == IssueSeverity::Blocker),
        "unexpected blockers: {issues:?}"
    );

    let reopened = ProjectStore::new();
    reopened.open_work(&root).unwrap();
    let consumer = reopened.step("consume").unwrap();
    assert_eq!(consumer.inputs[0].source_step.as_deref(), Some("prepare"));
    assert_eq!(consumer.dependencies, ["prepare"]);

    cleanup(&parent);
}

/// Rename keeps runtime params (existing values win over the new defaults)
/// and migrates environment/command; the old id disappears.
#[test]
fn update_step_rename_keeps_and_migrates_runtime_params() {
    let (parent, store) = open_store("rename-params");
    let mut first = step("first", "First");
    first.command = "echo ok".to_string();
    store.create_step(first).unwrap();
    let mut second = step("second", "Second");
    second.environment = "conda:old".to_string();
    second.parameters = vec![select_spec("mode", "fast", &["fast", "slow"])];
    store.create_step(second).unwrap();
    store
        .set_params(
            "second",
            BTreeMap::from([("mode".to_string(), str_value("slow"))]),
        )
        .unwrap();

    let mut renamed = step("final", "Final");
    renamed.environment = "conda:new".to_string();
    renamed.command = "echo {mode}".to_string();
    renamed.dependencies = vec!["first".to_string()];
    renamed.parameters = vec![select_spec("mode", "fast", &["fast", "slow"])];
    store.update_step("second", renamed).unwrap();

    let updated = store.step("final").unwrap();
    assert_eq!(updated.id, "final");
    assert_eq!(updated.environment, "conda:new");
    assert_eq!(updated.dependencies, ["first"]);
    assert!(store.step("second").is_none());
    assert!(store.params_for("second").is_empty());
    assert_eq!(
        store.params_for("final").get("mode"),
        Some(&str_value("slow"))
    );

    cleanup(&parent);
}

/// Update must keep exact non-empty pipeline paths untouched (whitespace and
/// POSIX backslashes are legal in the text).
#[test]
fn update_step_preserves_exact_nonempty_paths() {
    let (parent, store) = open_store("update-paths");
    let mut write = step("write", "Write");
    store.create_step(write.clone()).unwrap();

    write.working_directory = " work ".to_string();
    let mut source = input_spec("source");
    source.default = Some(str_value(" input.txt "));
    write.inputs = vec![source];
    write.outputs = vec![output_spec("result", " result.txt ")];
    store.update_step("write", write).unwrap();

    let updated = store.step("write").unwrap();
    assert_eq!(updated.working_directory, " work ");
    assert_eq!(updated.inputs[0].default, Some(str_value(" input.txt ")));
    assert_eq!(updated.outputs[0].path, " result.txt ");

    cleanup(&parent);
}

// ---------------------------------------------------------------- create_step

#[test]
fn create_step_applies_defaults_and_appends_to_the_pipeline() {
    let (parent, store) = open_store("create-defaults");
    let root = store.project_root().unwrap();

    let mut first = step("first", "First");
    first.working_directory = String::new();
    first.name = "   ".to_string();
    first.command = "echo ok".to_string();
    store.create_step(first).unwrap();
    let first = store.step("first").unwrap();
    assert_eq!(first.working_directory, "steps/first/work");
    assert_eq!(first.name, "first");
    assert!(first.dependencies.is_empty());
    assert!(root.join("steps/first/work").is_dir());
    assert!(root.join("steps/first/input").is_dir());
    assert!(root.join("steps/first/output").is_dir());

    let mut second = step("second", "Second");
    second.command = "echo {mode}".to_string();
    second.parameters = vec![select_spec("mode", "fast", &["fast", "slow"])];
    store.create_step(second).unwrap();
    let second = store.step("second").unwrap();
    assert_eq!(second.working_directory, ".");
    assert_eq!(second.dependencies, ["first"]);
    assert_eq!(
        store.params_for("second").get("mode"),
        Some(&str_value("fast"))
    );
    assert!(root.join("steps/second/work").is_dir());
    let layout = store.snapshot().visual_layout;
    assert_eq!(layout["second"].x, layout["first"].x);
    assert!(layout["second"].y > layout["first"].y);

    cleanup(&parent);
}

#[test]
fn create_step_rejects_blank_id() {
    let (parent, store) = open_store("create-blank-id");
    let blank = step("", "Blank");
    match store.create_step(blank) {
        Err(StoreError::StepIdRequired) => {}
        other => panic!("expected StepIdRequired, got {other:?}"),
    }
    let whitespace = step("   \t ", "White");
    match store.create_step(whitespace) {
        Err(StoreError::StepIdRequired) => {}
        other => panic!("expected StepIdRequired, got {other:?}"),
    }
    cleanup(&parent);
}

#[test]
fn create_step_rejects_duplicate_id() {
    let (parent, store) = open_store("create-duplicate");
    store.create_step(step("a", "A")).unwrap();
    match store.create_step(step("a", "Again")) {
        Err(StoreError::StepExists(id)) => assert_eq!(id, "a"),
        other => panic!("expected StepExists, got {other:?}"),
    }
    cleanup(&parent);
}

// ------------------------------------------------------------------ set_params

/// Params can never carry undeclared keys: output keys and unknown keys are
/// dropped (storage.py `_runtime_params`).
#[test]
fn set_params_drops_undeclared_keys() {
    let (parent, store) = open_store("set-params");
    let mut write = step("write", "Write");
    write.command = "tool --out {result_file}".to_string();
    write.outputs = vec![output_spec("result_file", "result.txt")];
    write.parameters = vec![input_spec("mode")];
    store.create_step(write).unwrap();

    store
        .set_params(
            "write",
            BTreeMap::from([
                ("result_file".to_string(), str_value("../outside.txt")),
                ("mode".to_string(), str_value("fast")),
                ("bogus".to_string(), str_value("x")),
            ]),
        )
        .unwrap();

    let params = store.params_for("write");
    assert_eq!(params.get("mode"), Some(&str_value("fast")));
    assert!(!params.contains_key("result_file"));
    assert!(!params.contains_key("bogus"));

    cleanup(&parent);
}

// ------------------------------------------------------------------- is_step_ok

#[test]
fn is_step_ok_requires_done_marker_and_outputs() {
    let (parent, store) = open_store("ok-done-outputs");
    let root = store.project_root().unwrap();
    let mut a = step("a", "A");
    a.outputs = vec![output_spec("", "out.txt")];
    store.create_step(a).unwrap();

    assert!(!store.is_step_ok("a"));
    store.write_done_marker("a", "2026-10-27T00:00:00.000001+00:00");
    assert!(!store.is_step_ok("a"));
    fs::write(root.join("out.txt"), "ok").unwrap();
    assert!(store.is_step_ok("a"));

    cleanup(&parent);
}

#[test]
fn is_step_ok_preserves_output_path_whitespace() {
    let (parent, store) = open_store("ok-whitespace");
    let root = store.project_root().unwrap();
    let mut a = step("a", "A");
    a.outputs = vec![output_spec("", " result.txt ")];
    store.create_step(a).unwrap();
    store.write_done_marker("a", "done");

    fs::write(root.join("result.txt"), "wrong file").unwrap();
    assert!(!store.is_step_ok("a"));
    fs::write(root.join(" result.txt "), "expected file").unwrap();
    assert!(store.is_step_ok("a"));

    cleanup(&parent);
}

#[cfg(unix)]
#[test]
fn is_step_ok_preserves_posix_backslashes_in_output_paths() {
    let (parent, store) = open_store("ok-backslash");
    let root = store.project_root().unwrap();
    let mut a = step("a", "A");
    a.outputs = vec![output_spec("", "result\\file.txt")];
    store.create_step(a).unwrap();
    store.write_done_marker("a", "done");

    fs::create_dir_all(root.join("result")).unwrap();
    fs::write(root.join("result/file.txt"), "wrong file").unwrap();
    assert!(!store.is_step_ok("a"));
    fs::write(root.join("result\\file.txt"), "expected file").unwrap();
    assert!(store.is_step_ok("a"));

    cleanup(&parent);
}

#[cfg(unix)]
#[test]
fn is_step_ok_returns_false_for_symlink_loop_output() {
    let (parent, store) = open_store("ok-symlink-loop");
    let root = store.project_root().unwrap();
    std::os::unix::fs::symlink("loop", root.join("loop")).unwrap();
    let mut a = step("a", "A");
    a.outputs = vec![output_spec("", "loop")];
    store.create_step(a).unwrap();
    store.write_done_marker("a", "done");

    assert!(!store.is_step_ok("a"));

    cleanup(&parent);
}

#[test]
fn is_step_ok_ignores_outputs_outside_the_project() {
    let (parent, store) = open_store("ok-outside");
    let outside = parent.join("proj-outside-output.txt");
    fs::write(&outside, "ok").unwrap();
    let mut a = step("a", "A");
    a.outputs = vec![output_spec("", "../proj-outside-output.txt")];
    store.create_step(a).unwrap();
    store.write_done_marker("a", "done");

    assert!(!store.is_step_ok("a"));

    cleanup(&parent);
}

// --------------------------------------------------------------------- open/create

/// A work folder is identified by `run.json` alone: no marker file is
/// required, and a folder without `run.json` fails with a visible error.
#[test]
fn open_work_requires_run_json() {
    let parent = temp_dir("open-requires");
    let store = ProjectStore::new();
    match store.open_work(&parent) {
        Err(StoreError::Message(message)) => {
            assert!(message.contains("run.json"), "message: {message}")
        }
        other => panic!("expected missing run.json error, got {other:?}"),
    }

    let root = parent.join("work");
    store
        .create_work(&root, &PipelineDoc::default())
        .unwrap();
    store.open_work(&root).unwrap();
    assert_eq!(store.project_root().unwrap(), fs::canonicalize(&root).unwrap());

    cleanup(&parent);
}

/// `create_work` scaffolds the runtime folders and writes ONLY `run.json` —
/// no `project.config.json`, no `config.txt`, no legacy snapshot/runtime
/// documents, and a second create never overwrites the existing work.
#[test]
fn create_work_scaffolds_only_run_json() {
    let parent = temp_dir("create-work");
    let store = ProjectStore::new();
    let root = parent.join("work");

    let mut pipeline = PipelineConfig::default();
    pipeline.steps.push(step("alpha", "Alpha"));
    let mut doc = PipelineDoc::new("scaffold", pipeline);
    doc.layout
        .insert("alpha".to_string(), Position { x: 1.0, y: 2.0 });
    store.create_work(&root, &doc).unwrap();

    for dir in [
        "Input",
        "steps",
        "outputs",
        ".pipeline-manager/logs",
        ".pipeline-manager/done",
        "steps/alpha/work",
        "steps/alpha/input",
        "steps/alpha/output",
    ] {
        assert!(root.join(dir).is_dir(), "missing folder {dir}");
    }
    assert!(root.join(".pipeline-manager/run.json").is_file());
    for legacy in [
        "project.config.json",
        "config.txt",
        "pipeline.project.json",
        ".pipeline-manager/params.json",
        ".pipeline-manager/state.json",
        ".pipeline-manager/visual-layout.json",
        ".pipeline-manager/validation.json",
    ] {
        assert!(!root.join(legacy).exists(), "legacy file written: {legacy}");
    }

    let run: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".pipeline-manager/run.json")).unwrap())
            .unwrap();
    assert_eq!(run["format_version"], serde_json::json!(1));
    assert_eq!(run["definition"]["name"], serde_json::json!("scaffold"));
    assert_eq!(store.definition().layout["alpha"], Position { x: 1.0, y: 2.0 });

    match store.create_work(&root, &PipelineDoc::default()) {
        Err(StoreError::Message(message)) => {
            assert!(message.contains("run.json"), "message: {message}")
        }
        other => panic!("expected existing work error, got {other:?}"),
    }

    cleanup(&parent);
}

// ------------------------------------------------------------------ state lifecycle

/// Stale `ok` (output vanished while the marker remains) downgrades to
/// `pending` with `exit_code = None` on load — but `last_run_at` is kept.
#[test]
fn open_downgrades_stale_ok_state_when_output_disappears() {
    let (parent, store) = open_store("stale-ok");
    let root = store.project_root().unwrap();
    let mut write = step("write", "Write");
    write.outputs = vec![output_spec("", "result.txt")];
    store.create_step(write).unwrap();
    fs::write(root.join("result.txt"), "result").unwrap();
    store.write_done_marker("write", "2026-10-27T00:00:00.000001+00:00");
    store.set_step_state(
        "write",
        StepState {
            status: StepStatus::Ok,
            selected: false,
            exit_code: Some(0),
            last_run_at: Some("2026-10-27T00:00:00.000001+00:00".to_string()),
            message: String::new(),
        },
    );
    fs::remove_file(root.join("result.txt")).unwrap();

    let reopened = ProjectStore::new();
    reopened.open_work(&root).unwrap();
    let state = reopened.state_of("write");
    assert_eq!(state.status, StepStatus::Pending);
    assert_eq!(state.exit_code, None);
    assert_eq!(state.message, "");
    assert_eq!(
        state.last_run_at.as_deref(),
        Some("2026-10-27T00:00:00.000001+00:00")
    );

    cleanup(&parent);
}

// -------------------------------------------------------------- set_dependencies

#[test]
fn set_dependencies_updates_validates_and_rejects_bad_references() {
    let (parent, store) = open_store("set-deps");
    store.create_step(step("first", "First")).unwrap();
    store.create_step(step("second", "Second")).unwrap();

    store.set_dependencies("second", Vec::new()).unwrap();
    assert!(store.step("second").unwrap().dependencies.is_empty());

    store
        .set_dependencies("second", vec!["first".to_string(), "first".to_string()])
        .unwrap();
    assert_eq!(store.step("second").unwrap().dependencies, ["first"]);

    match store.set_dependencies("second", vec!["missing".to_string()]) {
        Err(StoreError::UnknownDependency(dep)) => assert_eq!(dep, "missing"),
        other => panic!("expected UnknownDependency, got {other:?}"),
    }
    match store.set_dependencies("second", vec!["second".to_string()]) {
        Err(StoreError::SelfDependency) => {}
        other => panic!("expected SelfDependency, got {other:?}"),
    }

    cleanup(&parent);
}

// ------------------------------------------------------------ selection + layout

#[test]
fn selection_and_layout_persist_across_reopen() {
    let (parent, store) = open_store("persist-selection");
    let root = store.project_root().unwrap();
    store.create_step(step("first", "First")).unwrap();

    let state = store.toggle_selected("first").unwrap();
    assert!(state.selected);
    store
        .set_layout(HashMap::from([("first".to_string(), (7.5, 9.25))]))
        .unwrap();

    let reopened = ProjectStore::new();
    reopened.open_work(&root).unwrap();
    assert!(reopened.state_of("first").selected);
    assert_eq!(
        reopened.snapshot().visual_layout["first"],
        Position {
            x: 7.5,
            y: 9.25
        }
    );

    cleanup(&parent);
}

// --------------------------------------------- managed-path hardening (F2/F3)

/// Un error de ruta gestionada es visible y claro: contención («outside…»,
/// el enlace apunta fuera) o componente-symlink («symbolic link», apunta
/// dentro) — nunca se sigue el enlace.
#[cfg(unix)]
fn assert_managed_path_error(text: &str) {
    assert!(
        text.contains("symbolic link") || text.contains("outside the current project"),
        "mensaje de ruta gestionada poco claro: {text}"
    );
}

/// F2 (escenario 2): `.pipeline-manager/logs/a.log` enlazado a un fichero
/// de usuario — preflight con error claro, sinks que jamás siguen el
/// enlace y cero escrituras fuera del proyecto.
#[cfg(unix)]
#[test]
fn symlinked_log_artifact_is_never_followed() {
    let (parent, store) = open_store("log-symlink");
    let root = store.project_root().unwrap();
    store.create_step(step("a", "A")).unwrap();

    let outside = parent.join("outside-log.txt");
    fs::write(&outside, "keep me").unwrap();
    std::os::unix::fs::symlink(&outside, root.join(".pipeline-manager/logs/a.log")).unwrap();

    // Preflight del motor: error visible y nada arranca.
    let err = store.ensure_step_artifacts_safe("a").unwrap_err();
    assert_managed_path_error(&err.to_string());

    // Escritura/lectura del artefacto: cero efectos fuera.
    store.append_log("a", "attack line\n");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep me");
    assert_eq!(store.log_content("a"), "");
    cleanup(&parent);
}

/// F2 (escenario 3): `run.json` con enlace colgante hacia fuera — guardar y
/// cargar fallan con error claro y NUNCA crean el fichero de fuera.
#[cfg(unix)]
#[test]
fn dangling_symlink_runtime_json_fails_load_and_save() {
    let (parent, store) = open_store("dangling-json");
    let root = store.project_root().unwrap();
    let outside = parent.join("outside-run.json");

    fs::remove_file(root.join(".pipeline-manager/run.json")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join(".pipeline-manager/run.json")).unwrap();

    // save: la ruta gestionada se rechaza antes de abrir.
    let err = store.save_run().unwrap_err();
    assert_managed_path_error(&err.to_string());
    assert!(!outside.exists(), "nunca debe crearse el destino de fuera");

    // load: `exists()` no lo trataría como ausente → también error claro.
    let reopened = ProjectStore::new();
    let err = reopened.open_work(&root).unwrap_err();
    assert_managed_path_error(&err.to_string());
    assert!(!outside.exists());
    cleanup(&parent);
}

/// F3: los nombres de artefacto mapean `/`, `\` y `:` a `_` (el mapeo `/`
/// idéntico a Python) — planos en cualquier plataforma.
#[test]
fn artifact_names_map_every_path_separator_to_underscore() {
    let (parent, store) = open_store("artifact-names");
    let root = store.project_root().unwrap();
    for (id, expected_log, expected_done) in [
        ("a/b", "a_b.log", "a_b.done"),
        ("..\\..\\x", ".._.._x.log", ".._.._x.done"),
        ("C:\\Users\\victim\\x", "C__Users_victim_x.log", "C__Users_victim_x.done"),
        ("plain", "plain.log", "plain.done"),
    ] {
        let log = store.log_path(id);
        assert_eq!(
            log.file_name().and_then(|name| name.to_str()),
            Some(expected_log),
            "id {id:?}"
        );
        let done = store.done_path(id);
        assert_eq!(
            done.file_name().and_then(|name| name.to_str()),
            Some(expected_done),
            "id {id:?}"
        );
        assert!(log.starts_with(root.join(".pipeline-manager/logs")));
        assert!(done.starts_with(root.join(".pipeline-manager/done")));
    }
    cleanup(&parent);
}

/// F3 (PoCs del revisor en importación): los dos ids con separadores
/// producen bloqueos visibles que nombran el paso, y los workspaces creados
/// siguen siendo planos dentro del proyecto.
#[test]
fn imported_separator_ids_produce_blockers_at_import() {
    let (parent, store) = open_store("import-sep-ids");
    let root = store.project_root().unwrap();
    fs::write(
        root.join("pipeline.yml"),
        "steps:\n  - id: \"C:\\\\Users\\\\victim\\\\x\"\n    name: Windows\n    working_directory: \".\"\n  - id: \"..\\\\..\\\\x\"\n    name: Relative\n    working_directory: \".\"\n",
    )
    .unwrap();

    let ids = import_definition(&store, Path::new("pipeline.yml")).unwrap();
    assert_eq!(ids, ["C:\\Users\\victim\\x", "..\\..\\x"]);

    let issues = store.validate();
    for id in ["C:\\Users\\victim\\x", "..\\..\\x"] {
        assert!(
            issues.iter().any(|issue| issue.severity == IssueSeverity::Blocker
                && issue.field.as_deref() == Some("id")
                && issue.message == format!("Step id contains a path separator: {}", id)),
            "missing blocker para {id:?}: {issues:?}"
        );
    }
    // Los workspaces son planos y contenidos (safe_segment + F2).
    assert!(root.join("steps/C_Users_victim_x").is_dir());
    assert!(root.join("steps/x").is_dir());
    cleanup(&parent);
}

/// F2/F3 (prune): `.pipeline-manager/logs` enlazado hacia fuera con un
/// `.log` de fuera dentro — la adopción NO borra nada fuera y el propio
/// guardado falla con error claro.
#[cfg(unix)]
#[test]
fn prune_never_deletes_through_a_symlinked_logs_dir() {
    let (parent, store) = open_store("prune-symlink");
    let root = store.project_root().unwrap();
    let mut obsolete = step("obsolete", "Obsolete");
    obsolete.command = "echo old".to_string();
    store.create_step(obsolete).unwrap();

    let outside = parent.join("prune-outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("obsolete.log"), "outside log").unwrap();
    fs::remove_dir_all(root.join(".pipeline-manager/logs")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join(".pipeline-manager/logs")).unwrap();

    fs::write(
        root.join("replacement.yml"),
        "steps:\n  - id: current\n    name: Current\n",
    )
    .unwrap();
    let err = import_definition(&store, Path::new("replacement.yml")).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("outside the current project") || text.contains("symbolic link"),
        "{text}"
    );
    // Cero escrituras/borrados fuera.
    assert_eq!(
        fs::read_to_string(outside.join("obsolete.log")).unwrap(),
        "outside log"
    );
    cleanup(&parent);
}

/// F3 (prune con ids hostiles metidos a mano en el runtime): solo se borra
/// dentro de `logs` (nombre aplanado F3) y ningún sentinel de fuera cambia.
#[test]
fn prune_of_hostile_runtime_ids_stays_inside_the_project() {
    let (parent, store) = open_store("prune-hostile");
    let root = store.project_root().unwrap();
    let run_path = root.join(".pipeline-manager/run.json");
    let mut run: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&run_path).unwrap()).unwrap();
    run["state"][r"C:\victim\x"] = serde_json::json!({});
    run["state"][r"..\..\x"] = serde_json::json!({});
    run["values"][r"C:\victim\x"] = serde_json::json!({});
    run["values"][r"..\..\x"] = serde_json::json!({});
    fs::write(
        &run_path,
        serde_json::to_string_pretty(&run).unwrap(),
    )
    .unwrap();

    let reopened = ProjectStore::new();
    reopened.open_work(&root).unwrap();
    assert_eq!(reopened.snapshot().params.len(), 2);
    assert_eq!(reopened.snapshot().state.len(), 2);

    fs::write(parent.join("victim_x.log"), "keep").unwrap();
    fs::write(
        root.join(".pipeline-manager/logs/C__victim_x.log"),
        "inside",
    )
    .unwrap();

    fs::write(
        root.join("replacement.yml"),
        "steps:\n  - id: current\n    name: Current\n",
    )
    .unwrap();
    import_definition(&reopened, Path::new("replacement.yml")).unwrap();

    // El artefacto interior (nombre aplanado) se poda; el de fuera no existe.
    assert!(!root
        .join(".pipeline-manager/logs/C__victim_x.log")
        .exists());
    assert_eq!(fs::read_to_string(parent.join("victim_x.log")).unwrap(), "keep");
    assert!(!parent.join("victim_x.done").exists());
    // Los ids hostiles desaparecen del estado y los valores.
    assert_eq!(
        reopened.snapshot().params.keys().collect::<Vec<_>>(),
        ["current"]
    );
    assert_eq!(
        reopened.snapshot().state.keys().collect::<Vec<_>>(),
        ["current"]
    );
    cleanup(&parent);
}
