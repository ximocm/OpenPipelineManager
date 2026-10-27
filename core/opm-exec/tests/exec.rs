//! Behavior ports of the pinned Python contracts:
//! `backend/tests/test_execution.py` (command building, quoting/injection,
//! `is_step_ok` transitions, ordering, blocking) and the behavioral essence of
//! `backend/tests/test_execution_api.py` (blockers prevent a run; nothing
//! executes on a blocked run — no HTTP in the new app).

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::temp_dir;
use opm_exec::{ExecError, ExecEvent, ExecutionManager, StepState, StepStatus};
use opm_pipeline::{FieldType, IssueSeverity, OutputSpec, PipelineStep, Value, ValueSpec};
use opm_store::{ProjectStore, ValueMap};

fn cleanup(parent: &PathBuf) {
    let _ = fs::remove_dir_all(parent);
}

/// Fixture: project + store + manager (§11 A6: `Arc<ProjectStore>`).
fn fixture(tag: &str) -> (PathBuf, Arc<ProjectStore>, ExecutionManager) {
    let parent = temp_dir(tag);
    let store = Arc::new(ProjectStore::new());
    store.create_project(&parent, "proj").expect("scaffold");
    let manager = ExecutionManager::new(Arc::clone(&store));
    (parent, store, manager)
}

/// Write `pipeline.json` with the given `steps` array and import it.
fn import_json(store: &ProjectStore, steps: &str) {
    let root = store.project_root().expect("project open");
    let path = root.join("pipeline.json");
    fs::write(&path, format!("{{\"steps\": {steps}}}")).expect("write pipeline");
    store.import_pipeline(&path).expect("import");
}

/// Collect events until `done` matches (or panic with what arrived).
fn collect_until(
    manager: &ExecutionManager,
    timeout: Duration,
    done: impl Fn(&ExecEvent) -> bool,
) -> Vec<ExecEvent> {
    let deadline = Instant::now() + timeout;
    let mut events: Vec<ExecEvent> = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for event; collected: {events:?}");
        }
        let Some(event) = manager.next_event(remaining.min(Duration::from_millis(100))) else {
            continue;
        };
        let matched = done(&event);
        events.push(event);
        if matched {
            return events;
        }
    }
}

fn collect_finished(manager: &ExecutionManager) -> Vec<ExecEvent> {
    collect_until(manager, Duration::from_secs(15), |event| {
        matches!(event, ExecEvent::ExecutionFinished)
    })
}

fn started_ids(events: &[ExecEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ExecEvent::StepStarted { step_id, .. } => Some(step_id.clone()),
            _ => None,
        })
        .collect()
}

fn finished_state<'a>(events: &'a [ExecEvent], step_id: &str) -> Option<&'a StepState> {
    events.iter().find_map(|event| match event {
        ExecEvent::StepFinished {
            step_id: id,
            state,
        } if id == step_id => Some(state),
        _ => None,
    })
}

