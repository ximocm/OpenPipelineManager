//! Validation contract — ports `backend/tests/test_validation.py`.
//!
//! Every Python test function maps to one Rust test (parametrized pytest
//! cases run as loops inside the matching test). Fixtures are parsed from
//! YAML, mirroring the Python `PipelineConfig.model_validate({...})` dicts.
//! The single exception: `test_source_link_relpath_failure_becomes_validation_blocker`
//! monkeypatches `os.path.relpath` in Python and Rust has no equivalent
//! hook; its error branch lives in `opm_fs::relative_project_path` instead
//! (reported deviation, not part of the pinned scenario list).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU32, Ordering};

use opm_pipeline::{
    parse_pipeline, placeholders, validate_pipeline, FieldType, IssueSeverity, PipelineConfig,
    ValidationIssue, Value,
};

// ---------------------------------------------------------------- helpers --

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Isolated project tree (Python `tmp_path` equivalent): `base/project` is
/// the project directory, `base/` its parent (where outside-files live),
/// removed on drop.
struct TempProject {
    base: PathBuf,
}

impl TempProject {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "opm-pipeline-test-{}-{}",
            process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(base.join("project")).expect("temp project created");
        TempProject { base }
    }

    fn project(&self) -> PathBuf {
        self.base.join("project")
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn config(yaml: &str) -> PipelineConfig {
    parse_pipeline("pipeline.yml", yaml).expect("fixture must parse")
}

fn params(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn params_for(step_id: &str, entries: &[(&str, Value)]) -> HashMap<String, BTreeMap<String, Value>> {
    let mut map = HashMap::new();
    if !entries.is_empty() {
        map.insert(step_id.to_owned(), params(entries));
    }
    map
}

fn blockers(issues: &[ValidationIssue]) -> Vec<&ValidationIssue> {
    issues
        .iter()
        .filter(|issue| issue.severity == IssueSeverity::Blocker)
        .collect()
}

fn assert_no_blockers(issues: &[ValidationIssue]) {
    let found = blockers(issues);
    assert!(found.is_empty(), "unexpected blockers: {:#?}", found);
}

// ------------------------------------------------------- graph-level rules --

/// test_detects_duplicate_ids
#[test]
fn detects_duplicate_ids() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
  - id: a
    name: A again
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("id")
                && issue.severity == IssueSeverity::Blocker),
        "{:#?}",
        issues
    );
}

/// test_detects_missing_dependency
#[test]
fn detects_missing_dependency() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: b
    name: B
    dependencies: [missing]
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.message.contains("does not exist")),
        "{:#?}",
        issues
    );
    assert!(
        issues.iter().any(|issue| issue.message == "Dependency does not exist: missing"
            && issue.field.as_deref() == Some("dependencies")
            && issue.step_id.as_deref() == Some("b")),
        "{:#?}",
        issues
    );
}

/// test_detects_cycles (plus the `a -> b -> a` message shape)
#[test]
fn detects_cycles_with_python_message_shape() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    dependencies: [b]
  - id: b
    name: B
    dependencies: [a]
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues
            .iter()
            .any(|issue| issue.message.to_lowercase().contains("cycle")),
        "{:#?}",
        issues
    );
    assert!(
        issues.iter().any(|issue| issue.message
            == "Dependency cycle detected: a -> b -> a"
            && issue.severity == IssueSeverity::Blocker
            && issue.field.as_deref() == Some("dependencies")
            && issue.step_id.as_deref() == Some("a")),
        "{:#?}",
        issues
    );
}

// ---------------------------------------------------------- input presence --

/// test_required_input_missing_is_blocker
#[test]
fn required_input_missing_is_blocker() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        required: true
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("input_file")
            && issue.severity == IssueSeverity::Blocker),
        "{:#?}",
        issues
    );
}

/// test_optional_input_missing_is_warning
#[test]
fn optional_input_missing_is_warning() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        required: false
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("input_file")
            && issue.severity == IssueSeverity::Warning),
        "{:#?}",
        issues
    );
}

// --------------------------------------------------------- value-spec rules --

/// test_numeric_range_validation
#[test]
fn numeric_range_validation() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    parameters:
      - key: threshold
        type: decimal
        min: 0
        max: 1
"#,
    );
    let params = params_for("a", &[("threshold", Value::Int(2))]);

    let issues = validate_pipeline(&pipeline, &tmp.project(), &params);

    assert!(
        issues.iter().any(|issue| issue.message.contains("above maximum")),
        "{:#?}",
        issues
    );
}

/// test_select_option_validation
#[test]
fn select_option_validation() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    parameters:
      - key: method
        type: select
        options: [fast]
