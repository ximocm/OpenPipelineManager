//! Work-folder envelope behavior (pipeline-storage, 2026-11-03): isolation
//! between work folders sharing one library document, interrupted writes,
//! moved folders, explicit adoption of a changed definition and rejection of
//! unsupported schema versions.

mod common;

use std::fs;
use std::path::Path;

use common::temp_dir;
use opm_pipeline::{FieldType, OutputSpec, PipelineConfig, PipelineStep, Value, ValueSpec};
use opm_store::{
    PipelineDoc, PipelineLibrary, ProjectStore, SaveMode, StepState, StepStatus, StoreError,
    ValueMap,
};

const STAMP: &str = "2026-11-03T00:00:00+00:00";

fn cleanup(parent: &Path) {
    let _ = fs::remove_dir_all(parent);
}

fn str_value(value: &str) -> Value {
    Value::Str(value.to_string())
}

fn plain_step(id: &str, command: &str) -> PipelineStep {
    PipelineStep {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        command: command.to_string(),
        environment: String::new(),
        working_directory: ".".to_string(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        parameters: Vec::new(),
        dependencies: Vec::new(),
    }
}

fn param_step(id: &str, command: &str, key: &str, default: &str) -> PipelineStep {
    let mut step = plain_step(id, command);
    step.parameters = vec![ValueSpec {
        key: key.to_string(),
        label: None,
        kind: FieldType::Text,
        required: false,
        default: Some(str_value(default)),
        options: Vec::new(),
        min: None,
        max: None,
        description: String::new(),
        source_step: None,
        source_output: None,
    }];
    step
}

fn pipeline_of(steps: Vec<PipelineStep>) -> PipelineConfig {
    PipelineConfig { steps }
}

/// One library document started in two work folders: values, state, `.done`
/// markers and logs never cross over, and editing one folder's values never
/// affects the other.
#[test]
fn work_isolation() {
    let parent = temp_dir("isolation");
    let library = PipelineLibrary::new(parent.join("pipelines"));
    let doc = PipelineDoc::new("shared", pipeline_of(vec![param_step("make", "true", "mode", "fast")]));
    library.save("shared", &doc, SaveMode::CreateNew).unwrap();
    let loaded = library.load("shared").unwrap();

    let first = parent.join("work-one");
    let second = parent.join("work-two");
    let store_one = ProjectStore::new();
    let store_two = ProjectStore::new();
    store_one.create_work(&first, &loaded).unwrap();
    store_two.create_work(&second, &loaded).unwrap();

    // Everything below happens in the first work folder only.
    store_one
        .set_params(
            "make",
            ValueMap::from([("mode".to_string(), str_value("custom-one"))]),
        )
        .unwrap();
    store_one.set_step_state(
        "make",
        StepState {
            status: StepStatus::Ok,
            exit_code: Some(0),
            message: "ran".to_string(),
            ..StepState::default()
        },
    );
    store_one.append_log("make", "one log line\n");
    store_one.write_done_marker("make", STAMP);

    // The second folder shares nothing: default values, pending state, no
    // log and no completion marker on disk.
    assert_eq!(
        store_two.params_for("make").get("mode"),
        Some(&str_value("fast"))
    );
    assert_eq!(store_two.state_of("make").status, StepStatus::Pending);
    assert!(store_two.log_content("make").is_empty());
    assert!(!second.join(".pipeline-manager/done/make.done").exists());
    assert!(!second.join(".pipeline-manager/logs/make.log").exists());
    assert!(first.join(".pipeline-manager/done/make.done").exists());

    // Editing the first folder's values again never reaches the other one.
    store_one
        .set_params(
            "make",
            ValueMap::from([("mode".to_string(), str_value("custom-two"))]),
        )
        .unwrap();
    let reopen_two = ProjectStore::new();
    reopen_two.open_work(&second).unwrap();
    assert_eq!(
        reopen_two.params_for("make").get("mode"),
        Some(&str_value("fast"))
    );
    assert_eq!(reopen_two.state_of("make").status, StepStatus::Pending);
    assert!(reopen_two.log_content("make").is_empty());

    // The first folder keeps its own state after a reload.
    let reopen_one = ProjectStore::new();
    reopen_one.open_work(&first).unwrap();
    assert_eq!(
        reopen_one.params_for("make").get("mode"),
        Some(&str_value("custom-two"))
    );
    assert_eq!(reopen_one.state_of("make").status, StepStatus::Ok);
    assert_eq!(reopen_one.log_content("make"), "one log line\n");

    cleanup(&parent);
}

#[test]
fn work_layout_changes_stay_in_its_embedded_copy() {
    let parent = temp_dir("layout-copy");
    let library = PipelineLibrary::new(parent.join("pipelines"));
    let doc = PipelineDoc::new("shared", pipeline_of(vec![plain_step("make", "true")]));
    library.save("shared", &doc, SaveMode::CreateNew).unwrap();
    let library_file = parent.join("pipelines/shared.pipeline");
    let before = fs::read(&library_file).unwrap();
    let first = ProjectStore::new();
    let second = ProjectStore::new();
    first.create_work(&parent.join("one"), &library.load("shared").unwrap()).unwrap();
    second.create_work(&parent.join("two"), &library.load("shared").unwrap()).unwrap();
    let second_layout = second.snapshot().visual_layout;
    first.set_step_state("make", StepState { status: StepStatus::Running, ..StepState::default() });
    first.set_layout(std::collections::HashMap::from([("make".into(), (320.0, 240.0))])).unwrap();
    let reopened = ProjectStore::new();
    reopened.open_work(&parent.join("one")).unwrap();
    assert_eq!(reopened.snapshot().visual_layout["make"], opm_store::Position { x: 320.0, y: 240.0 });
    assert_eq!(second.snapshot().visual_layout, second_layout);
    assert_eq!(fs::read(&library_file).unwrap(), before);
    assert_eq!(first.state_of("make").status, StepStatus::Running);
    cleanup(&parent);
}

/// An interrupted save (staged write aborted before the rename, or an orphan
/// temp left by a killed process) never damages the previous complete
/// `run.json` or `.pipeline` document — both keep loading.
#[test]
fn interrupted_write_keeps_previous() {
    let parent = temp_dir("interrupted");

    // --- run.json ---
    let root = parent.join("work");
    let store = ProjectStore::new();
    let mut doc = PipelineDoc::new("before", pipeline_of(vec![plain_step("a", "true")]));
    doc.description = "v1".to_string();
    store.create_work(&root, &doc).unwrap();
    // Completion marker present: the `Ok` state survives the reopen (a
    // marker-less `Ok` would legitimately downgrade to `Pending`).
    store.write_done_marker("a", STAMP);
    store.set_step_state(
        "a",
        StepState {
            status: StepStatus::Ok,
            message: "v1".to_string(),
            ..StepState::default()
        },
    );
    let run_path = root.join(".pipeline-manager/run.json");
    let previous = fs::read(&run_path).unwrap();

    // (a) A staged atomic write that dies before the commit (guard dropped).
    let staged = opm_fs::stage_write(&run_path, b"{ \"format_version\": 1, \"na").unwrap();
    drop(staged);
    // (b) An orphan temp a killed process leaves behind.
    fs::write(
        root.join(".pipeline-manager/.run.json.opm-tmp-999999-0"),
        b"{ \"trunc",
    )
    .unwrap();

    assert_eq!(fs::read(&run_path).unwrap(), previous);
    let reopened = ProjectStore::new();
    reopened.open_work(&root).unwrap();
    assert_eq!(reopened.state_of("a").message, "v1");
    assert_eq!(reopened.definition().description, "v1");

    // A real save still works and the orphan temp is never read.
    reopened.set_step_state(
        "a",
        StepState {
            status: StepStatus::Ok,
            message: "v2".to_string(),
            ..StepState::default()
        },
    );
    let again = ProjectStore::new();
    again.open_work(&root).unwrap();
    assert_eq!(again.state_of("a").message, "v2");

    // --- .pipeline: the same guarantee for the library document ---
    let library = PipelineLibrary::new(parent.join("pipelines"));
    let lib_doc = PipelineDoc::new("lib", pipeline_of(vec![plain_step("x", "true")]));
    library.save("lib", &lib_doc, SaveMode::CreateNew).unwrap();
    let lib_path = parent.join("pipelines/lib.pipeline");
    let lib_previous = fs::read(&lib_path).unwrap();

    let staged = opm_fs::stage_write(&lib_path, b"{ \"format_version\":").unwrap();
    drop(staged);
    fs::write(
        parent.join("pipelines/.lib.pipeline.opm-tmp-999999-0"),
        b"{ \"trunc",
    )
    .unwrap();

    assert_eq!(fs::read(&lib_path).unwrap(), lib_previous);
    let loaded = library.load("lib").unwrap();
    assert_eq!(loaded.pipeline.steps[0].id, "x");

    cleanup(&parent);
}

/// Moving the whole work folder keeps it openable: the root is the
/// directory containing `run.json`, state and `.done` semantics hold under
/// the new location, and a stale `source.library_file` never participates.
#[test]
fn moved_work_folder() {
    let parent = temp_dir("moved-work");
    let original = parent.join("original");
    let mut step = param_step("work", "true", "mode", "fast");
    step.outputs = vec![OutputSpec {
        key: "result".to_string(),
        label: None,
        path: "result.txt".to_string(),
    }];

    let store = ProjectStore::new();
    store
        .create_work(&original, &PipelineDoc::new("movable", pipeline_of(vec![step])))
        .unwrap();
    store
        .set_params(
            "work",
            ValueMap::from([("mode".to_string(), str_value("moved-mode"))]),
        )
        .unwrap();
    store.set_step_state(
        "work",
        StepState {
            status: StepStatus::Ok,
            selected: true,
            exit_code: Some(0),
            last_run_at: Some(STAMP.to_string()),
            message: String::new(),
            ..StepState::default()
        },
    );
    store.append_log("work", "moved log\n");
    store.write_done_marker("work", STAMP);
    fs::write(original.join("result.txt"), "ok").unwrap();
    assert!(store.is_step_ok("work"));

    // Record stale provenance: the referenced library file does not exist
    // (and will never be needed to reopen the work).
    let run_path = original.join(".pipeline-manager/run.json");
    let mut run: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&run_path).unwrap()).unwrap();
    run["source"]["library_file"] = serde_json::json!("/nonexistent/library/old.pipeline");
    fs::write(&run_path, serde_json::to_string_pretty(&run).unwrap()).unwrap();

    // Move the whole directory elsewhere — no stored absolute path decides
    // where the work lives.
    let moved = parent.join("elsewhere");
    fs::rename(&original, &moved).unwrap();

    let resumed = ProjectStore::new();
    resumed.open_work(&moved).unwrap();
    assert_eq!(resumed.project_root().unwrap(), fs::canonicalize(&moved).unwrap());
    assert_eq!(resumed.definition().name, "movable");
    let state = resumed.state_of("work");
    assert_eq!(state.status, StepStatus::Ok);
    assert!(state.selected);
    assert_eq!(state.exit_code, Some(0));
    assert_eq!(state.last_run_at.as_deref(), Some(STAMP));
    assert_eq!(
        resumed.params_for("work").get("mode"),
        Some(&str_value("moved-mode"))
    );
    assert_eq!(resumed.log_content("work"), "moved log\n");
    // Completion resolves under the NEW root: marker + declared output both
    // moved with the folder.
    assert!(resumed.is_step_ok("work"));

    cleanup(&parent);
}