/// A `PipelineStep` as Pydantic would build it from a partial dict.
fn command_step(command: &str) -> PipelineStep {
    PipelineStep {
        id: "a".to_string(),
        name: "A".to_string(),
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

fn spec(key: &str, kind: FieldType, default: Option<Value>) -> ValueSpec {
    ValueSpec {
        key: key.to_string(),
        label: None,
        kind,
        required: false,
        default,
        options: Vec::new(),
        min: None,
        max: None,
        description: String::new(),
        source_step: None,
        source_output: None,
    }
}

fn str_value(value: &str) -> Value {
    Value::Str(value.to_string())
}

/// A closed store is enough: `build_command` never touches it.
fn builder() -> ExecutionManager {
    ExecutionManager::new(Arc::new(ProjectStore::new()))
}

// --------------------------------------------------------- build_command tests

#[test]
fn build_command_replaces_placeholders() {
    let mut step = command_step("echo {input_file} {threshold}");
    step.inputs = vec![spec(
        "input_file",
        FieldType::File,
        Some(str_value("input.txt")),
    )];
    step.parameters = vec![spec(
        "threshold",
        FieldType::Decimal,
        Some(Value::Float(0.5)),
    )];

    let command = builder().build_command(&step, &ValueMap::new()).unwrap();
    assert_eq!(command, "echo input.txt 0.5");
}

#[test]
fn build_command_replaces_output_placeholders() {
    let mut step = command_step("tool --out {result_file}");
    step.outputs = vec![OutputSpec {
        key: "result_file".to_string(),
        label: None,
        path: "outputs/result.txt".to_string(),
    }];

    let command = builder().build_command(&step, &ValueMap::new()).unwrap();
    assert_eq!(command, "tool --out outputs/result.txt");
}

/// Params can never override output paths (effective values: outputs win).
#[test]
fn build_command_does_not_allow_params_to_override_output_paths() {
    let mut step = command_step("tool --out {result_file}");
    step.outputs = vec![OutputSpec {
        key: "result_file".to_string(),
        label: None,
        path: "safe.txt".to_string(),
    }];
    let params = ValueMap::from([("result_file".to_string(), str_value("../outside.txt"))]);

    let command = builder().build_command(&step, &params).unwrap();
    assert_eq!(command, "tool --out safe.txt");
}

/// The injection payload stays a single shell word (exact shlex quoting).
#[test]
fn build_command_quotes_placeholder_values_with_shell_metacharacters() {
    let mut step = command_step("tool --input {input_file}");
    step.inputs = vec![spec(
        "input_file",
        FieldType::File,
        Some(str_value("Input/sample file.txt; touch injected")),
    )];

    let command = builder().build_command(&step, &ValueMap::new()).unwrap();
    assert_eq!(command, "tool --input 'Input/sample file.txt; touch injected'");
}

#[test]
fn build_command_preserves_already_quoted_placeholder_boundaries() {
    let mut step = command_step(r#"tool --input "{input_file}" --label '{sample_label}'"#);
    step.inputs = vec![spec(
        "input_file",
        FieldType::File,
        Some(str_value("Input/sample file.txt; touch injected")),
    )];
    step.parameters = vec![spec(
        "sample_label",
        FieldType::Text,
        Some(str_value("alpha's sample")),
    )];

    let command = builder().build_command(&step, &ValueMap::new()).unwrap();
    assert_eq!(
        command,
        r#"tool --input "Input/sample file.txt; touch injected" --label 'alpha'\''s sample'"#
    );
}

#[test]
fn build_command_keeps_multiline_commands() {
    let mut step = command_step(
        "INPUT=\"{input_file}\" \\\nOUTPUT=\"{result_file}\" \\\nbash run.sh",
    );
    step.inputs = vec![spec(
        "input_file",
        FieldType::File,
        Some(str_value("input.txt")),
    )];
    step.outputs = vec![OutputSpec {
        key: "result_file".to_string(),
        label: None,
        path: "outputs/result.txt".to_string(),
    }];

    let command = builder().build_command(&step, &ValueMap::new()).unwrap();
    assert_eq!(
        command,
        "INPUT=\"input.txt\" \\\nOUTPUT=\"outputs/result.txt\" \\\nbash run.sh"
    );
}

#[test]
fn build_command_errors_for_missing_placeholder_and_names_the_key() {
    let mut step = command_step("echo {input_file}");
    step.inputs = vec![spec("input_file", FieldType::File, None)];

    match builder().build_command(&step, &ValueMap::new()) {
        Err(ExecError::MissingPlaceholders(keys)) => assert_eq!(keys, ["input_file"]),
        other => panic!("expected MissingPlaceholders, got {other:?}"),
    }
}

/// conda|mamba|micromamba run, module load, shell snippet, bare/unknown env.
#[test]
fn build_command_wraps_environments() {
    let cases = [
        ("conda:analysis", "python script.py", "conda run -n analysis python script.py"),
        ("mamba:analysis", "cmd", "mamba run -n analysis cmd"),
        ("micromamba:analysis", "cmd", "micromamba run -n analysis cmd"),
        ("module:bioinfo", "cmd", "module load bioinfo && cmd"),
        (
            "shell:source /opt/venv/bin/activate",
            "cmd",
            "source /opt/venv/bin/activate && cmd",
        ),
        ("myenv", "cmd", "conda run -n myenv cmd"),
        ("unknown:thing", "cmd", "conda run -n unknown:thing cmd"),
        (":oops", "cmd", "conda run -n :oops cmd"),
        ("conda:", "cmd", "cmd"),
        ("", "cmd", "cmd"),
        ("   ", "cmd", "cmd"),
        ("conda:my env", "cmd", "conda run -n 'my env' cmd"),
        (
            "shell:cd /tmp && source x",
            "cmd",
            "cd /tmp && source x && cmd",
        ),
    ];
    for (environment, command, expected) in cases {
        let mut step = command_step(command);
        step.environment = environment.to_string();
        let built = builder().build_command(&step, &ValueMap::new()).unwrap();
        assert_eq!(built, expected, "environment {environment:?}");
    }
}

// ------------------------------------------------------- real /bin/sh runs

/// The unquoted injection payload reaches printf as ONE argv element and never
/// creates `injected` (`#[cfg(unix)]`: real `/bin/sh`).
#[cfg(unix)]
#[test]
fn run_keeps_placeholder_as_a_single_shell_argument() {
    let input_value = "Input/sample file.txt; touch injected";
    let (parent, store, manager) = fixture("run-inject");
    let root = store.project_root().unwrap();
    fs::create_dir_all(root.join("Input")).unwrap();
    fs::write(root.join(input_value), "ok").unwrap();
    import_json(
        &store,
        &format!(
            r#"[{{
            "id": "a",
            "name": "A",
            "command": "printf '%s' {{input_file}} > argv.txt",
            "inputs": [{{"key": "input_file", "type": "file", "required": true, "default": "{value}"}}]
        }}]"#,
            value = input_value
        ),
    );

    manager.run_steps(vec!["a".to_string()]).unwrap();
    let events = collect_finished(&manager);

    assert_eq!(store.state_of("a").status, StepStatus::Ok);
    assert_eq!(
        fs::read_to_string(root.join("argv.txt")).unwrap(),
        input_value
    );
    assert!(!root.join("injected").exists());
    assert!(finished_state(&events, "a").is_some());

    cleanup(&parent);
}

