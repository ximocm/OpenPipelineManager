//! Versioned persistence envelopes (pipeline-storage, 2026-11-03).
//!
//! Two UTF-8 JSON documents, both pinned to [`FORMAT_VERSION`]:
//! - [`PipelineDoc`] — one `.pipeline` library document (also embedded in
//!   [`RunDoc`]); reusable definition + canvas layout, no work state.
//! - [`RunDoc`] — `.pipeline-manager/run.json` per working directory: the
//!   work's definition snapshot, per-step values and execution state.
//!
//! YAML/JSON pipeline bodies remain import/export formats only (see
//! [`crate::library`]); nothing else is persisted: no validation document,
//! no project marker, no absolute project paths.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use opm_pipeline::PipelineConfig;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::state::{Position, StepState};
use crate::store::StoreError;
use crate::ValueMap;

/// Native envelope schema version (`pipeline-storage` decision 2026-11-03).
pub const FORMAT_VERSION: u32 = 1;

/// `LAYOUT_START_X` (Python `storage.py:_default_step_position`).
const LAYOUT_START_X: f64 = 80.0;
/// `LAYOUT_START_Y`.
const LAYOUT_START_Y: f64 = 120.0;
/// `LAYOUT_GAP_Y`, the vertical spacing between steps (flows read top to
/// bottom; the old horizontal `LAYOUT_GAP_X` is gone).
const LAYOUT_GAP_Y: f64 = 180.0;

/// Default top-to-bottom canvas position of the step at `index`.
pub(crate) fn default_step_position(index: usize) -> Position {
    Position {
        x: LAYOUT_START_X,
        y: LAYOUT_START_Y + index as f64 * LAYOUT_GAP_Y,
    }
}

/// Reject an envelope schema other than [`FORMAT_VERSION`] with the visible
/// "unsupported …" message required by the storage proposal.
pub(crate) fn ensure_supported(version: u32, label: impl std::fmt::Display) -> Result<(), StoreError> {
    if version != FORMAT_VERSION {
        return Err(StoreError::Message(format!(
            "unsupported format_version {version} in {label}: expected {FORMAT_VERSION}"
        )));
    }
    Ok(())
}

/// Parse a version-checked JSON envelope; a corrupt document surfaces as a
/// visible `"<path>: <serde error>"` message and is never treated as absent.
pub(crate) fn parse_envelope<T: DeserializeOwned>(text: &str, path: &Path) -> Result<T, StoreError> {
    serde_json::from_str(text)
        .map_err(|err| StoreError::Message(format!("{}: {err}", path.display())))
}

/// Where a [`RunDoc`]'s definition came from — informational only: reopening
/// a work folder must work even if this file moved or was deleted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct PipelineSource {
    pub name: String,
    #[serde(default)]
    pub library_file: Option<String>,
}

/// `.pipeline` library document (also embedded in `run.json`).
///
/// Field order pins the serialized key order; `layout` maps step id → canvas
/// position. Work-specific values/state/logs never live here, and saving a
/// work folder never rewrites the library file this came from.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PipelineDoc {
    pub format_version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The pipeline model — the existing alias-compatible wire format
    /// (`type`, `min`/`max`, `"key": ""`).
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub layout: BTreeMap<String, Position>,
    /// UTC ISO-8601.
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