/// Explicit adoption of a revised library definition: changed fingerprints
/// are invalidated (`.done` deleted, `Pending`), removed steps are pruned
/// entirely, values are reconciled to the declared keys, unchanged steps
/// keep their `Ok` state — and the library file itself is never rewritten.
#[test]
fn changed_library_adopt() {
    let parent = temp_dir("adopt");
    let library = PipelineLibrary::new(parent.join("pipelines"));

    let doc_v1 = PipelineDoc::new(
        "adopted",
        pipeline_of(vec![
            param_step("alpha", "echo one", "mode", "fast"),
            plain_step("beta", "echo two"),
            param_step("gamma", "echo three", "old_param", "x"),
            param_step("delta", "echo four", "k", "d"),
        ]),
    );
    library
        .save("adopted", &doc_v1, SaveMode::CreateNew)
        .unwrap();

    let root = parent.join("work");
    let store = ProjectStore::new();
    store
        .create_work(&root, &library.load("adopted").unwrap())
        .unwrap();

    // User values + completion state for every step.
    store
        .set_params(
            "alpha",
            ValueMap::from([("mode".to_string(), str_value("slow"))]),
        )
        .unwrap();
    store
        .set_params(
            "gamma",
            ValueMap::from([("old_param".to_string(), str_value("kept"))]),
        )
        .unwrap();
    store
        .set_params(
            "delta",
            ValueMap::from([("k".to_string(), str_value("custom"))]),
        )
        .unwrap();
    for id in ["alpha", "beta", "gamma", "delta"] {
        store.append_log(id, &format!("log of {id}\n"));
        store.write_done_marker(id, STAMP);
        store.set_step_state(
            id,
            StepState {
                status: StepStatus::Ok,
                exit_code: Some(0),
                message: format!("ran {id}"),
                ..StepState::default()
            },
        );
    }
    assert!(store.is_step_ok("alpha"));
    assert!(store.is_step_ok("delta"));

    // Library revision: change one command, drop a step, rename a parameter.
    let mut v2 = doc_v1.clone();
    v2.pipeline.steps[0].command = "echo one-modified".to_string();
    v2.pipeline.steps.retain(|step| step.id != "beta");
    assert_eq!(v2.pipeline.steps[1].id, "gamma");
    v2.pipeline.steps[1].parameters[0].key = "new_param".to_string();

    let library_path = parent.join("pipelines/adopted.pipeline");
    let library_bytes_before = fs::read(&library_path).unwrap();
    let report = store.adopt_definition(&v2).expect("adopt");

    assert_eq!(report.changed, ["alpha", "gamma"]);
    assert_eq!(report.removed, ["beta"]);
    assert!(report.added.is_empty());
    assert_eq!(report.unchanged, ["delta"]);

    // Changed steps: .done deleted, status Pending with cleared
    // exit_code/message; still-declared values are kept.
    let alpha = store.state_of("alpha");
    assert_eq!(alpha.status, StepStatus::Pending);
    assert_eq!(alpha.exit_code, None);
    assert!(alpha.message.is_empty());
    assert!(!store.done_path("alpha").exists());
    assert_eq!(
        store.params_for("alpha").get("mode"),
        Some(&str_value("slow"))
    );
    let gamma = store.state_of("gamma");
    assert_eq!(gamma.status, StepStatus::Pending);
    assert_eq!(gamma.exit_code, None);
    assert!(!store.done_path("gamma").exists());

    // Removed step: pruned entirely — values, state, .done and log.
    assert!(store.snapshot().params.get("beta").is_none());
    assert!(store.snapshot().state.get("beta").is_none());
    assert!(!store.done_path("beta").exists());
    assert!(!store.log_path("beta").exists());
    assert_eq!(store.log_content("beta"), "");

    // Parameter rename: the stale key is dropped, the declared default of
    // the new key is filled in.
    assert!(!store.params_for("gamma").contains_key("old_param"));
    assert_eq!(
        store.params_for("gamma").get("new_param"),
        Some(&str_value("x"))
    );

    // Unchanged step keeps its Ok state, marker and custom value.
    let delta = store.state_of("delta");
    assert_eq!(delta.status, StepStatus::Ok);
    assert!(store.done_path("delta").exists());
    assert_eq!(
        store.params_for("delta").get("k"),
        Some(&str_value("custom"))
    );

    // Adopting (and saving work afterwards) never rewrites the library file.
    assert_eq!(fs::read(&library_path).unwrap(), library_bytes_before);

    // The adopted snapshot is what a reopen resumes.
    let reopened = ProjectStore::new();
    reopened.open_work(&root).unwrap();
    let ids: Vec<String> = reopened
        .steps()
        .iter()
        .map(|step| step.id.clone())
        .collect();
    assert_eq!(ids, ["alpha", "gamma", "delta"]);
    assert_eq!(reopened.state_of("alpha").status, StepStatus::Pending);
    assert_eq!(reopened.state_of("gamma").exit_code, None);
    assert_eq!(reopened.state_of("delta").status, StepStatus::Ok);
    assert!(!reopened.done_path("alpha").exists());
    assert!(!reopened.done_path("beta").exists());
    assert!(reopened.done_path("delta").exists());
    assert_eq!(
        reopened.params_for("gamma").get("new_param"),
        Some(&str_value("x"))
    );
    assert_eq!(fs::read(&library_path).unwrap(), library_bytes_before);

    cleanup(&parent);
}