/// Same payload behind `"…"` quotes: `$` and `;` stay literal.
#[cfg(unix)]
#[test]
fn run_keeps_quoted_placeholder_as_a_single_shell_argument() {
    let input_value = "Input/sample $HOME file.txt; touch injected";
    let (parent, store, manager) = fixture("run-quoted-inject");
    let root = store.project_root().unwrap();
    fs::create_dir_all(root.join("Input")).unwrap();
    fs::write(root.join(input_value), "ok").unwrap();
    import_json(
        &store,
        &format!(
            r#"[{{
            "id": "a",
            "name": "A",
            "command": "printf '%s' \"{{input_file}}\" > quoted-argv.txt",
            "inputs": [{{"key": "input_file", "type": "file", "required": true, "default": "{value}"}}]
        }}]"#,
            value = input_value
        ),
    );

    manager.run_steps(vec!["a".to_string()]).unwrap();
    let events = collect_finished(&manager);

    assert_eq!(store.state_of("a").status, StepStatus::Ok);
    assert_eq!(
        fs::read_to_string(root.join("quoted-argv.txt")).unwrap(),
        input_value
    );
    assert!(!root.join("injected").exists());
    assert!(finished_state(&events, "a").is_some());

    cleanup(&parent);
}

// ------------------------------------------------------------- preflight