"#,
    );
    let params = params_for("a", &[("method", Value::Str("slow".into()))]);

    let issues = validate_pipeline(&pipeline, &tmp.project(), &params);

    assert!(
        issues.iter().any(|issue| issue.message.contains("Invalid option")),
        "{:#?}",
        issues
    );
}

/// test_selector_type_alias_is_validated_as_select
#[test]
fn selector_type_alias_is_validated_as_select() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    parameters:
      - key: method
        type: selector
        options: [fast]
"#,
    );
    let params = params_for("a", &[("method", Value::Str("slow".into()))]);

    let issues = validate_pipeline(&pipeline, &tmp.project(), &params);

    assert_eq!(pipeline.steps[0].parameters[0].kind, FieldType::Select);
    assert!(
        issues.iter().any(|issue| issue.message.contains("Invalid option")),
        "{:#?}",
        issues
    );
}

/// test_placeholder_without_definition_is_blocker
#[test]
fn placeholder_without_definition_is_blocker() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    command: "echo {missing}"
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("command")),
        "{:#?}",
        issues
    );
    assert!(
        issues.iter().any(|issue| issue.message
            == "Placeholder has no input or parameter: missing"
            && issue.severity == IssueSeverity::Blocker),
        "{:#?}",
        issues
    );
}

/// Placeholders with a declaration but no value are blockers on the key.
#[test]
fn placeholder_without_value_is_blocker() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    command: "echo {count}"
    parameters:
      - key: count
        type: integer
        required: false
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.message == "Placeholder has no value: count"
            && issue.field.as_deref() == Some("count")
            && issue.severity == IssueSeverity::Blocker),
        "{:#?}",
        issues
    );
}

// -------------------------------------------------------------- path rules --

/// test_rejects_absolute_working_directory_path
#[test]
fn rejects_absolute_working_directory_path() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    working_directory: /tmp
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("working_directory")
            && issue.severity == IssueSeverity::Blocker
            && issue.message.contains("absolute")),
        "{:#?}",
        issues
    );
}

/// test_rejects_parent_working_directory_path
#[test]
fn rejects_parent_working_directory_path() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    working_directory: ".."
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("working_directory")
            && issue.severity == IssueSeverity::Blocker
            && issue.message.contains("outside")),
        "{:#?}",
        issues
    );
}

/// test_rejects_absolute_input_path
#[test]
fn rejects_absolute_input_path() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        required: true
        default: /etc/passwd
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("input_file")
            && issue.severity == IssueSeverity::Blocker
            && issue.message.contains("absolute")),
        "{:#?}",
        issues
    );
}

/// test_rejects_windows_absolute_input_path_on_every_platform — plus the
/// same Windows absolute form for `working_directory` (every platform).
#[test]
fn rejects_windows_absolute_input_path_on_every_platform() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        required: true
        default: "C:\\outside.txt"
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("input_file")
            && issue.severity == IssueSeverity::Blocker
            && issue.message.contains("absolute")),
        "{:#?}",
        issues
    );

    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    working_directory: "C:\\work"
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("working_directory")
            && issue.severity == IssueSeverity::Blocker
            && issue.message.contains("absolute")),
        "{:#?}",
        issues
    );
}

/// test_rejects_nested_traversal_input_path
#[test]
fn rejects_nested_traversal_input_path() {
    let tmp = TempProject::new();
    let project = tmp.project();
    std::fs::create_dir_all(project.join("safe")).expect("safe dir");
    let outside_name = format!("{}-outside.txt", project.file_name().unwrap().to_string_lossy());
    let outside = tmp.base.join(&outside_name);
    std::fs::write(&outside, "outside").expect("outside file");
    let default = format!("safe/../../{}", outside_name);
    let pipeline = config(&format!(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        required: true
        default: "{default}"
"#
    ));

    let issues = validate_pipeline(&pipeline, &project, &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("input_file")
            && issue.severity == IssueSeverity::Blocker
            && issue.message.contains("outside")),
        "{:#?}",
        issues
    );
}

/// test_rejects_unsafe_output_path
#[test]
fn rejects_unsafe_output_path() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    outputs:
      - key: result_file
        path: ../outside.txt
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("result_file")
            && issue.severity == IssueSeverity::Blocker
            && issue.message.contains("outside")),
        "{:#?}",
        issues
    );
}

/// test_accepts_valid_relative_input_and_output_paths
#[test]
fn accepts_valid_relative_input_and_output_paths() {
    let tmp = TempProject::new();
    let project = tmp.project();
    std::fs::create_dir_all(project.join("Input")).expect("Input dir");
    std::fs::create_dir_all(project.join("outputs")).expect("outputs dir");
    std::fs::write(project.join("Input").join("data.txt"), "data").expect("data file");
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        required: true
        default: Input/data.txt
    outputs:
      - key: result_file
        path: outputs/result.txt
"#,
    );

    let issues = validate_pipeline(&pipeline, &project, &HashMap::new());

    assert_no_blockers(&issues);
}