/// A failed adoption must leave logs, markers, `run.json` and in-memory
/// state exactly as they were: the fallible, non-destructive preparation
/// (step folders of an incoming new step) runs BEFORE any artifact removal,
/// and a later successful adoption still prunes normally.
#[test]
fn failed_adoption_keeps_artifacts_and_state() {
    let parent = temp_dir("adopt-fail");
    let root = parent.join("work");
    let store = ProjectStore::new();
    store
        .create_work(
            &root,
            &PipelineDoc::new(
                "failme",
                pipeline_of(vec![
                    param_step("alpha", "echo one", "mode", "fast"),
                    plain_step("beta", "echo two"),
                ]),
            ),
        )
        .unwrap();
    for id in ["alpha", "beta"] {
        store.append_log(id, &format!("log of {id}\n"));
        store.write_done_marker(id, STAMP);
        store.set_step_state(
            id,
            StepState {
                status: StepStatus::Ok,
                exit_code: Some(0),
                message: format!("ran {id}"),
                ..StepState::default()
            },
        );
    }
    store
        .set_params(
            "alpha",
            ValueMap::from([("mode".to_string(), str_value("custom"))]),
        )
        .unwrap();

    // Revision: alpha changes, beta is removed, gamma is NEW — and the
    // folder work must do for gamma is poisoned (`steps/gamma` is a file, so
    // `steps/gamma/input` cannot be resolved).
    let v2 = PipelineDoc::new(
        "failme",
        pipeline_of(vec![
            param_step("alpha", "echo CHANGED", "mode", "fast"),
            plain_step("gamma", "echo three"),
        ]),
    );
    fs::write(root.join("steps/gamma"), "not a directory").unwrap();
    let run_before = fs::read(root.join(".pipeline-manager/run.json")).unwrap();
    let alpha_done_before =
        fs::read(root.join(".pipeline-manager/done/alpha.done")).unwrap();

    store
        .adopt_definition(&v2)
        .expect_err("poisoned step-folder work must abort the adoption");

    // Nothing was destroyed or committed.
    assert_eq!(
        fs::read(root.join(".pipeline-manager/run.json")).unwrap(),
        run_before
    );
    assert_eq!(
        fs::read(root.join(".pipeline-manager/done/alpha.done")).unwrap(),
        alpha_done_before
    );
    assert_eq!(
        fs::read_to_string(root.join(".pipeline-manager/done/beta.done")).unwrap(),
        STAMP
    );
    assert_eq!(
        fs::read_to_string(root.join(".pipeline-manager/logs/alpha.log")).unwrap(),
        "log of alpha\n"
    );
    assert_eq!(
        fs::read_to_string(root.join(".pipeline-manager/logs/beta.log")).unwrap(),
        "log of beta\n"
    );
    // In-memory work state is untouched: definition, state and values.
    let ids: Vec<String> = store.steps().iter().map(|step| step.id.clone()).collect();
    assert_eq!(ids, ["alpha", "beta"]);
    assert_eq!(store.definition().pipeline.steps[0].command, "echo one");
    assert_eq!(store.state_of("alpha").status, StepStatus::Ok);
    assert_eq!(store.state_of("alpha").message, "ran alpha");
    assert_eq!(
        store.params_for("alpha").get("mode"),
        Some(&str_value("custom"))
    );
    assert_eq!(store.log_content("beta"), "log of beta\n");

    // Recovery: once the poison is gone, the same adoption commits fully —
    // changed alpha invalidated, removed beta pruned, gamma created.
    fs::remove_file(root.join("steps/gamma")).unwrap();
    let report = store.adopt_definition(&v2).expect("adoption succeeds");
    assert_eq!(report.changed, ["alpha"]);
    assert_eq!(report.removed, ["beta"]);
    assert_eq!(report.added, ["gamma"]);
    assert_eq!(store.state_of("alpha").status, StepStatus::Pending);
    assert!(!store.done_path("alpha").exists());
    assert!(!store.done_path("beta").exists());
    assert!(!store.log_path("beta").exists());
    assert!(store.snapshot().state.get("beta").is_none());
    assert!(root.join("steps/gamma/work").is_dir());
    assert_eq!(
        store.params_for("alpha").get("mode"),
        Some(&str_value("custom"))
    );

    cleanup(&parent);
}