/// Unsafe working directory → validation blocker → `Blocked` BEFORE any
/// thread/process starts (no marker file ever appears).
#[test]
fn run_blocks_unsafe_working_directory_before_starting_anything() {
    let (parent, store, manager) = fixture("unsafe-wd");
    let outside = parent.join("proj-outside-work");
    fs::create_dir(&outside).unwrap();
    let marker = outside.join("ran.txt");
    import_json(
        &store,
        r#"[{
            "id": "blocked",
            "name": "Blocked",
            "working_directory": "../proj-outside-work",
            "command": "printf ran > ran.txt"
        }]"#,
    );

    match manager.run_steps(vec!["blocked".to_string()]) {
        Err(ExecError::Blocked { message, blockers }) => {
            assert_eq!(message, "Execution blocked by 1 validation blocker");
            assert!(blockers
                .iter()
                .any(|issue| issue.field.as_deref() == Some("working_directory")));
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
    let status = manager.status();
    assert!(!status.running);
    assert_eq!(status.current_step_id, None);
    assert!(!marker.exists());

    cleanup(&parent);
}

/// Behavioral essence of `test_execution_api.py`: a blocker inside the
/// dependency closure of the requested step blocks the run — no thread, no
/// marker.
#[test]
fn run_blocks_on_dependency_closure_blockers_without_executing() {
    let (parent, store, manager) = fixture("closure-blocked");
    let root = store.project_root().unwrap();
    let marker = root.join("target.marker");
    import_json(
        &store,
        r#"[
            {
                "id": "prepare",
                "name": "Prepare",
                "command": "printf prepare",
                "inputs": [{"key": "input_file", "type": "file", "required": true}]
            },
            {
                "id": "target",
                "name": "Target",
                "command": "printf target > target.marker",
                "dependencies": ["prepare"]
            }
        ]"#,
    );

    match manager.run_steps(vec!["target".to_string()]) {
        Err(ExecError::Blocked { message, blockers }) => {
            assert_eq!(message, "Execution blocked by 1 validation blocker");
            assert!(blockers.iter().any(|issue| {
                issue.step_id.as_deref() == Some("prepare")
                    && issue.field.as_deref() == Some("input_file")
            }));
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
    assert!(!manager.status().running);
    assert!(!marker.exists());

    cleanup(&parent);
}

/// §11 A10: an invalid branch must not block running a valid independent step.
#[cfg(unix)]
#[test]
fn run_proceeds_for_a_valid_step_when_another_branch_is_invalid() {
    let (parent, store, manager) = fixture("unrelated-valid");
    let root = store.project_root().unwrap();
    import_json(
        &store,
        r#"[
            {
                "id": "prepare",
                "name": "Prepare",
                "command": "printf prepare",
                "inputs": [{"key": "input_file", "type": "file", "required": true}]
            },
            {
                "id": "target",
                "name": "Target",
                "command": "printf target > target.marker",
                "dependencies": ["prepare"]
            },
            {
                "id": "sibling",
                "name": "Sibling",
                "command": "printf sibling > sibling.marker"
            }
        ]"#,
    );

    manager.run_steps(vec!["sibling".to_string()]).unwrap();
    collect_finished(&manager);

    assert_eq!(store.state_of("sibling").status, StepStatus::Ok);
    assert!(root.join("sibling.marker").exists());
    assert!(!root.join("target.marker").exists());

    cleanup(&parent);
}

#[test]
fn run_rejects_unknown_step_ids() {
    let (parent, store, manager) = fixture("unknown-step");
    import_json(
        &store,
        r#"[{"id": "a", "name": "A", "command": "printf a > a.marker"}]"#,
    );

    match manager.run_steps(vec!["ghost".to_string()]) {
        Err(ExecError::UnknownStep(id)) => assert_eq!(id, "ghost"),
        other => panic!("expected UnknownStep, got {other:?}"),
    }
    assert!(!manager.status().running);

    cleanup(&parent);
}

/// fan-out (a → b, a → c) and fan-in (b, c → d): deps first, deterministic
/// post-order — `a, b, c, d`.
#[cfg(unix)]
#[test]
fn run_orders_dependencies_with_fan_in_and_fan_out() {
    let (parent, store, manager) = fixture("topo-order");
    let root = store.project_root().unwrap();
    import_json(
        &store,
        r#"[
            {"id": "a", "name": "A", "command": "printf a > a.mark"},
            {"id": "b", "name": "B", "command": "printf b > b.mark", "dependencies": ["a"]},
            {"id": "c", "name": "C", "command": "printf c > c.mark", "dependencies": ["a"]},
            {"id": "d", "name": "D", "command": "printf d > d.mark", "dependencies": ["b", "c"]}
        ]"#,
    );

    manager.run_steps(vec!["d".to_string()]).unwrap();
    let events = collect_finished(&manager);

    assert_eq!(started_ids(&events), ["a", "b", "c", "d"]);
    for id in ["a", "b", "c", "d"] {
        assert_eq!(store.state_of(id).status, StepStatus::Ok, "step {id}");
        assert!(root.join(format!("{id}.mark")).exists(), "step {id} ran");
    }
    assert!(matches!(
        events.last().unwrap(),
        ExecEvent::ExecutionFinished
    ));

    cleanup(&parent);
}

