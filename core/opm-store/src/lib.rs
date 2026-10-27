//! opm-store — persisted project state for OPM.
//!
//! Rust port of `backend/app/services/storage.py` + `backend/app/models/state.py`:
//! [`ProjectStore`] loads the `.pipeline-manager/` runtime files and the
//! `pipeline.project.json` snapshot with the Python precedence rules (non-empty
//! snapshot collections override the runtime files, `storage.py:408-421`) and
//! saves everything back with Python-compatible JSON (by-alias field names,
//! §11 A5).

mod state;
mod store;

pub use state::{Position, ProjectSnapshot, StepDetail, StepState, StepStatus};
pub use store::{ProjectStore, StoreError};

use opm_pipeline::Value;
use std::collections::BTreeMap;

/// Per-step map of values (runtime params / effective defaults), §11 A4:
/// `BTreeMap<String, Value>` — never `serde_json::Value`.
pub type ValueMap = BTreeMap<String, Value>;