/// test_preserves_leading_and_trailing_whitespace_in_input_paths
#[test]
fn preserves_leading_and_trailing_whitespace_in_input_paths() {
    let tmp = TempProject::new();
    let project = tmp.project();
    std::fs::write(project.join(" input.txt "), "data").expect("spaced file");
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        required: true
        default: " input.txt "
"#,
    );

    let issues = validate_pipeline(&pipeline, &project, &HashMap::new());

    assert_no_blockers(&issues);
}

/// test_preserves_posix_backslashes_in_input_paths (Python skipif win32)
#[test]
fn preserves_posix_backslashes_in_input_paths() {
    if cfg!(windows) {
        return; // Python: pytest.mark.skipif(sys.platform == "win32")
    }
    let tmp = TempProject::new();
    let project = tmp.project();
    std::fs::write(project.join("input\\file.txt"), "data").expect("backslash file");
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        required: true
        default: "input\\file.txt"
"#,
    );

    let issues = validate_pipeline(&pipeline, &project, &HashMap::new());

    assert_no_blockers(&issues);
}

/// test_preserves_posix_backslashes_when_linking_step_outputs
/// (Python skipif win32; exercises the shared `opm-fs` relpath helper with
/// the same composition `validate_pipeline` uses).
#[test]
fn preserves_posix_backslashes_when_linking_step_outputs() {
    if cfg!(windows) {
        return;
    }
    let tmp = TempProject::new();
    let root = tmp.project();
    let from = opm_fs::resolve_pipeline_path(&root, "consume", &root).expect("target dir");
    let source_dir = opm_fs::resolve_pipeline_path(&root, "produce", &root).expect("source dir");
    let source =
        opm_fs::resolve_pipeline_path(&root, "result\\file.txt", &source_dir).expect("source file");

    let relative = opm_fs::relative_project_path(&root, &from, &source).expect("relatable");

    assert_eq!(relative, "../produce/result\\file.txt");
}

/// test_rejects_whitespace_only_input_paths
#[test]
fn rejects_whitespace_only_input_paths() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        required: true
        default: "   "
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.field.as_deref() == Some("input_file")
            && issue.severity == IssueSeverity::Blocker
            && issue.message.contains("only whitespace")),
        "{:#?}",
        issues
    );
}

/// test_reports_symlink_loop_as_working_directory_blocker
/// (Python: skip when symlinks are unavailable — mirrored by early return.)
#[test]
fn reports_symlink_loop_as_working_directory_blocker() {
    let tmp = TempProject::new();
    let project = tmp.project();
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink("loop", project.join("loop")).is_err() {
            return; // Python: pytest.skip("Symbolic links are not available")
        }
        let pipeline = config(
            r#"
steps:
  - id: a
    name: A
    working_directory: loop
"#,
        );

        let issues = validate_pipeline(&pipeline, &project, &HashMap::new());

        assert!(
            issues.iter().any(|issue| issue.field.as_deref() == Some("working_directory")
                && issue.severity == IssueSeverity::Blocker
                && issue.message.contains("cannot be resolved")),
            "{:#?}",
            issues
        );
    }
    #[cfg(not(unix))]
    {
        let _ = project;
        return; // Python: pytest.skip without symlink support
    }
}

/// test_allows_external_program_paths_in_command
#[test]
fn allows_external_program_paths_in_command() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    command: "/shared/tools/run-analysis.sh --version"
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert_no_blockers(&issues);
}

// -------------------------------------------------------------- key rules --

/// test_duplicate_keys_across_step_fields_are_blockers (3 pytest cases)
#[test]
fn duplicate_keys_across_step_fields_are_blockers() {
    let tmp = TempProject::new();
    let cases = [
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: shared
        type: text
    parameters:
      - key: shared
        type: text
"#,
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: shared
        type: text
    outputs:
      - key: shared
        path: result.txt
"#,
        r#"
steps:
  - id: a
    name: A
    parameters:
      - key: shared
        type: text
    outputs:
      - key: shared
        path: result.txt
"#,
    ];
    for yaml in cases {
        let pipeline = config(yaml);
        let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
        assert!(
            issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("shared")
                    && issue.severity == IssueSeverity::Blocker),
            "{:#?}",
            issues
        );
    }
}