/// Dependency not `ok` at run time → `Error` + `StepSkipped`, and the step's
/// command never runs.
#[cfg(unix)]
#[test]
fn run_skips_step_when_dependency_is_not_ok() {
    let (parent, store, manager) = fixture("skip-dependency");
    let root = store.project_root().unwrap();
    import_json(
        &store,
        r#"[
            {"id": "prep", "name": "Prep", "command": "exit 7"},
            {
                "id": "follow",
                "name": "Follow",
                "command": "printf ran > follow.mark",
                "dependencies": ["prep"]
            }
        ]"#,
    );

    manager.run_steps(vec!["follow".to_string()]).unwrap();
    let events = collect_finished(&manager);

    assert_eq!(started_ids(&events), ["prep"]);
    let prep_state = finished_state(&events, "prep").expect("prep finished");
    assert_eq!(prep_state.status, StepStatus::Error);
    assert_eq!(prep_state.message, "Command exited with 7");
    let skipped = events.iter().find_map(|event| match event {
        ExecEvent::StepSkipped { step_id, message } if step_id == "follow" => Some(message.clone()),
        _ => None,
    });
    assert_eq!(
        skipped.as_deref(),
        Some("Dependency is not complete: prep")
    );
    let follow_state = store.state_of("follow");
    assert_eq!(follow_state.status, StepStatus::Error);
    assert_eq!(follow_state.message, "Dependency is not complete: prep");
    assert!(!root.join("follow.mark").exists());
    assert!(matches!(
        events.last().unwrap(),
        ExecEvent::ExecutionFinished
    ));

    // Log header: `[\n<UTC ISO>] $ <command>` (timestamp smoke check).
    let log = store.log_content("prep");
    assert!(log.contains("] $ exit 7"), "log: {log:?}");
    assert!(log.contains("+00:00"), "log: {log:?}");

    cleanup(&parent);
}

/// `stop` terminates the current process (`Cancelled` /
/// "Execution cancelled by user") and cancels the remaining steps.
#[cfg(unix)]
#[test]
fn stop_cancels_current_step_and_skips_the_rest() {
    let (parent, store, manager) = fixture("stop-cancel");
    let root = store.project_root().unwrap();
    import_json(
        &store,
        r#"[
            {"id": "sleeper", "name": "Sleeper", "command": "sleep 30"},
            {
                "id": "follower",
                "name": "Follower",
                "command": "printf ran > follower.marker",
                "dependencies": ["sleeper"]
            }
        ]"#,
    );

    manager.run_steps(vec!["follower".to_string()]).unwrap();
    let mut events = collect_until(&manager, Duration::from_secs(10), |event| {
        matches!(event, ExecEvent::StepStarted { step_id, .. } if step_id == "sleeper")
    });
    manager.stop();
    events.extend(collect_finished(&manager));

    let sleeper = finished_state(&events, "sleeper").expect("sleeper finished");
    assert_eq!(sleeper.status, StepStatus::Cancelled);
    assert_eq!(sleeper.message, "Execution cancelled by user");
    assert!(sleeper.exit_code.is_some());
    assert_eq!(started_ids(&events), ["sleeper"]);
    assert!(!root.join("follower.marker").exists());
    assert_eq!(store.state_of("sleeper").status, StepStatus::Cancelled);
    assert_eq!(store.state_of("follower").status, StepStatus::Pending);

    let status = manager.status();
    assert!(!status.running);
    assert!(status.stop_requested);
    assert_eq!(status.current_step_id, None);

    cleanup(&parent);
}

