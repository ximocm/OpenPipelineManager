//! opm-store — versioned persistence for OPM (pipeline-storage decision,
//! 2026-11-03 — clean break, no legacy readers or migration).
//!
//! Two UTF-8 JSON documents, both pinned to [`FORMAT_VERSION`]:
//! - `.pipeline` library documents ([`PipelineDoc`]) via [`PipelineLibrary`]:
//!   one canonical, editable document per reusable pipeline;
//! - `.pipeline-manager/run.json` ([`RunDoc`]) via [`ProjectStore`]: one
//!   support document per working directory holding the definition
//!   snapshot, per-step values and execution state.
//!
//! All writes go through `opm_fs::write_atomic` (staged rename: a failed or
//! interrupted save leaves the previous complete document readable);
//! unsupported schema versions surface as a visible [`StoreError::Message`].
//! Validation is recalculated on demand and never persisted.

mod envelope;
mod library;
mod state;
mod store;
mod tools;

pub use envelope::{FORMAT_VERSION, PipelineDoc, PipelineSource, RunDoc};
pub use library::{ExportFormat, PipelineLibrary, SaveMode};
pub use state::{Position, ProjectSnapshot, StepDetail, StepState, StepStatus};
pub use store::{AdoptReport, ProjectStore, StoreError};
pub use tools::{ToolArgument, ToolDefinition, ToolField, ToolLibrary, ToolOptions, ToolSource};

use opm_pipeline::Value;
use std::collections::BTreeMap;

/// Per-step map of values (runtime params / effective defaults), §11 A4:
/// `BTreeMap<String, Value>` — never `serde_json::Value`. Persisted as the
/// entries of `RunDoc::values`.
pub type ValueMap = BTreeMap<String, Value>;
