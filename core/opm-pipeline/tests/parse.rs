//! Parser contract — ports `backend/tests/test_pipeline_parser.py` plus the
//! parsing rules pinned by the contract (empty YAML, top-level shape,
//! unsupported suffix, unknown-field tolerance, wire-schema parity).

use opm_pipeline::{parse_pipeline, FieldType, ParseError, Value};

/// test_parse_pipeline_yml
#[test]
fn parse_pipeline_yml() {
    let pipeline = parse_pipeline(
        "pipeline.yml",
        r#"
steps:
  - id: preprocess
    name: Preprocess
    command: "echo {input_file}"
    inputs:
      - key: input_file
        type: file
        required: true
"#,
    )
    .expect("valid yml parses");

    assert_eq!(pipeline.steps[0].id, "preprocess");
    assert!(pipeline.steps[0].inputs[0].required);
}

/// test_parse_pipeline_json
#[test]
fn parse_pipeline_json() {
    let pipeline = parse_pipeline(
        "pipeline.json",
        r#"{"steps": [{"id": "analyze", "name": "Analyze", "command": "echo ok"}]}"#,
    )
    .expect("valid json parses");

    assert_eq!(pipeline.steps[0].name, "Analyze");
}

/// Contract: empty YAML file == `{}`.
#[test]
fn empty_yaml_file_parses_as_empty_config() {
    let pipeline = parse_pipeline("pipeline.yml", "").expect("empty file is an empty pipeline");
    assert!(pipeline.steps.is_empty());

    let null_doc = parse_pipeline("pipeline.yaml", "null\n").expect("null is an empty pipeline");
    assert!(null_doc.steps.is_empty());
}

/// Contract: non-object top level -> ParseError::NotAnObject (YAML and JSON).
#[test]
fn non_object_top_level_is_rejected() {
    let list = parse_pipeline("pipeline.yml", "- id: a\n").expect_err("yaml list is not an object");
    assert!(matches!(list, ParseError::NotAnObject), "{:?}", list);
    assert_eq!(list.to_string(), "Pipeline file must contain an object at the top level");

    let array = parse_pipeline("pipeline.json", "[1, 2]").expect_err("json array is not an object");
    assert!(matches!(array, ParseError::NotAnObject), "{:?}", array);

    let scalar = parse_pipeline("pipeline.json", "5").expect_err("json scalar is not an object");
    assert!(matches!(scalar, ParseError::NotAnObject), "{:?}", scalar);
}

/// Contract: unsupported suffix -> ParseError::UnsupportedFormat with the
/// Python message text.
#[test]
fn unsupported_suffix_is_rejected() {
    let err = parse_pipeline("pipeline.txt", "steps: []").expect_err("txt is unsupported");
    assert!(matches!(&err, ParseError::UnsupportedFormat(suffix) if suffix == ".txt"), "{:?}", err);
    assert_eq!(err.to_string(), "Unsupported pipeline format: .txt");

    let no_suffix = parse_pipeline("README", "").expect_err("no suffix is unsupported");
    assert!(matches!(&no_suffix, ParseError::UnsupportedFormat(suffix) if suffix == ""));
}

/// Deviation pinned as behavior: strict JSON rejects bare NaN/Infinity tokens
/// (Python `json.loads` accepts them); the error names the file.
#[test]
fn json_nan_tokens_are_rejected_naming_the_file() {
    let err = parse_pipeline(
        "/projects/demo/pipeline.json",
        r#"{"steps": [{"id": "a", "name": "A", "inputs": [{"key": "k", "type": "decimal", "default": NaN}]}]}"#,
    )
    .expect_err("bare NaN is not valid strict JSON");
    match &err {
        ParseError::InvalidJson { file, .. } => assert_eq!(file, "/projects/demo/pipeline.json"),
        other => panic!("expected InvalidJson, got {:?}", other),
    }
    assert!(err.to_string().contains("/projects/demo/pipeline.json"));
}

/// Pydantic default: unknown fields are ignored (model and step level).
#[test]
fn unknown_fields_are_ignored() {
    let pipeline = parse_pipeline(
        "pipeline.json",
        r#"{
            "steps": [{"id": "a", "name": "A", "unexpected": {"deep": [1, 2]}}],
            "also_unexpected": true
        }"#,
    )
    .expect("unknown fields must not fail the parse");
    assert_eq!(pipeline.steps.len(), 1);
    assert_eq!(pipeline.steps[0].id, "a");
}