#[cfg(unix)]
#[test]
fn second_run_while_running_is_rejected() {
    let (parent, store, manager) = fixture("already-running");
    import_json(&store, r#"[{"id": "sleeper", "name": "Sleeper", "command": "sleep 30"}]"#);

    manager.run_steps(vec!["sleeper".to_string()]).unwrap();
    collect_until(&manager, Duration::from_secs(10), |event| {
        matches!(event, ExecEvent::StepStarted { .. })
    });
    match manager.run_steps(vec!["sleeper".to_string()]) {
        Err(ExecError::AlreadyRunning) => {}
        other => panic!("expected AlreadyRunning, got {other:?}"),
    }
    manager.stop();
    collect_finished(&manager);

    cleanup(&parent);
}

/// `run_selected`: explicit selection wins; otherwise every step not `ok`.
#[cfg(unix)]
#[test]
fn run_selected_prefers_selected_steps_then_not_ok_steps() {
    let (parent, store, manager) = fixture("run-selected");
    let root = store.project_root().unwrap();
    import_json(
        &store,
        r#"[
            {"id": "a", "name": "A", "command": "printf a > a.mark"},
            {"id": "b", "name": "B", "command": "printf b > b.mark"}
        ]"#,
    );
    // b auto-chains onto a (no explicit deps anywhere).
    assert_eq!(store.steps()[1].dependencies, ["a"]);

    store.toggle_selected("a").unwrap();
    manager.run_selected().unwrap();
    let events = collect_finished(&manager);
    assert_eq!(started_ids(&events), ["a"]);
    assert!(root.join("a.mark").exists());
    assert!(!root.join("b.mark").exists());
    assert_eq!(store.state_of("b").status, StepStatus::Pending);

    // Deselect everything: the requested set is every step that is not ok
    // (only b), and Python's `_topological_subset` still pulls a in as b's
    // dependency — so both steps run.
    store.toggle_selected("a").unwrap();
    manager.run_selected().unwrap();
    let events = collect_finished(&manager);
    assert_eq!(started_ids(&events), ["a", "b"]);
    assert!(root.join("b.mark").exists());
    assert_eq!(store.state_of("b").status, StepStatus::Ok);

    cleanup(&parent);
}

// ------------------------------------------------- unsupported shell contexts (F1)

/// Payload de inyección de los PoCs del revisor.
fn injection_label() -> Value {
    str_value("hello; touch injected; #")
}

fn label_parameter(step: &mut PipelineStep) {
    step.parameters = vec![spec("label", FieldType::Text, Some(injection_label()))];
}

/// F1 (PoC revisor): placeholder dentro de COMENTARIO → `UnsupportedContext`
/// con el placeholder nombrado; jamás se sustituye.
#[test]
fn build_command_rejects_placeholder_in_a_comment() {
    let mut step = command_step("# it's a note about {label}\nprintf '%s' ok");
    label_parameter(&mut step);

    match builder().build_command(&step, &ValueMap::new()) {
        Err(ExecError::UnsupportedContext { placeholder }) => {
            assert_eq!(placeholder, "label");
        }
        other => panic!("expected UnsupportedContext, got {other:?}"),
    }
}

/// El texto del error es la explicación visible (paridad con el bloqueo de
/// validación).
#[test]
fn unsupported_context_error_explains_itself() {
    let mut step = command_step("printf '%s' \"$(printf '%s' {label})\"");
    label_parameter(&mut step);

    let err = builder()
        .build_command(&step, &ValueMap::new())
        .expect_err("substitution inside $() must be refused");
    assert_eq!(
        err.to_string(),
        "placeholder label sits in a shell context OPM cannot substitute safely (comment, $() or heredoc)"
    );
}