/// test_empty_step_field_keys_are_blockers (inputs/parameters x ""/"   ")
#[test]
fn empty_step_field_keys_are_blockers() {
    let tmp = TempProject::new();
    for field_name in ["inputs", "parameters"] {
        for key in ["", "   "] {
            let yaml = format!(
                r#"
steps:
  - id: a
    name: A
    {field_name}:
      - key: "{key}"
        type: text
"#
            );
            let pipeline = config(&yaml);
            let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
            assert!(
                blockers(&issues)
                    .iter()
                    .any(|issue| issue.message.to_lowercase().contains("key")),
                "field_name={field_name} key={key:?} issues={:#?}",
                issues
            );
        }
    }
}

/// test_empty_output_paths_are_blockers ("" and whitespace)
#[test]
fn empty_output_paths_are_blockers() {
    let tmp = TempProject::new();
    for path in ["", "   "] {
        let yaml = format!(
            r#"
steps:
  - id: a
    name: A
    outputs:
      - key: result
        path: "{path}"
"#
        );
        let pipeline = config(&yaml);
        let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
        assert!(
            issues.iter().any(|issue| issue.field.as_deref() == Some("result")
                && issue.severity == IssueSeverity::Blocker),
            "path={path:?} issues={:#?}",
            issues
        );
    }
}

/// test_unkeyed_output_remains_valid_after_model_round_trip
#[test]
fn unkeyed_output_remains_valid_after_model_round_trip() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    outputs:
      - path: result.txt
"#,
    );
    let json = serde_json::to_string(&pipeline).expect("serializes");
    assert!(json.contains("\"key\":\"\""), "{}", json);
    let reloaded = parse_pipeline("pipeline.json", &json).expect("round-trips");

    let issues = validate_pipeline(&reloaded, &tmp.project(), &HashMap::new());

    assert_no_blockers(&issues);
}

/// test_whitespace_only_output_key_is_blocker
#[test]
fn whitespace_only_output_key_is_blocker() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    outputs:
      - key: "   "
        path: result.txt
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        blockers(&issues)
            .iter()
            .any(|issue| issue.message.to_lowercase().contains("output key")),
        "{:#?}",
        issues
    );
}

// ------------------------------------------- non-file value spec validation --

/// test_non_file_inputs_use_value_spec_validation (5 pytest cases)
#[test]
fn non_file_inputs_use_value_spec_validation() {
    let tmp = TempProject::new();
    // (spec yaml fragment, submitted value, expected message fragment)
    let cases: [(&str, Option<Value>, &str); 5] = [
        ("key: title\n        type: text\n        required: true", None, "required"),
        (
            "key: count\n        type: integer",
            Some(Value::Str("not-a-number".into())),
            "numeric",
        ),
        ("key: count\n        type: integer", Some(Value::Float(1.5)), "integer"),
        (
            "key: ratio\n        type: decimal\n        min: 0\n        max: 1",
            Some(Value::Int(2)),
            "maximum",
        ),
        (
            "key: mode\n        type: select\n        options: [fast]",
            Some(Value::Str("slow".into())),
            "invalid option",
        ),
    ];
    for (spec_yaml, submitted, expected) in cases {
        let yaml = format!(
            r#"
steps:
  - id: a
    name: A
    inputs:
      - {spec_yaml}
"#
        );
        let pipeline = config(&yaml);
        let spec_key = pipeline.steps[0].inputs[0].key.clone();
        let submitted_map = match submitted {
            Some(value) => params_for("a", &[(spec_key.as_str(), value)]),
            None => HashMap::new(),
        };
        let issues = validate_pipeline(&pipeline, &tmp.project(), &submitted_map);
        assert!(
            issues.iter().any(|issue| issue.field.as_deref() == Some(spec_key.as_str())
                && issue.severity == IssueSeverity::Blocker
                && issue.message.to_lowercase().contains(expected)),
            "expected fragment {expected:?}, issues={:#?}",
            issues
        );
    }
}

/// test_non_numeric_values_and_non_finite_numbers_are_blockers (5 cases:
/// true, false, NaN, inf, -inf — all native `Value`s).
#[test]
fn non_numeric_values_and_non_finite_numbers_are_blockers() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    parameters:
      - key: threshold
        type: decimal
"#,
    );
    let values = [
        Value::Bool(true),
        Value::Bool(false),
        Value::Float(f64::NAN),
        Value::Float(f64::INFINITY),
        Value::Float(f64::NEG_INFINITY),
    ];
    for value in values {
        let params = params_for("a", &[("threshold", value)]);
        let issues = validate_pipeline(&pipeline, &tmp.project(), &params);
        assert!(
            issues
                .iter()
                .any(|issue| issue.field.as_deref() == Some("threshold")
                    && issue.severity == IssueSeverity::Blocker),
            "{:#?}",
            issues
        );
    }
}