impl Default for PipelineDoc {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            name: String::new(),
            description: String::new(),
            pipeline: PipelineConfig::default(),
            layout: BTreeMap::new(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}

impl PipelineDoc {
    /// A current-version document stamped with the given name/pipeline.
    pub fn new(name: impl Into<String>, pipeline: PipelineConfig) -> Self {
        let now = utc_iso_now();
        Self {
            format_version: FORMAT_VERSION,
            name: name.into(),
            description: String::new(),
            pipeline,
            layout: BTreeMap::new(),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Layout normalization for a document entering persistence: the layout
    /// holds only declared steps and gives each one a default position when
    /// the document lacks it. The stored dependency graph is NEVER touched
    /// here — native documents used by `create_work`/`adopt_definition` keep
    /// their graph exactly as written (deliberately removed dependencies
    /// stay removed; independent steps stay independent).
    pub fn normalize(&mut self) {
        let ids: HashSet<&str> = self
            .pipeline
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .collect();
        self.layout.retain(|id, _| ids.contains(id.as_str()));
        for (index, step) in self.pipeline.steps.iter().enumerate() {
            self.layout
                .entry(step.id.clone())
                .or_insert_with(|| default_step_position(index));
        }
    }

    /// Import-only normalization for external YAML/JSON files parsed into a
    /// document (`PipelineLibrary::import_file`, and the parse-then-adopt
    /// helpers in the test suites):
    /// FIRST append the implicit dependencies contributed by source-linked
    /// inputs (by step id), THEN chain steps in file order — but only if the
    /// graph is STILL fully unconnected afterwards (the old
    /// `_connect_linear_pipeline_if_unconnected` semantics, which made the
    /// chaining decision after source resolution). A source link whose
    /// consumer is listed before its producer therefore never gets a
    /// manufactured reverse edge, and the layout follows.
    pub fn normalize_for_import(&mut self) {
        append_implicit_dependencies(&mut self.pipeline);
        connect_linear_pipeline(&mut self.pipeline);
        self.normalize();
    }

    /// Pretty JSON for a `.pipeline` document; refuses to serialize a
    /// document with an unsupported schema version.
    pub(crate) fn to_json(&self) -> Result<String, StoreError> {
        ensure_supported(self.format_version, &self.name)?;
        serde_json::to_string_pretty(self).map_err(|err| StoreError::Message(err.to_string()))
    }

    /// Read + version-check a `.pipeline` document.
    pub(crate) fn from_json(text: &str, path: &Path) -> Result<Self, StoreError> {
        let doc: Self = parse_envelope(text, path)?;
        ensure_supported(doc.format_version, path.display())?;
        Ok(doc)
    }
}

/// `.pipeline-manager/run.json` — the single support document of a working
/// directory. The embedded [`PipelineDoc`] snapshot is the work's source of
/// truth for execution; the work root is the directory containing this file
/// at open time (never stored as an absolute path).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct RunDoc {
    pub format_version: u32,
    pub source: PipelineSource,
    pub definition: PipelineDoc,
    /// Per-step parameter/input values (step id → key → value).
    #[serde(default)]
    pub values: BTreeMap<String, ValueMap>,
    /// Per-step execution state.
    #[serde(default)]
    pub state: BTreeMap<String, StepState>,
    /// UTC ISO-8601.
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

impl RunDoc {
    /// Read + version-check a `run.json` document. Both the outer envelope
    /// and the embedded definition carry a schema version: a nested
    /// unsupported `definition.format_version` is rejected with the same
    /// visible "unsupported …" message before any work state is installed.
    pub(crate) fn from_json(text: &str, path: &Path) -> Result<Self, StoreError> {
        let doc: Self = parse_envelope(text, path)?;
        ensure_supported(doc.format_version, path.display())?;
        ensure_supported(doc.definition.format_version, path.display())?;
        Ok(doc)
    }
}

/// Root-free twin of the dependency half of the store's
/// `resolve_input_sources`: append the source step id to every consumer that
/// declares a resolvable `source_step`/`source_output` link to a declared
/// step (same link conditions as resolution, so the import-time chaining
/// decision below sees exactly the graph resolution would produce later).
fn append_implicit_dependencies(pipeline: &mut PipelineConfig) {
    let mut sources: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for step in &pipeline.steps {
        let outputs = step
            .outputs
            .iter()
            .map(|output| (output.key.clone(), output.path.clone()))
            .collect();
        sources.insert(step.id.clone(), outputs);
    }
    for step in &mut pipeline.steps {
        for input in &step.inputs {
            let (Some(source_step), Some(source_output)) =
                (input.source_step.as_deref(), input.source_output.as_deref())
            else {
                continue;
            };
            if source_step.is_empty() || source_output.is_empty() {
                continue;
            }
            let Some(source_outputs) = sources.get(source_step) else {
                continue;
            };
            let linked = source_outputs
                .iter()
                .any(|(key, path)| key == source_output || path == source_output);
            if linked && !step.dependencies.iter().any(|dep| dep == source_step) {
                step.dependencies.push(source_step.to_string());
            }
        }
    }
}

/// `storage.py:_connect_linear_pipeline_if_unconnected` — chain steps only
/// when the pipeline declares no dependency anywhere (explicit links win).
fn connect_linear_pipeline(pipeline: &mut PipelineConfig) {
    if pipeline.steps.iter().any(|step| !step.dependencies.is_empty()) {
        return;
    }
    for index in 1..pipeline.steps.len() {
        let previous_id = pipeline.steps[index - 1].id.clone();
        pipeline.steps[index].dependencies = vec![previous_id];
    }
}

/// UTC ISO-8601 timestamp in `datetime.now(timezone.utc).isoformat()` shape
/// (microseconds only when nonzero) — same algorithm as the `utc_iso_now`
/// helpers in `opm-fs`/`opm-exec` (no extra time dependency here).
pub(crate) fn utc_iso_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let micros = now.subsec_micros();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
    if micros == 0 {
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}+00:00")
    } else {
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{micros:06}+00:00")
    }
}

/// Days since the epoch → civil date (Howard Hinnant's algorithm, as in
/// `opm-fs`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}