/// F1 (PoC revisor): placeholder dentro de `$(...)` → `UnsupportedContext`.
#[test]
fn build_command_rejects_placeholder_in_command_substitution() {
    let mut step = command_step(r#"printf '%s' "$(printf '%s' {label})""#);
    label_parameter(&mut step);

    match builder().build_command(&step, &ValueMap::new()) {
        Err(ExecError::UnsupportedContext { placeholder }) => {
            assert_eq!(placeholder, "label");
        }
        other => panic!("expected UnsupportedContext, got {other:?}"),
    }
}

/// F1: placeholder dentro del cuerpo de un heredoc → `UnsupportedContext`.
#[test]
fn build_command_rejects_placeholder_in_a_heredoc_body() {
    let mut step = command_step("cat <<EOF\n{label}\nEOF");
    label_parameter(&mut step);

    match builder().build_command(&step, &ValueMap::new()) {
        Err(ExecError::UnsupportedContext { placeholder }) => {
            assert_eq!(placeholder, "label");
        }
        other => panic!("expected UnsupportedContext, got {other:?}"),
    }
}

/// Contextos ordinarios: la sustitución sigue siendo la cotexto-dependiente
/// de siempre (comillas simples/dobles y comentarios sin placeholder sin
/// cambios) — ningún `UnsupportedContext` falso.
#[test]
fn build_command_keeps_ordinary_quoted_and_commented_commands() {
    let mut step = command_step("# user's pipeline\nprintf '%s' {label} \"{label}\"");
    label_parameter(&mut step);

    let command = builder().build_command(&step, &ValueMap::new()).unwrap();
    assert_eq!(
        command,
        "# user's pipeline\nprintf '%s' 'hello; touch injected; #' \"hello; touch injected; #\""
    );
}

/// PoC COMENTARIO exacto del revisor, a nivel de run: la sustitución es
/// segura (valor entero entrecomillado) — `injected` NUNCA aparece.
#[cfg(unix)]
#[test]
fn run_comment_poc_never_creates_injected() {
    let (parent, store, manager) = fixture("run-comment-poc");
    let root = store.project_root().unwrap();
    import_json(
        &store,
        r##"[{
            "id": "a",
            "name": "A",
            "command": "# user's pipeline\nprintf '%s' {label} > argv.txt",
            "parameters": [{"key": "label", "type": "text", "required": true, "default": "hello; touch injected; #"}]
        }]"##,
    );

    manager.run_steps(vec!["a".to_string()]).unwrap();
    collect_finished(&manager);

    assert_eq!(store.state_of("a").status, StepStatus::Ok);
    assert_eq!(
        fs::read_to_string(root.join("argv.txt")).unwrap(),
        "hello; touch injected; #"
    );
    assert!(!root.join("injected").exists());

    cleanup(&parent);
}

/// PoC `$(...)` exacto del revisor, a nivel de run: la validación bloquea
/// ANTES de arrancar nada (`Blocked`, 1 bloqueador en `command`) y
/// `injected` jamás aparece.
#[test]
fn run_blocks_command_substitution_poc_before_starting() {
    let (parent, store, manager) = fixture("run-subst-poc");
    let root = store.project_root().unwrap();
    import_json(
        &store,
        r#"[{
            "id": "a",
            "name": "A",
            "command": "printf '%s' \"$(printf '%s' {label})\"",
            "parameters": [{"key": "label", "type": "text", "required": true, "default": "hello; touch injected; #"}]
        }]"#,
    );

    match manager.run_steps(vec!["a".to_string()]) {
        Err(ExecError::Blocked { message, blockers }) => {
            assert_eq!(message, "Execution blocked by 1 validation blocker");
            assert!(blockers.iter().any(|issue| {
                issue.severity == IssueSeverity::Blocker
                    && issue.field.as_deref() == Some("command")
                    && issue.step_id.as_deref() == Some("a")
            }));
        }
        other => panic!("expected Blocked, got {other:?}"),
    }
    assert!(!manager.status().running);
    assert!(!root.join("injected").exists());

    cleanup(&parent);
}