/// test_invalid_boolean_defaults_and_submitted_values_are_blockers
/// (inputs/parameters x default/submitted x "true","false","yes","no",0,1)
#[test]
fn invalid_boolean_defaults_and_submitted_values_are_blockers() {
    let tmp = TempProject::new();
    let invalid_values: [Value; 6] = [
        Value::Str("true".into()),
        Value::Str("false".into()),
        Value::Str("yes".into()),
        Value::Str("no".into()),
        Value::Int(0),
        Value::Int(1),
    ];
    for field_name in ["inputs", "parameters"] {
        for invalid in invalid_values.iter() {
            // (a) default in the declaration — quoted strings stay strings,
            // numbers stay numbers.
            let default_yaml = match invalid {
                Value::Str(text) => format!("\"{text}\""),
                Value::Int(0) => "0".to_owned(),
                Value::Int(1) => "1".to_owned(),
                _ => unreachable!(),
            };
            let yaml = format!(
                r#"
steps:
  - id: a
    name: A
    {field_name}:
      - key: enabled
        type: boolean
        default: {default_yaml}
"#
            );
            let pipeline = config(&yaml);
            let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
            assert!(
                issues.iter().any(|issue| issue.field.as_deref() == Some("enabled")
                    && issue.severity == IssueSeverity::Blocker
                    && issue.message.to_lowercase().contains("boolean")),
                "default={invalid:?} field={field_name} issues={:#?}",
                issues
            );

            // (b) submitted value.
            let yaml = format!(
                r#"
steps:
  - id: a
    name: A
    {field_name}:
      - key: enabled
        type: boolean
"#
            );
            let pipeline = config(&yaml);
            let params = params_for("a", &[("enabled", invalid.clone())]);
            let issues = validate_pipeline(&pipeline, &tmp.project(), &params);
            assert!(
                issues.iter().any(|issue| issue.field.as_deref() == Some("enabled")
                    && issue.severity == IssueSeverity::Blocker
                    && issue.message.to_lowercase().contains("boolean")),
                "submitted={invalid:?} field={field_name} issues={:#?}",
                issues
            );
        }
    }
}

/// test_native_boolean_defaults_and_submitted_values_are_valid
/// (inputs/parameters x default/submitted x true/false)
#[test]
fn native_boolean_defaults_and_submitted_values_are_valid() {
    let tmp = TempProject::new();
    for field_name in ["inputs", "parameters"] {
        for boolean in [true, false] {
            let yaml = format!(
                r#"
steps:
  - id: a
    name: A
    {field_name}:
      - key: enabled
        type: boolean
        default: {boolean}
"#
            );
            let pipeline = config(&yaml);
            let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
            assert!(
                !issues.iter().any(|issue| issue.field.as_deref() == Some("enabled")
                    && issue.severity == IssueSeverity::Blocker),
                "default={boolean} field={field_name} issues={:#?}",
                issues
            );

            let yaml = format!(
                r#"
steps:
  - id: a
    name: A
    {field_name}:
      - key: enabled
        type: boolean
"#
            );
            let pipeline = config(&yaml);
            let params = params_for("a", &[("enabled", Value::Bool(boolean))]);
            let issues = validate_pipeline(&pipeline, &tmp.project(), &params);
            assert!(
                !issues.iter().any(|issue| issue.field.as_deref() == Some("enabled")
                    && issue.severity == IssueSeverity::Blocker),
                "submitted={boolean} field={field_name} issues={:#?}",
                issues
            );
        }
    }
}

/// test_inverted_numeric_ranges_are_blockers (inputs/parameters)
#[test]
fn inverted_numeric_ranges_are_blockers() {
    let tmp = TempProject::new();
    for field_name in ["inputs", "parameters"] {
        let yaml = format!(
            r#"
steps:
  - id: a
    name: A
    {field_name}:
      - key: threshold
        type: decimal
        min: 10
        max: 1
        default: 5
"#
        );
        let pipeline = config(&yaml);
        let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
        assert!(
            issues.iter().any(|issue| issue.field.as_deref() == Some("threshold")
                && issue.severity == IssueSeverity::Blocker
                && issue.message.to_lowercase().contains("minimum")),
            "field={field_name} issues={:#?}",
            issues
        );
    }
}

/// test_non_finite_numeric_bounds_are_blockers
/// (inputs/parameters x min/max x NaN, inf, -inf)
#[test]
fn non_finite_numeric_bounds_are_blockers() {
    let tmp = TempProject::new();
    for field_name in ["inputs", "parameters"] {
        for bound_name in ["min", "max"] {
            for bound_value in [".nan", ".inf", "-.inf"] {
                let yaml = format!(
                    r#"
steps:
  - id: a
    name: A
    {field_name}:
      - key: threshold
        type: decimal
        {bound_name}: {bound_value}
"#
                );
                let pipeline = config(&yaml);
                let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
                assert!(
                    issues.iter().any(|issue| issue.field.as_deref() == Some("threshold")
                        && issue.severity == IssueSeverity::Blocker
                        && issue.message.to_lowercase().contains("finite")),
                    "bound={bound_name} value={bound_value} field={field_name} issues={:#?}",
                    issues
                );
            }
        }
    }
}

