mod common;
use opm_pipeline::Value;
use opm_store::{SaveMode, ToolDefinition, ToolLibrary, ToolSource};
use std::fs;

fn fastqc() -> ToolDefinition {
    ToolDefinition::from_json(include_str!("fixtures/fastqc.json")).unwrap()
}
fn cutadapt() -> ToolDefinition {
    ToolDefinition::from_json(include_str!("fixtures/cutadapt-single.json")).unwrap()
}

#[test]
fn installed_tool_roundtrips_provenance_and_refuses_silent_replacement() {
    let root = common::temp_dir("tool-library");
    let library = ToolLibrary::new(root.join("tools"));
    let mut tool = fastqc();
    tool.source = Some(ToolSource {
        repository: "owner/registry".into(),
        revision: "a".repeat(40),
        path: "tools/fastqc.json".into(),
    });
    library.save(&tool, SaveMode::CreateNew).unwrap();
    assert_eq!(library.load("fastqc").unwrap(), tool);
    assert_eq!(library.list().unwrap(), vec![tool.clone()]);
    tool.description = "Changed".into();
    assert!(library.save(&tool, SaveMode::CreateNew).is_err());
    assert_ne!(
        library.load("fastqc").unwrap().description,
        tool.description
    );
    library.save(&tool, SaveMode::Overwrite).unwrap();
    assert_eq!(library.load("fastqc").unwrap(), tool);
    assert_eq!(fs::read_dir(library.root()).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn list_inputs_become_separate_arguments_and_each_copy_gets_its_own_outputs() {
    let tool = fastqc();
    let mut options = tool.default_options();
    options.counts.insert("reads".into(), 2);
    let first = tool.instantiate("fastqc_1", &options).unwrap();
    let second = tool.instantiate("fastqc_2", &options).unwrap();
    assert_eq!(
        first
            .inputs
            .iter()
            .map(|s| s.key.as_str())
            .collect::<Vec<_>>(),
        ["reads_1", "reads_2"]
    );
    assert!(first.command.contains("{reads_1} {reads_2}"));
    assert!(first.command.contains("mkdir -p {outdir}"));
    assert_ne!(first.outputs, second.outputs);
    assert_eq!(first.parameters[0].default, Some(Value::Int(1)));
    options.counts.insert("reads".into(), 0);
    assert!(tool.instantiate("fastqc_1", &options).is_err());
}

#[test]
fn optional_arguments_are_explicit_and_unselected_fields_do_not_become_required() {
    let tool = cutadapt();
    let mut options = tool.default_options();
    let plain = tool.instantiate("trim", &options).unwrap();
    assert!(!plain.command.contains("{adapter3}"));
    assert!(!plain.parameters.iter().any(|p| p.key == "adapter3"));
    options.enabled.insert("adapter3".into());
    options.enabled.insert("max_length".into());
    let configured = tool.instantiate("trim", &options).unwrap();
    assert!(configured.command.contains("'-a' {adapter3}"));
    assert!(configured.command.contains("'-M' {max_length}"));
    assert!(configured
        .parameters
        .iter()
        .any(|p| p.key == "adapter3" && p.required));
    assert!(configured.command.ends_with("> {report}"));
    options.enabled.insert("unknown".into());
    assert!(tool.instantiate("trim", &options).is_err());
}

#[test]
fn invalid_imports_cannot_be_saved_as_executable_templates() {
    let fixture =
        serde_json::from_str::<serde_json::Value>(include_str!("fixtures/fastqc.json")).unwrap();
    for (field, value) in [
        ("id", serde_json::json!("../outside")),
        ("executable", serde_json::json!("fastqc; touch injected")),
        ("schema_version", serde_json::json!(2)),
        ("arguments", serde_json::json!(["{unknown}"])),
        (
            "arguments",
            serde_json::json!([{"when":"unknown","args":[]}]),
        ),
    ] {
        let mut doc = fixture.clone();
        doc[field] = value;
        assert!(
            ToolDefinition::from_json(&doc.to_string()).is_err(),
            "{field}"
        );
    }
    let mut tool = fastqc();
    tool.outputs
        .insert("reads".into(), tool.outputs["outdir"].clone());
    assert!(tool.validate().is_err());
}

#[cfg(unix)]
#[test]
fn tool_library_rejects_symlinked_managed_files() {
    let root = common::temp_dir("tool-symlink");
    fs::create_dir(root.join("tools")).unwrap();
    let outside = root.join("outside.json");
    fs::write(&outside, "sentinel").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("tools/fastqc.json")).unwrap();
    let library = ToolLibrary::new(root.join("tools"));
    assert!(library.load("fastqc").is_err());
    assert!(library.save(&fastqc(), SaveMode::Overwrite).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "sentinel");
    fs::remove_dir_all(root).unwrap();
}