// ------------------------------------------------------------------ stop (F7)

/// F7: un hijo que cierra su salida capturada (`exec >/dev/null 2>&1;
/// sleep 30`) deja al worker haciendo `wait()` — `stop()` debe seguir
/// llegando al candado enseguida, matar al hijo y emitir `execution.finished`.
#[cfg(unix)]
#[test]
fn stop_returns_promptly_when_the_child_closed_its_output() {
    let (parent, store, manager) = fixture("stop-closed-output");
    import_json(
        &store,
        r#"[{"id": "quiet", "name": "Quiet", "command": "exec >/dev/null 2>&1; sleep 30"}]"#,
    );

    manager.run_steps(vec!["quiet".to_string()]).unwrap();
    collect_until(&manager, Duration::from_secs(10), |event| {
        matches!(event, ExecEvent::StepStarted { step_id, .. } if step_id == "quiet")
    });
    // Margen para que el hijo cierre su salida y el worker entre en el reap.
    std::thread::sleep(Duration::from_millis(500));

    let started = Instant::now();
    manager.stop();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(3), "stop() bloqueó {elapsed:?}");

    // Si el hijo no se terminara, `sleep 30` haría saltar el timeout (15s).
    let events = collect_finished(&manager);
    let quiet = finished_state(&events, "quiet").expect("quiet finished");
    assert_eq!(quiet.status, StepStatus::Cancelled);
    assert_eq!(quiet.message, "Execution cancelled by user");
    assert_eq!(store.state_of("quiet").status, StepStatus::Cancelled);
    assert!(!manager.status().running);

    cleanup(&parent);
}

/// Escalada de parada: un hijo que IGNORA SIGTERM (`trap '' TERM`) sobrevive
/// a `stop()`, así que `force_stop()` (SIGKILL al hijo inmediato) es el que
/// termina la ejecución — que sigue reportándose como `cancelled`.
///
/// La salida va a `/dev/null` para que el nieto `sleep` no mantenga abierto
/// el extremo de escritura de la tubería (limitación hijo-único documentada:
/// los descendientes pueden sobrevivir al hijo).
#[cfg(unix)]
#[test]
fn force_stop_kills_a_child_that_ignores_sigterm() {
    let (parent, store, manager) = fixture("force-stop");
    import_json(
        &store,
        r#"[{"id": "stubborn", "name": "Stubborn", "command": "trap '' TERM; sleep 30 >/dev/null 2>&1; true"}]"#,
    );

    manager.run_steps(vec!["stubborn".to_string()]).unwrap();
    collect_until(&manager, Duration::from_secs(10), |event| {
        matches!(event, ExecEvent::StepStarted { step_id, .. } if step_id == "stubborn")
    });
    // Margen para que la shell instale el `trap` y arranque el `sleep`.
    std::thread::sleep(Duration::from_millis(500));

    // SIGTERM ignorado: la ejecución sigue viva tras esperar lo razonable.
    manager.stop();
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        manager.status().running,
        "el hijo ignora SIGTERM: stop() no basta y hace falta force_stop()"
    );

    // SIGKILL al hijo inmediato → EOF de la tubería → `ExecutionFinished`.
    let started = Instant::now();
    manager.force_stop();
    let events = collect_finished(&manager);
    let state = finished_state(&events, "stubborn").expect("stubborn finished");
    assert_eq!(state.status, StepStatus::Cancelled);
    assert_eq!(state.message, "Execution cancelled by user");
    assert!(!manager.status().running);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "force_stop() no recogió la ejecución en {:?}",
        started.elapsed()
    );
    assert_eq!(store.state_of("stubborn").status, StepStatus::Cancelled);

    cleanup(&parent);
}