/// test_non_finite_numeric_defaults_and_submitted_values_are_blockers
/// (inputs/parameters x default/submitted x NaN, inf, -inf)
#[test]
fn non_finite_numeric_defaults_and_submitted_values_are_blockers() {
    let tmp = TempProject::new();
    let non_finite = [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for field_name in ["inputs", "parameters"] {
        for value in non_finite {
            // (a) YAML default `.nan`/`.inf`/`-.inf` (native Float in Value).
            let yaml = format!(
                r#"
steps:
  - id: a
    name: A
    {field_name}:
      - key: threshold
        type: decimal
        default: {value_yaml}
"#
                ,
                value_yaml = match () {
                    _ if value.is_nan() => ".nan",
                    _ if value > 0.0 => ".inf",
                    _ => "-.inf",
                }
            );
            let pipeline = config(&yaml);
            let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
            assert!(
                issues.iter().any(|issue| issue.field.as_deref() == Some("threshold")
                    && issue.severity == IssueSeverity::Blocker
                    && issue.message.to_lowercase().contains("finite")),
                "default={value} field={field_name} issues={:#?}",
                issues
            );

            // (b) submitted value.
            let yaml = format!(
                r#"
steps:
  - id: a
    name: A
    {field_name}:
      - key: threshold
        type: decimal
"#
            );
            let pipeline = config(&yaml);
            let params = params_for("a", &[("threshold", Value::Float(value))]);
            let issues = validate_pipeline(&pipeline, &tmp.project(), &params);
            assert!(
                issues.iter().any(|issue| issue.field.as_deref() == Some("threshold")
                    && issue.severity == IssueSeverity::Blocker
                    && issue.message.to_lowercase().contains("finite")),
                "submitted={value} field={field_name} issues={:#?}",
                issues
            );
        }
    }
}

/// test_valid_non_file_input_and_parameter_values_do_not_create_blockers
#[test]
fn valid_non_file_input_and_parameter_values_do_not_create_blockers() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: title
        type: text
        required: true
        default: report
      - key: count
        type: integer
        min: 1
        max: 5
        default: 3
      - key: ratio
        type: decimal
        min: 0
        max: 1
        default: 0.25
      - key: mode
        type: select
        options: [fast, safe]
        default: fast
      - key: enabled
        type: boolean
        required: true
        default: false
    parameters:
      - key: retries
        type: integer
        min: 0
        max: 3
        default: 2
      - key: dry_run
        type: boolean
        default: true
    outputs:
      - key: result
        path: outputs/result.txt
      - path: outputs/unnamed.txt
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert_no_blockers(&issues);
}

// ------------------------------------------------- source-link and extras --

/// Source-linked inputs resolve to project-relative paths and skip the
/// existence check (validation.py `_source_input_values` + file-input rule);
/// unknown source steps stay blockers via `_validate_source_refs`.
#[test]
fn source_linked_input_without_source_is_blocked_and_known_source_skips_existence() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: produce
    name: Produce
    working_directory: .
    outputs:
      - key: result
        path: outputs/result.txt
  - id: consume
    name: Consume
    working_directory: .
    inputs:
      - key: result_file
        type: file
        required: true
        source_step: produce
        source_output: result
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    // The linked output file does not exist on disk, yet no existence blocker
    // is reported for the source-linked input.
    assert_no_blockers(&issues);

    let pipeline = config(
        r#"
steps:
  - id: consume
    name: Consume
    inputs:
      - key: result_file
        type: file
        required: true
        source_step: ghost
        source_output: result
"#,
    );
    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
    assert!(
        issues.iter().any(|issue| issue.message == "Input source step does not exist: ghost"
            && issue.field.as_deref() == Some("result_file")
            && issue.severity == IssueSeverity::Blocker),
        "{:#?}",
        issues
    );
}

/// Half-declared source references are blockers (validation.py
/// `_validate_source_refs`).
#[test]
fn half_declared_source_reference_is_blocker() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: input_file
        type: file
        source_step: other
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.message
            == "Input source must include source_step and source_output: input_file"
            && issue.severity == IssueSeverity::Blocker),
        "{:#?}",
        issues
    );
}

/// Source output must exist by key or path on the source step.
#[test]
fn missing_source_output_is_blocker() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: produce
    name: Produce
    outputs:
      - key: result
        path: result.txt
  - id: consume
    name: Consume
    inputs:
      - key: result_file
        type: file
        source_step: produce
        source_output: ghost
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.message == "Input source output does not exist: ghost"
            && issue.field.as_deref() == Some("result_file")
            && issue.severity == IssueSeverity::Blocker),
        "{:#?}",
        issues
    );
}

