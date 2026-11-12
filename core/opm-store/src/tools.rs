//! Independent registry tools, their local library, and self-contained step templates.
//! Installing a definition never executes it or installs external software.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use opm_fs::{resolve_managed_path, write_atomic};
use opm_pipeline::{FieldType, OutputSpec, PipelineStep, Value, ValueSpec};
use serde::{Deserialize, Serialize};

use crate::{SaveMode, StoreError};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSource {
    pub repository: String,
    pub revision: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolField {
    #[serde(rename = "type")]
    pub kind: FieldType,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolArgument {
    Token(String),
    Optional {
        when: String,
        args: Vec<ToolArgument>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub executable: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub version_arguments: Vec<String>,
    #[serde(default)]
    pub tested_software_version: Option<String>,
    pub inputs: BTreeMap<String, ToolField>,
    pub parameters: BTreeMap<String, ToolField>,
    pub outputs: BTreeMap<String, ToolField>,
    pub arguments: Vec<ToolArgument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ToolSource>,
}

/// Optional argument groups and the number of separate inputs for list fields
/// are chosen before painting. The resulting ordinary step needs no registry at run time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolOptions {
    pub enabled: BTreeSet<String>,
    pub counts: BTreeMap<String, usize>,
}

fn error(message: impl Into<String>) -> StoreError {
    StoreError::Message(message.into())
}

fn identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 100
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn key(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 100
        && (text.as_bytes()[0].is_ascii_alphabetic() || text.as_bytes()[0] == b'_')
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn token_key(text: &str) -> Option<&str> {
    text.strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
        .filter(|text| key(text))
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

impl ToolDefinition {
    pub fn from_json(text: &str) -> Result<Self, StoreError> {
        if text.len() > 1024 * 1024 {
            return Err(error("Tool definition exceeds 1 MiB"));
        }
        let tool: Self =
            serde_json::from_str(text).map_err(|e| error(format!("Invalid tool: {e}")))?;
        tool.validate()?;
        Ok(tool)
    }

    pub fn validate(&self) -> Result<(), StoreError> {
        if self.schema_version != 1 {
            return Err(error(format!(
                "Unsupported tool schema_version {}",
                self.schema_version
            )));
        }
        if !identifier(&self.id) || self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err(error("A tool requires a valid id, name and version"));
        }
        if self.executable.is_empty()
            || self.executable.starts_with('-')
            || !self
                .executable
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-+.".contains(&b))
        {
            return Err(error(
                "Tool executable must be a program name, without a path or shell syntax",
            ));
        }
        let mut fields = BTreeSet::new();
        for (name, field) in self
            .inputs
            .iter()
            .chain(&self.parameters)
            .chain(&self.outputs)
        {
            if !key(name) || !fields.insert(name.as_str()) {
                return Err(error(format!("Invalid or duplicate tool field: {name}")));
            }
            if !matches!(
                field.kind,
                FieldType::Text | FieldType::Integer | FieldType::File | FieldType::Folder
            ) || (field.multiple && !self.inputs.contains_key(name))
                || (field.multiple && !matches!(field.kind, FieldType::File | FieldType::Folder))
                || (self.outputs.contains_key(name)
                    && !matches!(field.kind, FieldType::File | FieldType::Folder))
            {
                return Err(error(format!(
                    "Unsupported field type or multiplicity: {name}"
                )));
            }
            if field.min.is_some_and(|n| !n.is_finite())
                || field.max.is_some_and(|n| !n.is_finite())
                || matches!((field.min, field.max), (Some(min), Some(max)) if min > max)
            {
                return Err(error(format!("Invalid range: {name}")));
            }
            if let Some(value) = field.default.as_ref().filter(|v| **v != Value::Null) {
                let valid = match (field.kind, value) {
                    (FieldType::Integer, Value::Int(n)) => {
                        !field.min.is_some_and(|min| (*n as f64) < min)
                            && !field.max.is_some_and(|max| (*n as f64) > max)
                    }
                    (_, Value::Str(text))
                        if field.kind != FieldType::Integer && !field.multiple =>
                    {
                        !text.contains('\0')
                    }
                    (_, Value::Seq(items)) if field.multiple => items
                        .iter()
                        .all(|v| matches!(v, Value::Str(s) if !s.is_empty() && !s.contains('\0'))),
                    _ => false,
                };
                if !valid {
                    return Err(error(format!("Invalid default: {name}")));
                }
            }
        }
        self.check_arguments(&self.arguments, &fields, 0)?;
        if let Some(stdout) = &self.stdout {
            if !self
                .outputs
                .get(stdout)
                .is_some_and(|field| field.kind == FieldType::File)
            {
                return Err(error("stdout must reference a declared file output"));
            }
        }
        Ok(())
    }

    fn check_arguments(
        &self,
        args: &[ToolArgument],
        fields: &BTreeSet<&str>,
        depth: usize,
    ) -> Result<(), StoreError> {
        if depth > 8 {
            return Err(error("Tool argument groups are nested too deeply"));
        }
        for arg in args {
            match arg {
                ToolArgument::Token(token) => {
                    if token.contains('\0') || token.contains('\n') || token.contains('\r') {
                        return Err(error("Invalid character in tool argument"));
                    }
                    if let Some(name) = token_key(token) {
                        if !fields.contains(name) {
                            return Err(error(format!("Unknown argument field: {name}")));
                        }
                    } else if token.contains('{') || token.contains('}') {
                        return Err(error("A tool placeholder must occupy a complete argument"));
                    }
                }
                ToolArgument::Optional { when, args } => {
                    if !self.parameters.contains_key(when) {
                        return Err(error(format!("Unknown optional parameter: {when}")));
                    }
                    self.check_arguments(args, fields, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    pub fn optional_keys(&self) -> BTreeSet<String> {
        fn visit(args: &[ToolArgument], result: &mut BTreeSet<String>) {
            for arg in args {
                if let ToolArgument::Optional { when, args } = arg {
                    result.insert(when.clone());
                    visit(args, result);
                }
            }
        }
        let mut result = BTreeSet::new();
        visit(&self.arguments, &mut result);
        result
    }

    pub fn default_options(&self) -> ToolOptions {
        ToolOptions {
            enabled: self
                .optional_keys()
                .into_iter()
                .filter(|name| {
                    self.parameters[name].default.as_ref().is_some_and(|value| {
                        *value != Value::Null && !matches!(value, Value::Str(s) if s.is_empty())
                    })
                })
                .collect(),
            counts: self
                .inputs
                .iter()
                .filter(|(_, f)| f.multiple)
                .map(|(name, f)| {
                    let count = match &f.default {
                        Some(Value::Seq(v)) => v.len().max(1),
                        _ => 1,
                    };
                    (name.clone(), count)
                })
                .collect(),
        }
    }

    pub fn instantiate(
        &self,
        step_id: &str,
        options: &ToolOptions,
    ) -> Result<PipelineStep, StoreError> {
        self.validate()?;
        if !identifier(step_id) {
            return Err(error("Invalid tool step id"));
        }
        let optional = self.optional_keys();
        if !options.enabled.is_subset(&optional) {
            return Err(error("Unknown optional argument selection"));
        }
        for (name, count) in &options.counts {
            if !self.inputs.get(name).is_some_and(|f| f.multiple) || !(1..=32).contains(count) {
                return Err(error("Choose between 1 and 32 inputs for a list field"));
            }
        }
        let mut used = BTreeSet::new();
        let mut argv = vec![quote(&self.executable)];
        fn expand(
            args: &[ToolArgument],
            options: &ToolOptions,
            used: &mut BTreeSet<String>,
            argv: &mut Vec<String>,
        ) {
            for arg in args {
                match arg {
                    ToolArgument::Optional { when, args } if options.enabled.contains(when) => {
                        expand(args, options, used, argv)
                    }
                    ToolArgument::Optional { .. } => {}
                    ToolArgument::Token(token) => {
                        if let Some(name) = token_key(token) {
                            used.insert(name.to_owned());
                            let count = options.counts.get(name).copied().unwrap_or(1);
                            for i in 0..count {
                                let key = if count == 1 {
                                    name.to_owned()
                                } else {
                                    format!("{name}_{}", i + 1)
                                };
                                argv.push(format!("{{{key}}}"));
                            }
                        } else {
                            argv.push(quote(token));
                        }
                    }
                }
            }
        }
        expand(&self.arguments, options, &mut used, &mut argv);
        if let Some(stdout) = &self.stdout {
            used.insert(stdout.clone());
        }
        let mut inputs = Vec::new();
        let mut parameters = Vec::new();
        let mut outputs = Vec::new();
        let mut directories = BTreeSet::new();
        let mut expanded_keys = BTreeSet::new();
        for (name, field) in self
            .inputs
            .iter()
            .chain(&self.parameters)
            .chain(&self.outputs)
        {
            if !used.contains(name) {
                continue;
            }
            let count = options.counts.get(name).copied().unwrap_or(1);
            for index in 0..count {
                let expanded = if count == 1 {
                    name.clone()
                } else {
                    format!("{name}_{}", index + 1)
                };
                if !expanded_keys.insert(expanded.clone()) {
                    return Err(error("Expanded input names conflict"));
                }
                let label = expanded.replace('_', " ");
                if self.outputs.contains_key(name) {
                    let extension = if field.kind == FieldType::Folder {
                        ""
                    } else if name.starts_with("trimmed") {
                        ".fastq"
                    } else {
                        ".txt"
                    };
                    let path = format!("steps/{step_id}/{name}{extension}");
                    directories.insert(if field.kind == FieldType::Folder {
                        format!("{{{expanded}}}")
                    } else {
                        quote(&format!("steps/{step_id}"))
                    });
                    outputs.push(OutputSpec {
                        key: expanded,
                        label: Some(label),
                        path,
                    });
                } else {
                    let default = if field.multiple {
                        match &field.default {
                            Some(Value::Seq(items)) => items.get(index).cloned(),
                            value => value.clone(),
                        }
                    } else {
                        field.default.clone()
                    };
                    let spec = ValueSpec {
                        key: expanded,
                        label: Some(label),
                        kind: field.kind,
                        required: true,
                        default,
                        options: Vec::new(),
                        min: field.min,
                        max: field.max,
                        description: field.description.clone(),
                        source_step: None,
                        source_output: None,
                    };
                    if self.inputs.contains_key(name) {
                        inputs.push(spec);
                    } else {
                        parameters.push(spec);
                    }
                }
            }
        }
        let mut command = argv.join(" ");
        if let Some(stdout) = &self.stdout {
            command.push_str(&format!(" > {{{stdout}}}"));
        }
        if !directories.is_empty() {
            command = format!(
                "mkdir -p {} && {command}",
                directories.into_iter().collect::<Vec<_>>().join(" ")
            );
        }
        let mut description = format!("{}\n\nTool: {}@{}", self.description, self.id, self.version);
        if let Some(source) = &self.source {
            description.push_str(&format!(
                "\nRegistry: {}@{} · {}",
                source.repository, source.revision, source.path
            ));
        }
        Ok(PipelineStep {
            id: step_id.to_owned(),
            name: self.name.clone(),
            description,
            command,
            environment: String::new(),
            working_directory: ".".to_owned(),
            inputs,
            outputs,
            parameters,
            dependencies: Vec::new(),
        })
    }
}

pub struct ToolLibrary {
    root: PathBuf,
}

impl ToolLibrary {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn path(&self, id: &str) -> Result<PathBuf, StoreError> {
        if !identifier(id) {
            return Err(error("Invalid tool id"));
        }
        resolve_managed_path(&self.root, &format!("{id}.json")).map_err(StoreError::Io)
    }
    pub fn load(&self, id: &str) -> Result<ToolDefinition, StoreError> {
        let doc = ToolDefinition::from_json(&fs::read_to_string(self.path(id)?)?)?;
        if doc.id != id {
            return Err(error("Tool id does not match its file name"));
        }
        Ok(doc)
    }
    pub fn list(&self) -> Result<Vec<ToolDefinition>, StoreError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut tools = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| error("Invalid tool filename"))?;
                tools.push(self.load(id)?);
            }
        }
        tools.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(tools)
    }
    pub fn save(&self, tool: &ToolDefinition, mode: SaveMode) -> Result<(), StoreError> {
        tool.validate()?;
        let path = self.path(&tool.id)?;
        if mode == SaveMode::CreateNew && path.exists() {
            return Err(error(format!("Tool {} is already installed", tool.id)));
        }
        fs::create_dir_all(&self.root)?;
        let text = serde_json::to_vec_pretty(tool).map_err(|e| error(e.to_string()))?;
        write_atomic(&path, &text)?;
        Ok(())
    }
}
