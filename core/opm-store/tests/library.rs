//! `PipelineLibrary` behavior: native `.pipeline` documents (round-trip,
//! naming conflicts, YAML/JSON import, explicit JSON/YAML export).

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::temp_dir;
use opm_pipeline::{FieldType, OutputSpec, PipelineConfig, PipelineStep, Value, ValueSpec};
use opm_store::{ExportFormat, PipelineDoc, PipelineLibrary, Position, SaveMode, StoreError};

fn library(tag: &str) -> (PathBuf, PipelineLibrary) {
    let parent = temp_dir(tag);
    let library = PipelineLibrary::new(parent.join("pipelines"));
    (parent, library)
}

fn cleanup(parent: &Path) {
    let _ = fs::remove_dir_all(parent);
}

fn str_value(value: &str) -> Value {
    Value::Str(value.to_string())
}

fn sample_pipeline() -> PipelineConfig {
    PipelineConfig {
        steps: vec![
            PipelineStep {
                id: "first".to_string(),
                name: "First".to_string(),
                description: "first step".to_string(),
                command: "echo {sample}".to_string(),
                environment: String::new(),
                working_directory: ".".to_string(),
                inputs: vec![ValueSpec {
                    key: "sample".to_string(),
                    label: None,
                    kind: FieldType::Text,
                    required: false,
                    default: Some(str_value("hello")),
                    options: Vec::new(),
                    min: None,
                    max: None,
                    description: String::new(),
                    source_step: None,
                    source_output: None,
                }],
                outputs: vec![OutputSpec {
                    key: String::new(),
                    label: None,
                    path: "out.txt".to_string(),
                }],
                parameters: Vec::new(),
                dependencies: Vec::new(),
            },
            PipelineStep {
                id: "second".to_string(),
                name: "Second".to_string(),
                description: String::new(),
                command: "cat out.txt".to_string(),
                environment: String::new(),
                working_directory: ".".to_string(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                parameters: Vec::new(),
                dependencies: vec!["first".to_string()],
            },
        ],
    }
}

// ------------------------------------------------------------------ round trip

/// Save → load keeps name, description, steps, layout and the envelope
/// version; the file is UTF-8 JSON under `.pipeline`.
#[test]
fn library_roundtrip() {
    let (parent, library) = library("roundtrip");
    let mut doc = PipelineDoc::new("demo", sample_pipeline());
    doc.description = "Reusable demo pipeline".to_string();
    doc.layout.insert(
        "first".to_string(),
        Position {
            x: 12.5,
            y: 34.5,
        },
    );
    doc.layout.insert(
        "second".to_string(),
        Position {
            x: 400.0,
            y: 120.0,
        },
    );
    library
        .save("demo", &doc, SaveMode::CreateNew)
        .expect("first save");

    let file = parent.join("pipelines/demo.pipeline");
    assert!(file.is_file());
    let text = fs::read_to_string(&file).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).expect("UTF-8 JSON encoding");
    assert_eq!(raw["format_version"], serde_json::json!(1));
    assert_eq!(raw["name"], serde_json::json!("demo"));

    let loaded = library.load("demo").expect("load back");
    assert_eq!(loaded.format_version, 1);
    assert_eq!(loaded.name, "demo");
    assert_eq!(loaded.description, "Reusable demo pipeline");
    assert_eq!(loaded.pipeline, doc.pipeline);
    assert_eq!(loaded.layout, doc.layout);
    // Save writes the document verbatim (timestamps included).
    assert_eq!(loaded, doc);
    assert_eq!(library.list(), ["demo"]);

    cleanup(&parent);
}

// -------------------------------------------------------------- naming conflict