/// `placeholders` exposes the public regex shape.
#[test]
fn public_placeholders_helper_matches_the_regex() {
    assert_eq!(
        placeholders("echo {a} {b} {a} {kebab-1} {_u}"),
        vec!["a", "b", "kebab-1", "_u"]
    );
    assert!(placeholders("echo 123 {} {1bad}").is_empty());
}

// --------------------------------------------- unit-level model behavior --

/// `step_by_id` follows Python dict-comprehension semantics (last wins) and
/// `default_values`/`output_values` mirror `pipeline.py` exactly.
#[test]
fn model_accessors_match_pipeline_py() {
    let pipeline = config(
        r#"
steps:
  - id: first
    name: First
    inputs:
      - key: a
        default: 1
      - key: b
        default: null
      - key: c
        default: false
    parameters:
      - key: a
        default: 2
    outputs:
      - key: out
        path: out.txt
      - path: unnamed.txt
"#,
    );
    let step = pipeline.step_by_id("first").expect("step exists");
    assert_eq!(pipeline.step_by_id("ghost"), None);

    let defaults = step.default_values();
    // Null default skipped; params override inputs for duplicate keys;
    // `false` is a present value.
    assert_eq!(defaults.get("a"), Some(&Value::Int(2)));
    assert_eq!(defaults.get("b"), None);
    assert_eq!(defaults.get("c"), Some(&Value::Bool(false)));

    let outputs = step.output_values();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs.get("out"), Some(&Value::Str("out.txt".into())));

    // Effective values: outputs always win over params/defaults.
    let effective = opm_pipeline::effective_values(step, &params(&[("out", Value::Str("override".into()))]));
    assert_eq!(effective.get("out"), Some(&Value::Str("out.txt".into())));
}

/// Missing working directory is a blocker with the raw path in the message.
#[test]
fn missing_working_directory_is_blocker() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    working_directory: does-not-exist
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.message == "Working directory does not exist: does-not-exist"
            && issue.field.as_deref() == Some("working_directory")
            && issue.severity == IssueSeverity::Blocker),
        "{:#?}",
        issues
    );
}

/// A required file input whose file exists passes; a missing one is a
/// blocker, an optional missing one a warning (existence rules).
#[test]
fn file_existence_rules_match_severity_by_required() {
    let tmp = TempProject::new();
    let project = tmp.project();
    std::fs::write(project.join("present.txt"), "x").expect("file");
    let pipeline = config(
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: required_missing
        type: file
        required: true
        default: absent.txt
      - key: optional_missing
        type: file
        required: false
        default: absent2.txt
      - key: optional_present
        type: file
        required: false
        default: present.txt
"#,
    );

    let issues = validate_pipeline(&pipeline, &project, &HashMap::new());

    assert!(
        issues.iter().any(|issue| issue.message == "required_missing does not exist: absent.txt"
            && issue.severity == IssueSeverity::Blocker),
        "{:#?}",
        issues
    );
    assert!(
        issues.iter().any(|issue| issue.message == "optional_missing does not exist: absent2.txt"
            && issue.severity == IssueSeverity::Warning),
        "{:#?}",
        issues
    );
    assert!(
        !issues
            .iter()
            .any(|issue| issue.field.as_deref() == Some("optional_present")),
        "{:#?}",
        issues
    );
}

// ------------------------------------- unsupported shell contexts (F1) and ids (F3)

/// F1 (extensión de la regla de `command`): un placeholder en comentario,
/// `$()`/`${...}`/backticks o cuerpo de heredoc es bloqueo con el shape de
/// los demás problemas de `command` (severidad, `field`, paso nombrado).
#[test]
fn placeholder_in_unsupported_shell_context_is_a_blocker() {
    let tmp = TempProject::new();
    let cases = [
        "echo ok # note for {label}",
        r#"printf '%s' "$(printf '%s' {label})""#,
        "cat <<EOF\n{label}\nEOF",
        "echo `run {label}`",
        "echo ${label}",
    ];
    for command in cases {
        let yaml = format!(
            "steps:\n  - id: a\n    name: A\n    command: {command:?}\n    parameters:\n      - key: label\n        type: text\n        required: false\n        default: \"hello; touch injected; #\"\n"
        );
        let pipeline = config(&yaml);
        let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
        let matching: Vec<&ValidationIssue> = blockers(&issues)
            .into_iter()
            .filter(|issue| issue.field.as_deref() == Some("command"))
            .collect();
        assert_eq!(matching.len(), 1, "command {command:?}: {issues:#?}");
        assert_eq!(
            matching[0].message,
            "Placeholder sits in a shell context OPM cannot substitute safely (comment, $() or heredoc): label"
        );
        assert_eq!(matching[0].step_id.as_deref(), Some("a"));
    }
}

