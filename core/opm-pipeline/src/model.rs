//! Pipeline model — port of `backend/app/models/pipeline.py`.
//!
//! Wire schema matches Pydantic's `model_dump(mode="json", by_alias=True)`:
//! external field `type`, `min`/`max` aliases, null fields serialized, output
//! `key` always present (possibly `""`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::value::Value;

/// Field kinds accepted by `type` (Pydantic `FieldType` literal).
/// `selector` deserializes as `Select` (pipeline.py `normalize_type`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    Integer,
    Decimal,
    Boolean,
    #[serde(alias = "selector")]
    Select,
    File,
    Folder,
}

impl Default for FieldType {
    fn default() -> Self {
        FieldType::Text
    }
}

/// Validation issue severities (Pydantic `IssueSeverity` literal).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    Warning,
    Blocker,
}

/// An input or parameter declaration (Pydantic `ValueSpec`).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ValueSpec {
    pub key: String,
    pub label: Option<String>,
    /// Wire name: `type` (Pydantic field `type`), default `text`.
    #[serde(rename = "type", default)]
    pub kind: FieldType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub options: Vec<String>,
    /// Wire aliases: serialized as `min`, also accepted as `minimum`.
    #[serde(default, alias = "minimum")]
    pub min: Option<f64>,
    /// Wire aliases: serialized as `max`, also accepted as `maximum`.
    #[serde(default, alias = "maximum")]
    pub max: Option<f64>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source_step: Option<String>,
    #[serde(default)]
    pub source_output: Option<String>,
}

/// A step output declaration (Pydantic `OutputSpec`).
/// `key` may be empty (DECISIONS.md: unkeyed outputs are valid) and always
/// serializes, as `""` when unset.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct OutputSpec {
    #[serde(default)]
    pub key: String,
    pub label: Option<String>,
    pub path: String,
}

/// One pipeline step (Pydantic `PipelineStep`).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PipelineStep {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub environment: String,
    #[serde(default = "default_working_directory")]
    pub working_directory: String,
    #[serde(default)]
    pub inputs: Vec<ValueSpec>,
    #[serde(default)]
    pub outputs: Vec<OutputSpec>,
    #[serde(default)]
    pub parameters: Vec<ValueSpec>,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

fn default_working_directory() -> String {
    ".".to_owned()
}

impl PipelineStep {
    /// Non-null defaults of inputs followed by parameters (later declarations
    /// with the same key win, as in the Python dict loop).
    pub fn default_values(&self) -> BTreeMap<String, Value> {
        let mut values = BTreeMap::new();
        for spec in self.inputs.iter().chain(self.parameters.iter()) {
            if let Some(default) = &spec.default {
                if !matches!(default, Value::Null) {
                    values.insert(spec.key.clone(), default.clone());
                }
            }
        }
        values
    }

    /// Keyed outputs: non-empty `key` -> output path.
    pub fn output_values(&self) -> BTreeMap<String, Value> {
        self.outputs
            .iter()
            .filter(|output| !output.key.is_empty())
            .map(|output| (output.key.clone(), Value::Str(output.path.clone())))
            .collect()
    }
}

/// The whole pipeline file (Pydantic `PipelineConfig`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct PipelineConfig {
    #[serde(default)]
    pub steps: Vec<PipelineStep>,
}

impl PipelineConfig {
    /// Lookup by id with Python dict-comprehension semantics: when ids are
    /// duplicated (already a validation blocker), the LAST step wins.
    pub fn step_by_id(&self, id: &str) -> Option<&PipelineStep> {
        self.steps.iter().rev().find(|step| step.id == id)
    }
}

/// One validation finding (Pydantic `ValidationIssue`).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ValidationIssue {
    pub severity: IssueSeverity,
    pub message: String,
    pub step_id: Option<String>,
    pub field: Option<String>,
}
