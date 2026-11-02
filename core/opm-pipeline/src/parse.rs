//! Pipeline file parsing — port of `backend/app/services/pipeline_parser.py`.

use serde::Deserialize;
use std::fmt;

use crate::model::PipelineConfig;

/// Error produced by [`parse_pipeline`]. Display texts match the Python
/// `ValueError`s (`str(exc)`) wherever Python raises them.
#[derive(Debug)]
pub enum ParseError {
    /// Suffix is not `.json`/`.yml`/`.yaml`. Payload is the suffix as it
    /// appeared (Python: `f"Unsupported pipeline format: {path.suffix}"`).
    UnsupportedFormat(String),
    /// Top level of the file is not an object/mapping
    /// ("Pipeline file must contain an object at the top level").
    NotAnObject,
    /// A `.json` file failed to parse. `file` is the `path_or_suffix`
    /// argument. This also covers strict-JSON rejection of bare
    /// `NaN`/`Infinity` tokens, which Python's `json.loads` accepts — a
    /// documented deviation: nonfinite values must come from YAML or be
    /// supplied in memory, never normalized on the way in.
    InvalidJson {
        file: String,
        message: String,
    },
    /// A `.yml`/`.yaml` file failed to parse (syntax or shape).
    Yaml(serde_yml::Error),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnsupportedFormat(suffix) => {
                write!(f, "Unsupported pipeline format: {}", suffix)
            }
            ParseError::NotAnObject => {
                f.write_str("Pipeline file must contain an object at the top level")
            }
            ParseError::InvalidJson { file, message } => write!(f, "{}: {}", file, message),
            ParseError::Yaml(err) => write!(f, "{}", err),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<serde_yml::Error> for ParseError {
    fn from(err: serde_yml::Error) -> Self {
        ParseError::Yaml(err)
    }
}

/// Parse pipeline text from `.json` (strict JSON via `serde_json`) or
/// `.yml`/`.yaml` (via `serde_yml`). An empty YAML file parses as an empty
/// pipeline (`{}`); a non-object top level yields [`ParseError::NotAnObject`];
/// unknown fields are ignored (Pydantic default). The first argument may be a
/// file path or a bare `.suffix`.
pub fn parse_pipeline(path_or_suffix: &str, text: &str) -> Result<PipelineConfig, ParseError> {
    let suffix = pipeline_suffix(path_or_suffix);
    match suffix.to_ascii_lowercase().as_str() {
        ".json" => parse_json(path_or_suffix, text),
        ".yml" | ".yaml" => parse_yaml(text),
        _ => Err(ParseError::UnsupportedFormat(suffix)),
    }
}

fn parse_json(file: &str, text: &str) -> Result<PipelineConfig, ParseError> {
    let invalid = |err: serde_json::Error| ParseError::InvalidJson {
        file: file.to_owned(),
        message: err.to_string(),
    };
    // First pass: syntax + top-level shape (including NaN/Infinity tokens).
    let probe: serde_json::Value = serde_json::from_str(text).map_err(invalid)?;
    if !probe.is_object() {
        return Err(ParseError::NotAnObject);
    }
    // Second pass: the model itself (unknown fields ignored, `default`
    // values land in `opm_pipeline::Value`).
    serde_json::from_str::<PipelineConfig>(text).map_err(invalid)
}

fn parse_yaml(text: &str) -> Result<PipelineConfig, ParseError> {
    let doc: serde_yml::Value = serde_yml::from_str(text)?;
    match doc {
        // Empty file / explicit null == `{}` (Python: `yaml.safe_load(raw) or {}`).
        serde_yml::Value::Null => Ok(PipelineConfig::default()),
        serde_yml::Value::Mapping(_) => PipelineConfig::deserialize(doc).map_err(ParseError::Yaml),
        _ => Err(ParseError::NotAnObject),
    }
}

/// Suffix of `path`, lowercase comparison handled by the caller; the returned
/// value keeps its original casing for the `UnsupportedFormat` message.
/// Accepts full paths (`pipeline.yml`, `a.b.yaml`) and bare suffixes
/// (`.yml`), mirroring `Path.suffix` for real file paths.
fn pipeline_suffix(path_or_suffix: &str) -> String {
    let name = path_or_suffix
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path_or_suffix);
    // A bare ".yml"-style argument is itself the suffix (only when it has no
    // further dots, so hidden files like ".foo.bar" still take the last dot).
    if name.len() > 1 && name.starts_with('.') && !name[1..].contains('.') {
        return name.to_owned();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name[i..].to_owned(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::pipeline_suffix;

    #[test]
    fn suffix_extraction() {
        assert_eq!(pipeline_suffix("pipeline.yml"), ".yml");
        assert_eq!(pipeline_suffix("PIPELINE.YML"), ".YML");
        assert_eq!(pipeline_suffix("/tmp/a.b/pipeline.yaml"), ".yaml");
        assert_eq!(pipeline_suffix("generic-analysis.pipeline.yml"), ".yml");
        assert_eq!(pipeline_suffix("pipeline.json"), ".json");
        assert_eq!(pipeline_suffix(".yml"), ".yml");
        assert_eq!(pipeline_suffix("notes.txt"), ".txt");
        assert_eq!(pipeline_suffix("README"), "");
        assert_eq!(pipeline_suffix(".gitignore"), ".gitignore");
    }
}