/// Contract: `selector` normalizes to `Select`; wire name is `type`.
#[test]
fn selector_normalizes_to_select() {
    let pipeline = parse_pipeline(
        "pipeline.yml",
        r#"
steps:
  - id: a
    name: A
    parameters:
      - key: method
        type: selector
        options: [fast, safe]
"#,
    )
    .expect("selector parses");
    assert_eq!(pipeline.steps[0].parameters[0].kind, FieldType::Select);

    let again = parse_pipeline(
        "pipeline.json",
        r#"{"steps": [{"id": "a", "name": "A", "parameters": [{"key": "m", "type": "selector"}]}]}"#,
    )
    .expect("selector parses in json too");
    assert_eq!(again.steps[0].parameters[0].kind, FieldType::Select);
}

/// An unknown type value is a parse error (Pydantic Literal rejects it).
#[test]
fn unknown_field_type_is_rejected() {
    let err = parse_pipeline(
        "pipeline.yml",
        "steps:\n  - id: a\n    name: A\n    parameters:\n      - key: m\n        type: bogus\n",
    )
    .expect_err("bogus type is not a FieldType");
    assert!(matches!(err, ParseError::Yaml(_)), "{:?}", err);
}

/// Wire parity (A5): `type` on the wire, `min`/`max` on the wire (also
/// accepting `minimum`/`maximum` on input), output `key` always serialized,
/// Option fields serialized as null (Python by_alias model_dump shape).
#[test]
fn wire_schema_matches_python_export() {
    let pipeline = parse_pipeline(
        "pipeline.yml",
        r#"
steps:
  - id: a
    name: A
    inputs:
      - key: count
        minimum: 1
        maximum: 5
    outputs:
      - path: outputs/unnamed.txt
"#,
    )
    .expect("alias keys parse");

    let spec = &pipeline.steps[0].inputs[0];
    assert_eq!(spec.min, Some(1.0));
    assert_eq!(spec.max, Some(5.0));

    let json = serde_json::to_string(&pipeline).expect("serializes");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    let step = &parsed["steps"][0];
    assert_eq!(step["inputs"][0]["type"], "text");
    assert_eq!(step["inputs"][0]["min"], 1.0);
    assert_eq!(step["inputs"][0]["max"], 5.0);
    assert_eq!(step["inputs"][0]["label"], serde_json::Value::Null);
    assert_eq!(step["inputs"][0]["default"], serde_json::Value::Null);
    assert_eq!(step["outputs"][0]["key"], "");
    assert_eq!(step["working_directory"], ".");
    // minimum/maximum are accepted on input but never emitted.
    assert!(parsed.to_string().find("\"minimum\"").is_none());
}

/// YAML `.nan`/`.inf` flow into `Value::Float` natively (strict booleans and
/// nonfinite bounds stay exact through the parser).
#[test]
fn yaml_nonfinite_defaults_and_bounds_are_native_floats() {
    let pipeline = parse_pipeline(
        "pipeline.yml",
        r#"
steps:
  - id: a
    name: A
    parameters:
      - key: threshold
        type: decimal
        default: .nan
      - key: ratio
        type: decimal
        min: .inf
      - key: enabled
        type: boolean
        default: true
"#,
    )
    .expect("nonfinite yaml parses");

    let params = &pipeline.steps[0].parameters;
    match params[0].default {
        Some(Value::Float(value)) => assert!(value.is_nan()),
        ref other => panic!("expected Float(NaN), got {:?}", other),
    }
    assert_eq!(params[1].min, Some(f64::INFINITY));
    match params[2].default {
        Some(Value::Bool(true)) => {}
        ref other => panic!("expected Bool(true), got {:?}", other),
    }
}

/// Strict booleans on the wire: quoted "true" stays a string (DECISIONS.md).
#[test]
fn quoted_boolean_defaults_stay_strings() {
    let pipeline = parse_pipeline(
        "pipeline.yml",
        r#"
steps:
  - id: a
    name: A
    parameters:
      - key: enabled
        type: boolean
        default: "true"
"#,
    )
    .expect("parses");
    match &pipeline.steps[0].parameters[0].default {
        Some(Value::Str(text)) => assert_eq!(text, "true"),
        other => panic!("expected Str, got {:?}", other),
    }
}

/// Round-trip: a serialized pipeline reparses to an equal config (Python
/// `model_validate(model_dump(...))`).
#[test]
fn serialization_round_trip_preserves_the_model() {
    let original = parse_pipeline(
        "pipeline.yml",
        r#"
steps:
  - id: produce
    name: Produce
    outputs:
      - key: result
        label: Result
        path: result.txt
      - path: outputs/unnamed.txt
    parameters:
      - key: method
        type: select
        options: [fast, safe]
        default: fast
"#,
    )
    .expect("fixture parses");

    let json = serde_json::to_string(&original).expect("serializes");
    let reloaded = parse_pipeline("pipeline.json", &json).expect("round-trips");
    assert_eq!(reloaded, original);
}