/// `CreateNew` over an existing name fails with `NameExists`; `Overwrite`
/// replaces the document atomically; empty names are rejected.
#[test]
fn library_naming_conflict() {
    let (parent, library) = library("conflict");
    let doc = PipelineDoc::new("demo", sample_pipeline());
    library
        .save("demo", &doc, SaveMode::CreateNew)
        .expect("first save");

    match library.save("demo", &doc, SaveMode::CreateNew) {
        Err(StoreError::NameExists(name)) => assert_eq!(name, "demo"),
        other => panic!("expected NameExists, got {other:?}"),
    }
    // The conflict is on the sanitized file stem: a `.pipeline` suffix on
    // the requested name maps onto the same document.
    match library.save("demo.pipeline", &doc, SaveMode::CreateNew) {
        Err(StoreError::NameExists(name)) => assert_eq!(name, "demo"),
        other => panic!("expected NameExists, got {other:?}"),
    }

    // Overwrite replaces atomically: new content lands, no staged temp file
    // is left behind.
    let mut updated = doc.clone();
    updated.description = "second edition".to_string();
    library
        .save("demo", &updated, SaveMode::Overwrite)
        .expect("overwrite");
    let reloaded = library.load("demo").unwrap();
    assert_eq!(reloaded.description, "second edition");
    assert_eq!(reloaded.pipeline, doc.pipeline);
    let leftovers: Vec<String> = fs::read_dir(parent.join("pipelines"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".opm-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "staged temp left: {leftovers:?}");

    // New names sanitize to a single safe segment (whitespace → `_`).
    library
        .save("My Other Pipe", &doc, SaveMode::CreateNew)
        .expect("sanitized name");
    assert_eq!(library.list(), ["My_Other_Pipe", "demo"]);

    // Empty names are rejected outright.
    for blank in ["", "   "] {
        match library.save(blank, &doc, SaveMode::CreateNew) {
            Err(StoreError::Message(message)) => {
                assert!(message.contains("name"), "message: {message}")
            }
            other => panic!("expected name-required error for {blank:?}, got {other:?}"),
        }
    }

    cleanup(&parent);
}

// ------------------------------------------------------------------ import

/// Import `examples/first-run.json` (the shipped importable shape) and a
/// YAML variant into native documents; imports never silently overwrite.
#[test]
fn library_import_yaml_json() {
    let (parent, library) = library("import");

    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/first-run.json");
    let original = fs::read(&source).unwrap();
    let doc = library.import_file(&source, "first-run").expect("import json");
    assert_eq!(doc.format_version, 1);
    assert_eq!(doc.name, "first-run");
    assert_eq!(doc.pipeline.steps.len(), 2);
    assert_eq!(doc.pipeline.steps[0].id, "create_message");
    assert_eq!(doc.pipeline.steps[1].id, "write_report");
    assert_eq!(doc.pipeline.steps[1].dependencies, ["create_message"]);
    // Default layout is filled for every declared step.
    assert_eq!(doc.layout.len(), 2);
    assert!(library.list().contains(&"first-run".to_string()));
    // The native document on disk round-trips the parsed pipeline.
    let saved = library.load("first-run").unwrap();
    assert_eq!(saved.pipeline, doc.pipeline);
    // An import never modifies the source file.
    assert_eq!(fs::read(&source).unwrap(), original);

    // YAML variant imports into a native document too.
    let yml = parent.join("variant.yml");
    fs::write(
        &yml,
        "steps:\n  - id: only\n    name: Only\n    command: echo hi\n",
    )
    .unwrap();
    let yaml_doc = library.import_file(&yml, "variant").expect("import yaml");
    assert_eq!(yaml_doc.pipeline.steps.len(), 1);
    assert_eq!(yaml_doc.pipeline.steps[0].command, "echo hi");
    assert!(parent.join("pipelines/variant.pipeline").is_file());

    // Imports never silently overwrite an existing name.
    match library.import_file(&source, "first-run") {
        Err(StoreError::NameExists(name)) => assert_eq!(name, "first-run"),
        other => panic!("expected NameExists, got {other:?}"),
    }

    cleanup(&parent);
}

/// A source link whose consumer is listed BEFORE its producer receives its
/// implicit dependency during import, so the import-time chaining decision
/// never manufactures a `producer → consumer` reverse edge (a cycle).
#[test]
fn import_links_consumer_before_producer_without_cycle() {
    let (parent, library) = library("import-links");
    let yml = parent.join("linked.yml");
    fs::write(
        &yml,
        r#"steps:
  - id: consumer
    name: Consumer
    inputs:
      - key: result_file
        type: file
        source_step: producer
        source_output: result
  - id: producer
    name: Producer
    outputs:
      - key: result
        path: output/result.txt
"#,
    )
    .unwrap();

    let doc = library.import_file(&yml, "linked").expect("import linked");
    assert_eq!(doc.pipeline.steps.len(), 2);
    let consumer = &doc.pipeline.steps[0];
    let producer = &doc.pipeline.steps[1];
    assert_eq!(consumer.id, "consumer");
    assert_eq!(
        consumer.dependencies,
        ["producer"],
        "source link must contribute the implicit dependency first"
    );
    assert!(
        producer.dependencies.is_empty(),
        "chaining manufactured a reverse edge (cycle): {:?}",
        producer.dependencies
    );

    cleanup(&parent);
}

// ------------------------------------------------------------------ export

/// Explicit export writes the pipeline body in the by-alias wire format
/// (JSON); the YAML export parses back to the same pipeline.
#[test]
fn export_writes_by_alias_json() {
    let (parent, library) = library("export");
    let doc = PipelineDoc::new("exported", sample_pipeline());

    let dest = parent.join("out/pipeline.json");
    library
        .export(&doc, &dest, ExportFormat::Json)
        .expect("export json");
    let exported: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
    assert_eq!(exported["steps"][0]["id"], serde_json::json!("first"));
    assert_eq!(
        exported["steps"][0]["inputs"][0]["type"],
        serde_json::json!("text")
    );
    assert_eq!(exported["steps"][0]["inputs"][0]["min"], serde_json::Value::Null);
    assert_eq!(
        exported["steps"][0]["working_directory"],
        serde_json::json!(".")
    );
    assert_eq!(exported["steps"][0]["outputs"][0]["key"], serde_json::json!(""));

    let dest_yml = parent.join("out/pipeline.yml");
    library
        .export(&doc, &dest_yml, ExportFormat::Yaml)
        .expect("export yaml");
    let yaml_text = fs::read_to_string(&dest_yml).unwrap();
    let parsed = opm_pipeline::parse_pipeline("pipeline.yml", &yaml_text).expect("yaml parses");
    assert_eq!(parsed, doc.pipeline);

    cleanup(&parent);
}