/// El PoC del revisor con el placeholder FUERA del comentario valida limpio:
/// la regla no gera falsos positivos sobre comandos con comentarios (la
/// seguridad la aporta la sustitución cotexto-consciente, probada en
/// `opm-exec`).
#[test]
fn comment_before_a_placeholder_does_not_block() {
    let tmp = TempProject::new();
    let pipeline = config(
        "steps:\n  - id: a\n    name: A\n    working_directory: \".\"\n    command: \"# user's pipeline\\nprintf '%s' {label}\"\n    parameters:\n      - key: label\n        type: text\n        required: false\n        default: \"hello; touch injected; #\"\n",
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
    assert_no_blockers(&issues);
}

/// F3: los dos ids PoC del revisor (`C:\Users\victim\x`, `..\..\x`) producen
/// bloqueos que nombran el paso, en importación y vía validación directa.
#[test]
fn separator_step_ids_are_blockers_naming_the_step() {
    let tmp = TempProject::new();
    let pipeline = config(
        r#"
steps:
  - id: "C:\\Users\\victim\\x"
    name: Windows
    working_directory: "."
  - id: "..\\..\\x"
    name: Relative
    working_directory: "."
"#,
    );

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
    for id in ["C:\\Users\\victim\\x", "..\\..\\x"] {
        assert!(
            blockers(&issues).iter().any(|issue| issue.field.as_deref() == Some("id")
                && issue.step_id.as_deref() == Some(id)
                && issue.message == format!("Step id contains a path separator: {}", id)),
            "missing blocker for {id:?}: {issues:#?}"
        );
    }
}

/// F3: también un byte nulo en el id.
#[test]
fn null_byte_in_step_id_is_a_blocker() {
    let tmp = TempProject::new();
    let pipeline = opm_pipeline::parse_pipeline(
        "pipeline.json",
        r#"{"steps": [{"id": "a\u0000b", "name": "Nul"}]}"#,
    )
    .expect("json parses");

    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
    assert!(
        blockers(&issues).iter().any(|issue| issue.field.as_deref() == Some("id")
            && issue.message == "Step id contains an invalid null byte: a\u{0}b"),
        "{issues:#?}"
    );
}

// --------------------------------------------- long chains (F8, iteratividad)

/// Construye una cadena lineal de `count` pasos: `s_i` depende de `s_{i+1}`;
/// opcionalmente el último cierra el ciclo contra `tail_dep`.
fn long_chain(count: usize, tail_dep: Option<&str>) -> PipelineConfig {
    let mut yaml = String::from("steps:\n");
    for i in 0..count {
        yaml.push_str(&format!(
            "  - id: s{i}\n    name: S{i}\n    working_directory: \".\"\n"
        ));
        if i + 1 < count {
            yaml.push_str(&format!("    dependencies: [s{}]\n", i + 1));
        } else if let Some(dep) = tail_dep {
            yaml.push_str(&format!("    dependencies: [{dep}]\n"));
        }
    }
    config(&yaml)
}

/// F8: una cadena de 10.000 pasos (fichero sin validar) valida sin
/// desbordar la pila — el DFS de ciclos es iterativo.
#[test]
fn long_dependency_chain_validates_iteratively() {
    let tmp = TempProject::new();
    let pipeline = long_chain(10_000, None);
    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());
    assert_no_blockers(&issues);
}

/// F8: la misma cadena con el ciclo cerrado en la cola — UN bloqueo, con el
/// message shape `A -> B -> A` idéntico al de los ciclos pequeños.
#[test]
fn long_dependency_chain_with_tail_cycle_reports_one_blocker() {
    let tmp = TempProject::new();
    let pipeline = long_chain(10_000, Some("s0"));
    let issues = validate_pipeline(&pipeline, &tmp.project(), &HashMap::new());

    let cycles: Vec<&ValidationIssue> = blockers(&issues)
        .into_iter()
        .filter(|issue| issue.message.starts_with("Dependency cycle detected: "))
        .collect();
    assert_eq!(cycles.len(), 1, "unexpected cycle blockers: {cycles:#?}");
    assert_eq!(cycles[0].step_id.as_deref(), Some("s0"));
    assert_eq!(cycles[0].field.as_deref(), Some("dependencies"));
    let message = &cycles[0].message;
    assert!(
        message.starts_with("Dependency cycle detected: s0 -> ")
            && message.ends_with(" -> s0"),
        "bad shape: {message:.80}…"
    );
}
