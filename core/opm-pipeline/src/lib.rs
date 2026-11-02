//! opm-pipeline — OPM pipeline model, YAML/JSON parsing and validation.
//!
//! Rust port of the Python backend's pipeline contract:
//! `backend/app/models/pipeline.py`, `backend/app/services/pipeline_parser.py`
//! and `backend/app/services/validation.py` (all 29 validation rules), with
//! path safety delegated to the shared `opm-fs` helpers.

mod model;
mod parse;
mod pyfmt;
mod shell_context;
mod validate;
mod value;

pub use model::{
    FieldType, IssueSeverity, OutputSpec, PipelineConfig, PipelineStep, ValidationIssue,
    ValueSpec,
};
pub use parse::{parse_pipeline, ParseError};
pub use shell_context::{
    unsupported_context_placeholder, ScanStep, ShellQuote, ShellScanner,
};
pub use validate::{effective_values, placeholders, validate_pipeline};
pub use value::Value;
