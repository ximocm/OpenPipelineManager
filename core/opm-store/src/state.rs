//! Persisted state types — port of `backend/app/models/state.py` (§11 A5).
//!
//! Field order inside [`StepState`] mirrors the Pydantic model declaration so
//! the serialized JSON matches Python's `model_dump()` key order.

use std::collections::HashMap;

use opm_pipeline::{PipelineConfig, PipelineStep, ValidationIssue};
use serde::{Deserialize, Serialize};

use crate::ValueMap;

/// `Literal["pending", "scheduled", "running", "ok", "error", "cancelled"]`.
///
/// `Scheduled` is never set by the app itself; it exists for model fidelity
/// (§4 opm-exec).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    #[default]
    Pending,
    Scheduled,
    Running,
    Ok,
    Error,
    Cancelled,
}

/// `StepRuntimeState` — the persisted fields of `models/state.py:13-18`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct StepState {
    #[serde(default)]
    pub status: StepStatus,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub message: String,
}

/// Persisted canvas position: layout entries serialize as `{"x": .., "y": ..}`
/// objects (§11 A5, pinned by the Python round-trip).
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// Step detail payload: `{step, params, state, validation}` (§4 opm-store),
/// with `validation` filtered to the issues of this step.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct StepDetail {
    pub step: PipelineStep,
    pub params: ValueMap,
    pub state: StepState,
    pub validation: Vec<ValidationIssue>,
}

/// `ProjectSnapshot` — the `pipeline.project.json` document.
///
/// Serialization matches Python's `model_dump(mode="json", by_alias=True)`:
/// nested pipeline types carry their aliases (`type`, `min`, `max`) and `null`
/// fields are always written.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct ProjectSnapshot {
    pub project_path: String,
    #[serde(default)]
    pub pipeline: Option<PipelineConfig>,
    #[serde(default)]
    pub params: HashMap<String, ValueMap>,
    #[serde(default)]
    pub state: HashMap<String, StepState>,
    #[serde(default)]
    pub visual_layout: HashMap<String, Position>,
    #[serde(default)]
    pub validation: Vec<ValidationIssue>,
}