/// Marker staging is fallible and must abort the adoption: a poisoned
/// `.done` of a CHANGED step is never followed, `run.json` never commits,
/// no other artifact moves, and the failed marker cannot flip the step to
/// stale `Ok` (the marker survives untouched with the old state).
#[cfg(unix)]
#[test]
fn marker_staging_failure_aborts_adoption() {
    let parent = temp_dir("stage-fail");
    let root = parent.join("work");
    let store = ProjectStore::new();
    store
        .create_work(
            &root,
            &PipelineDoc::new(
                "stage",
                pipeline_of(vec![
                    param_step("alpha", "echo one", "mode", "fast"),
                    plain_step("beta", "echo two"),
                ]),
            ),
        )
        .unwrap();
    for id in ["alpha", "beta"] {
        store.append_log(id, &format!("log of {id}\n"));
        store.write_done_marker(id, STAMP);
        store.set_step_state(
            id,
            StepState {
                status: StepStatus::Ok,
                exit_code: Some(0),
                message: format!("ran {id}"),
                ..StepState::default()
            },
        );
    }

    // alpha becomes CHANGED; replace ITS marker with a symlink so parking
    // the marker fails inside the managed directory.
    let outside = parent.join("outside.done");
    fs::write(&outside, "keep me").unwrap();
    let alpha_done = root.join(".pipeline-manager/done/alpha.done");
    fs::remove_file(&alpha_done).unwrap();
    std::os::unix::fs::symlink(&outside, &alpha_done).unwrap();
    let run_before = fs::read(root.join(".pipeline-manager/run.json")).unwrap();

    let v2 = PipelineDoc::new(
        "stage",
        pipeline_of(vec![
            param_step("alpha", "echo CHANGED", "mode", "fast"),
            plain_step("beta", "echo two"),
        ]),
    );
    let err = store
        .adopt_definition(&v2)
        .expect_err("marker staging failure must abort the adoption");
    let text = err.to_string();
    assert!(
        text.contains("symbolic link") || text.contains("outside the current project"),
        "message: {text}"
    );

    // Nothing committed or applied: document, state, other artifacts, and
    // the poisoned target's outside file are all untouched.
    assert_eq!(
        fs::read(root.join(".pipeline-manager/run.json")).unwrap(),
        run_before
    );
    assert_eq!(store.definition().pipeline.steps[0].command, "echo one");
    assert_eq!(store.state_of("alpha").status, StepStatus::Ok);
    assert_eq!(store.state_of("alpha").message, "ran alpha");
    assert_eq!(
        fs::read_to_string(root.join(".pipeline-manager/done/beta.done")).unwrap(),
        STAMP
    );
    assert_eq!(
        fs::read_to_string(root.join(".pipeline-manager/logs/alpha.log")).unwrap(),
        "log of alpha\n"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep me");

    // Recovery with a real marker: adoption commits, the changed marker is
    // parked away and the step lands in Pending (never stale `Ok`).
    fs::remove_file(&alpha_done).unwrap();
    store.write_done_marker("alpha", STAMP);
    let report = store.adopt_definition(&v2).expect("adoption succeeds");
    assert_eq!(report.changed, ["alpha"]);
    assert!(!store.done_path("alpha").exists());
    assert_eq!(store.state_of("alpha").status, StepStatus::Pending);
    assert_eq!(store.state_of("alpha").exit_code, None);

    cleanup(&parent);
}

/// The native dependency graph is data, not a normalization target: two
/// independent steps stay independent through `create_work`, an explicit
/// stored edge survives adoption, and a deliberately removed dependency is
/// never re-chained.
#[test]
fn native_graph_is_preserved_by_create_and_adopt() {
    let parent = temp_dir("native-graph");
    let root = parent.join("work");
    let store = ProjectStore::new();
    let independent = pipeline_of(vec![
        plain_step("one", "echo 1"),
        plain_step("two", "echo 2"),
    ]);

    store
        .create_work(&root, &PipelineDoc::new("native", independent.clone()))
        .unwrap();
    assert!(store.steps()[1].dependencies.is_empty());

    let mut linked = independent.clone();
    linked.steps[1].dependencies = vec!["one".to_string()];
    store
        .adopt_definition(&PipelineDoc::new("native", linked))
        .unwrap();
    assert_eq!(store.steps()[1].dependencies, ["one"]);

    let report = store
        .adopt_definition(&PipelineDoc::new("native", independent.clone()))
        .unwrap();
    assert_eq!(report.changed, ["two"]);
    assert!(
        store.steps()[1].dependencies.is_empty(),
        "removed dependency must stay removed"
    );

    cleanup(&parent);
}

/// A corrupt `run.json` surfaces a visible parse error on open and is never
/// silently replaced with a fresh document.
#[test]
fn corrupt_run_json_errors_without_replacing() {
    let parent = temp_dir("corrupt-run");
    let root = parent.join("work");
    let store = ProjectStore::new();
    store
        .create_work(&root, &PipelineDoc::new("v1", PipelineConfig::default()))
        .unwrap();
    let run_path = root.join(".pipeline-manager/run.json");
    fs::write(&run_path, "{ \"format_version\": 1, \"na").unwrap();
    let corrupt = fs::read(&run_path).unwrap();

    match ProjectStore::new().open_work(&root) {
        Err(StoreError::Message(message)) => {
            assert!(message.contains("run.json"), "message: {message}")
        }
        other => panic!("expected visible parse error, got {other:?}"),
    }
    assert_eq!(
        fs::read(&run_path).unwrap(),
        corrupt,
        "a failed open must not rewrite run.json"
    );

    cleanup(&parent);
}

/// `format_version: 999` in `run.json` or a `.pipeline` document surfaces a
/// visible "unsupported …" error and never clobbers the file.
#[test]
fn unsupported_version_rejected() {
    let parent = temp_dir("bad-version");

    // run.json
    let root = parent.join("work");
    let store = ProjectStore::new();
    store
        .create_work(&root, &PipelineDoc::new("v1", PipelineConfig::default()))
        .unwrap();
    let run_path = root.join(".pipeline-manager/run.json");
    let mut run: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&run_path).unwrap()).unwrap();
    run["format_version"] = serde_json::json!(999);
    fs::write(&run_path, serde_json::to_string_pretty(&run).unwrap()).unwrap();
    let tampered = fs::read(&run_path).unwrap();
    match ProjectStore::new().open_work(&root) {
        Err(StoreError::Message(message)) => {
            assert!(message.contains("unsupported"), "message: {message}");
            assert!(message.contains("999"), "message: {message}");
        }
        other => panic!("expected unsupported version error, got {other:?}"),
    }
    assert_eq!(
        fs::read(&run_path).unwrap(),
        tampered,
        "a failed open must not rewrite run.json"
    );

    // Nested: outer envelope 1 but the embedded definition carries 999 —
    // rejected with the same visible message before any state is installed.
    let mut run: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&run_path).unwrap()).unwrap();
    run["format_version"] = serde_json::json!(1);
    run["definition"]["format_version"] = serde_json::json!(999);
    fs::write(&run_path, serde_json::to_string_pretty(&run).unwrap()).unwrap();
    let tampered = fs::read(&run_path).unwrap();
    match ProjectStore::new().open_work(&root) {
        Err(StoreError::Message(message)) => {
            assert!(message.contains("unsupported"), "message: {message}");
            assert!(message.contains("999"), "message: {message}");
        }
        other => panic!("expected nested unsupported version error, got {other:?}"),
    }
    assert_eq!(
        fs::read(&run_path).unwrap(),
        tampered,
        "a failed open must not rewrite run.json"
    );

    // .pipeline document
    let library = PipelineLibrary::new(parent.join("pipelines"));
    let doc = PipelineDoc::new("future", PipelineConfig::default());
    library.save("future", &doc, SaveMode::CreateNew).unwrap();
    let lib_path = parent.join("pipelines/future.pipeline");
    let mut lib: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&lib_path).unwrap()).unwrap();
    lib["format_version"] = serde_json::json!(999);
    fs::write(&lib_path, serde_json::to_string_pretty(&lib).unwrap()).unwrap();
    let tampered = fs::read(&lib_path).unwrap();
    match library.load("future") {
        Err(StoreError::Message(message)) => {
            assert!(message.contains("unsupported"), "message: {message}");
            assert!(message.contains("999"), "message: {message}");
        }
        other => panic!("expected unsupported version error, got {other:?}"),
    }
    assert_eq!(
        fs::read(&lib_path).unwrap(),
        tampered,
        "a failed load must not rewrite the document"
    );

    cleanup(&parent);
}
