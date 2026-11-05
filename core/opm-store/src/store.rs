//! [`ProjectStore`] — the persisted state of ONE working directory.
//!
//! The work folder is identified by `.pipeline-manager/run.json` alone
//! (`run.json` embeds the definition snapshot, per-step values and state);
//! there is no project marker and no legacy reader — the clean break of the
//! pipeline-storage decision (2026-11-03). Every write goes through
//! `opm_fs::write_atomic`, so a failed save leaves the previous complete
//! document readable.
//!
//! Every public method takes the interior `Mutex` for its own duration only
//! (§11 A6): no store lock is ever held across a child process run/wait — the
//! engine calls short methods like [`ProjectStore::set_step_state`] or
//! [`ProjectStore::append_log`] between process steps.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use opm_fs::{
    ensure_step_folders, relative_project_path, resolve_managed_path, resolve_pipeline_path,
    safe_segment, write_atomic, PathError,
};
use opm_pipeline::{validate_pipeline, PipelineConfig, PipelineStep, ValidationIssue, Value};

use crate::envelope::{
    FORMAT_VERSION, PipelineDoc, PipelineSource, RunDoc, default_step_position, ensure_supported,
    utc_iso_now,
};
use crate::state::{Position, ProjectSnapshot, StepDetail, StepState, StepStatus};
use crate::ValueMap;

/// Manager folder below the work root.
const MANAGER_DIR: &str = ".pipeline-manager";
/// The single support document of a work folder.
const RUN_JSON_REL: &str = ".pipeline-manager/run.json";

/// Store failures, with the Python message texts where tests pin them.
#[derive(Debug)]
pub enum StoreError {
    /// `RuntimeError("No project is open")`.
    NoProject,
    /// `ValueError("No pipeline has been imported")`.
    NoPipeline,
    /// `KeyError(f"Unknown step: {id}")`.
    UnknownStep(String),
    /// `ValueError(f"Step already exists: {id}")`.
    StepExists(String),
    /// `ValueError("Step id is required")`.
    StepIdRequired,
    /// `ValueError("A step cannot depend on itself")`.
    SelfDependency,
    /// `ValueError(f"Unknown dependency: {dependency}")`.
    UnknownDependency(String),
    /// A library name already exists and the save requested `CreateNew`.
    NameExists(String),
    /// Filesystem failure; `opm-fs` path messages pass through verbatim.
    Io(std::io::Error),
    /// Parse/other failure with a ready-to-display message.
    Message(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NoProject => write!(f, "No project is open"),
            StoreError::NoPipeline => write!(f, "No pipeline has been imported"),
            StoreError::UnknownStep(id) => write!(f, "Unknown step: {id}"),
            StoreError::StepExists(id) => write!(f, "Step already exists: {id}"),
            StoreError::StepIdRequired => write!(f, "Step id is required"),
            StoreError::SelfDependency => write!(f, "A step cannot depend on itself"),
            StoreError::UnknownDependency(dep) => write!(f, "Unknown dependency: {dep}"),
            StoreError::NameExists(name) => {
                write!(f, "A pipeline named {name} already exists")
            }
            StoreError::Io(err) => write!(f, "{err}"),
            StoreError::Message(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(err: std::io::Error) -> Self {
        StoreError::Io(err)
    }
}

fn msg(error: impl std::fmt::Display) -> StoreError {
    StoreError::Message(error.to_string())
}

/// Outcome of [`ProjectStore::adopt_definition`]: which steps were
/// invalidated, pruned, added or kept unchanged by the adoption.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdoptReport {
    /// Fingerprint changed: `.done` deleted, status reset to `Pending`.
    pub changed: Vec<String>,
    /// Gone from the definition: values/state pruned, log and `.done` deleted.
    pub removed: Vec<String>,
    /// New steps: default state and declared default values.
    pub added: Vec<String>,
    /// Same fingerprint: state and declared values untouched.
    pub unchanged: Vec<String>,
}

/// In-memory work state; all fields are replaced wholesale on open/adopt
/// (staged load, so a failed open or adopt leaves the previous state
/// untouched — a corrupt `run.json` is a visible error, never a reset).
#[derive(Default)]
struct StoreData {
    root: Option<PathBuf>,
    source: PipelineSource,
    definition: PipelineDoc,
    values: HashMap<String, ValueMap>,
    state: BTreeMap<String, StepState>,
    created_at: String,
    validation: Vec<ValidationIssue>,
}

/// F3: segmento de nombre de artefacto — `/`, `\` y `:` se mapean a `_`.
/// El mapeo `/`→`_` es exacto del Python (paridad POSIX); `\` y `:`
/// cierran la traversión de Windows (`C:\victim\x`, `..\..\x`). Los ids
/// creados por la UI ya pasan por `safe_segment`; los importados llegan
/// crudos y esta capa los reduce a un segmento plano inofensivo.
fn artifact_segment(step_id: &str) -> String {
    step_id.replace(['/', '\\', ':'], "_")
}

/// Ruta relativa (con `/`) del log de un paso dentro del trabajo.
fn log_rel(step_id: &str) -> String {
    format!("{MANAGER_DIR}/logs/{}.log", artifact_segment(step_id))
}

/// Ruta relativa (con `/`) del `.done` de un paso dentro del trabajo.
fn done_rel(step_id: &str) -> String {
    format!("{MANAGER_DIR}/done/{}.done", artifact_segment(step_id))
}

fn log_path_in(root: &Path, step_id: &str) -> PathBuf {
    root.join(log_rel(step_id))
}

fn done_path_in(root: &Path, step_id: &str) -> PathBuf {
    root.join(done_rel(step_id))
}

/// F2: los directorios de runtime se crean solo tras comprobar contención y
/// ausencia de symlinks en el prefijo gestionado (`.pipeline-manager/**`).
fn ensure_manager_dirs(root: &Path) -> Result<(), StoreError> {
    let logs =
        resolve_managed_path(root, &format!("{MANAGER_DIR}/logs")).map_err(StoreError::Io)?;
    let done =
        resolve_managed_path(root, &format!("{MANAGER_DIR}/done")).map_err(StoreError::Io)?;
    fs::create_dir_all(logs).map_err(StoreError::Io)?;
    fs::create_dir_all(done).map_err(StoreError::Io)?;
    Ok(())
}

/// F2: ruta gestionada de `run.json` — contención dentro del trabajo y
/// prefijo sin symlinks antes de leer, escribir o comprobar nada.
fn run_json_path(root: &Path) -> Result<PathBuf, StoreError> {
    resolve_managed_path(root, RUN_JSON_REL).map_err(StoreError::Io)
}

/// `storage.py:_runtime_params`: declared defaults, then the existing values
/// whose keys are still declared as inputs/parameters of the step (stale or
/// unknown keys are dropped). The adopt reconciliation uses the same rule.
fn runtime_params(step: &PipelineStep, existing: Option<&ValueMap>) -> ValueMap {
    let mut values = step.default_values();
    if let Some(existing) = existing {
        let allowed: HashSet<&str> = step
            .inputs
            .iter()
            .chain(step.parameters.iter())
            .map(|spec| spec.key.as_str())
            .collect();
        for (key, value) in existing {
            if allowed.contains(key.as_str()) {
                values.insert(key.clone(), value.clone());
            }
        }
    }
    values
}

/// Python's 4-argument `relative_project_path(project, target_wd, source_wd,
/// output_path)` composed over the 3-argument `opm-fs` helper (§11 A7).
fn source_relative_path(
    root: &Path,
    target_working_directory: &str,
    source_working_directory: &str,
    source_output_path: &str,
) -> Result<String, PathError> {
    let target = resolve_pipeline_path(root, target_working_directory, root)?;
    let source_base = resolve_pipeline_path(root, source_working_directory, root)?;
    let source = resolve_pipeline_path(root, source_output_path, &source_base)?;
    relative_project_path(root, &target, &source)
}

/// `storage.py:is_step_ok` — `.done` marker AND every resolved output path
/// exists; any unsafe path (traversal, absolute, NUL, symlink loop) → false.
fn is_step_ok_at(root: &Path, step: &PipelineStep) -> bool {
    // F2: el `.done` debe ser una ruta gestionada contenida y sin symlinks —
    // un enlace colgante/envenenado cuenta como «sin marcar», nunca se sigue.
    match resolve_managed_path(root, &done_rel(&step.id)) {
        Ok(path) if path.exists() => {}
        _ => return false,
    }
    let working_directory = match resolve_pipeline_path(root, &step.working_directory, root) {
        Ok(path) => path,
        Err(_) => return false,
    };
    for output in &step.outputs {
        match resolve_pipeline_path(root, &output.path, &working_directory) {
            Ok(path) if path.exists() => {}
            _ => return false,
        }
    }
    true
}

/// `storage.py:_resolve_input_sources`: rewrites source-linked input defaults
/// to work-relative paths (consuming-step wd → source output), mirrors the
/// value into `values[step][key]` and appends the implicit dependency.
/// In-memory only: validation and saves see the resolved form; the result is
/// root-relative, so it survives moving the work folder.
fn resolve_input_sources(data: &mut StoreData) {
    let (Some(root), pipeline, values) = (
        data.root.as_ref(),
        &mut data.definition.pipeline,
        &mut data.values,
    ) else {
        return;
    };

    // Snapshot the source facts first (Python held a dict of live references).
    let mut sources: HashMap<String, (String, Vec<(String, String)>)> = HashMap::new();
    for step in &pipeline.steps {
        let outputs = step
            .outputs
            .iter()
            .map(|output| (output.key.clone(), output.path.clone()))
            .collect();
        sources.insert(step.id.clone(), (step.working_directory.clone(), outputs));
    }

    for step in &mut pipeline.steps {
        let step_id = step.id.clone();
        let step_working_directory = step.working_directory.clone();
        let mut dependencies = step.dependencies.clone();
        for input_index in 0..step.inputs.len() {
            let (source_step_id, source_output_key) = {
                let input = &step.inputs[input_index];
                match (&input.source_step, &input.source_output) {
                    (Some(source_step), Some(source_output))
                        if !source_step.is_empty() && !source_output.is_empty() =>
                    {
                        (source_step.clone(), source_output.clone())
                    }
                    _ => continue,
                }
            };
            let Some((source_working_directory, source_outputs)) = sources.get(&source_step_id)
            else {
                continue;
            };
            let Some((_, source_output_path)) = source_outputs.iter().find(|(key, path)| {
                key == &source_output_key || path == &source_output_key
            }) else {
                continue;
            };
            if let Ok(value) = source_relative_path(
                root,
                &step_working_directory,
                source_working_directory,
                source_output_path,
            ) {
                step.inputs[input_index].default = Some(Value::Str(value.clone()));
                values
                    .entry(step_id.clone())
                    .or_default()
                    .insert(step.inputs[input_index].key.clone(), Value::Str(value));
            }
            if !dependencies.contains(&source_step_id) {
                dependencies.push(source_step_id);
            }
        }
        step.dependencies = dependencies;
    }
}

/// `storage.py:_initialize_pipeline_state`: for every declared step, values
/// are reconciled to the declared keys (`runtime_params`), default state is
/// seeded, and a stale `ok` (marker/outputs gone) downgrades to `pending`
/// (clearing exit_code/message, keeping `last_run_at`).
fn initialize_pipeline_state(data: &mut StoreData) {
    let (Some(root), pipeline, values, state) = (
        data.root.as_ref(),
        &data.definition.pipeline,
        &mut data.values,
        &mut data.state,
    ) else {
        return;
    };
    for step in &pipeline.steps {
        let existing = values.get(&step.id);
        let runtime = runtime_params(step, existing);
        values.insert(step.id.clone(), runtime);
        let entry = state.entry(step.id.clone()).or_default();
        if is_step_ok_at(root, step) {
            entry.status = StepStatus::Ok;
        } else if entry.status == StepStatus::Ok {
            entry.status = StepStatus::Pending;
            entry.exit_code = None;
            entry.message.clear();
        }
    }
}

fn get_step(data: &StoreData, step_id: &str) -> Result<PipelineStep, StoreError> {
    data.definition
        .pipeline
        .steps
        .iter()
        .find(|step| step.id == step_id)
        .cloned()
        .ok_or_else(|| StoreError::UnknownStep(step_id.to_string()))
}

fn step_ids(pipeline: &PipelineConfig) -> HashSet<String> {
    pipeline.steps.iter().map(|step| step.id.clone()).collect()
}

/// Adopt fingerprint (pipeline-storage decision): structural equality over
/// command, environment, working_directory, inputs, outputs, parameters and
/// dependencies. `name`/`description` are cosmetic and never invalidate work.
fn fingerprint_changed(previous: &PipelineStep, incoming: &PipelineStep) -> bool {
    previous.command != incoming.command
        || previous.environment != incoming.environment
        || previous.working_directory != incoming.working_directory
        || previous.inputs != incoming.inputs
        || previous.outputs != incoming.outputs
        || previous.parameters != incoming.parameters
        || previous.dependencies != incoming.dependencies
}

/// `storage.py:validate` — re-resolve input sources and recalculate the
/// issues from the snapshot + values + current files. Validation is NEVER
/// persisted (no `validation.json`): every reader recalculates on demand.
fn recompute_validation(data: &mut StoreData) {
    let Some(root) = data.root.clone() else {
        data.validation.clear();
        return;
    };
    resolve_input_sources(data);
    data.validation = validate_pipeline(&data.definition.pipeline, &root, &data.values);
}

fn snapshot_of(data: &StoreData) -> ProjectSnapshot {
    ProjectSnapshot {
        project_path: data
            .root
            .as_ref()
            .map(|root| root.to_string_lossy().into_owned())
            .unwrap_or_default(),
        pipeline: Some(data.definition.pipeline.clone()),
        params: data.values.clone().into_iter().collect(),
        state: data.state.clone().into_iter().collect(),
        visual_layout: data.definition.layout.clone().into_iter().collect(),
        validation: data.validation.clone(),
    }
}

/// Phase 1 of a save: managed directories, resolved target path and the
/// fully stamped/serialized envelope text. Every fallible, non-destructive
/// step of a save happens here, before any caller touches an artifact.
fn prepare_run(data: &mut StoreData) -> Result<(PathBuf, String), StoreError> {
    let root = data.root.as_ref().ok_or(StoreError::NoProject)?.clone();
    ensure_manager_dirs(&root)?;
    let path = run_json_path(&root)?;
    let now = utc_iso_now();
    data.definition.updated_at = now.clone();
    let run = RunDoc {
        format_version: FORMAT_VERSION,
        source: data.source.clone(),
        definition: data.definition.clone(),
        values: data.values.clone().into_iter().collect(),
        state: data.state.clone(),
        created_at: data.created_at.clone(),
        updated_at: now,
    };
    let text =
        serde_json::to_string_pretty(&run).map_err(|err| StoreError::Message(err.to_string()))?;
    Ok((path, text))
}

/// Atomically write `.pipeline-manager/run.json` (stamps `updated_at`);
/// refuses to persist an unsupported schema version.
fn write_run(data: &mut StoreData) -> Result<(), StoreError> {
    let (path, text) = prepare_run(data)?;
    write_atomic(&path, text.as_bytes()).map_err(StoreError::Io)
}

/// Attempts to pick a free staged sibling name (`stage_removal`).
const STAGE_ATTEMPTS: usize = 100;

/// A removed artifact parked as a rename inside its own managed directory
/// until an adoption commits: restored on any pre-commit failure, unlinked
/// after the commit. Both names go through `resolve_managed_path`, so the
/// containment rules hold for the staged name too (F2).
struct StagedRemoval {
    original: PathBuf,
    staged: PathBuf,
}

impl StagedRemoval {
    /// Put the artifact back (best effort: reverse rename inside the same
    /// directory, right after the forward rename succeeded).
    fn restore(self) {
        let _ = fs::rename(&self.staged, &self.original);
    }

    /// The document is committed: drop the parked copy.
    fn commit(self) {
        let _ = fs::remove_file(&self.staged);
    }
}

/// Rename the managed artifact at `rel` to a unique sibling in the same
/// directory. `Ok(None)` when the artifact simply does not exist; every
/// other failure (poisoned path, rename error) must abort the operation —
/// a staging failure is never silently skipped.
fn stage_removal(root: &Path, rel: &str) -> Result<Option<StagedRemoval>, StoreError> {
    let original = resolve_managed_path(root, rel).map_err(StoreError::Io)?;
    if !original.exists() {
        return Ok(None);
    }
    let pid = std::process::id();
    for attempt in 0..STAGE_ATTEMPTS {
        let staged_rel = format!("{rel}.opm-adopt-{pid}-{attempt}");
        let staged = resolve_managed_path(root, &staged_rel).map_err(StoreError::Io)?;
        match fs::symlink_metadata(&staged) {
            Ok(_) => continue,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::rename(&original, &staged).map_err(StoreError::Io)?;
                return Ok(Some(StagedRemoval { original, staged }));
            }
            Err(err) => return Err(StoreError::Io(err)),
        }
    }
    Err(StoreError::Message(format!(
        "Could not stage managed artifact for adoption: {rel}"
    )))
}

/// Roll back every staged removal of a failed adoption (reverse renames).
fn restore_all(staged: Vec<StagedRemoval>) {
    for item in staged {
        item.restore();
    }
}

fn rename_artifact(old_path: PathBuf, new_path: PathBuf) -> Result<(), StoreError> {
    if old_path.exists() && !new_path.exists() {
        fs::rename(old_path, new_path).map_err(StoreError::Io)?;
    }
    Ok(())
}

/// The persisted state of one work folder (interior mutability behind a
/// `Mutex`, §11 A6).
pub struct ProjectStore {
    inner: Mutex<StoreData>,
}

impl Default for ProjectStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectStore {
    /// Empty store; open or create a work folder to load state.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(StoreData::default()),
        }
    }

    /// Lock with poison recovery: a panicking caller must not brick the store.
    fn lock(&self) -> MutexGuard<'_, StoreData> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ---------------------------------------------------------- work lifecycle

    /// Open an existing work folder: the root is the directory containing
    /// `.pipeline-manager/run.json` (nothing else is required or read — no
    /// project marker, no legacy files, no available library file). A
    /// missing, corrupt or unsupported-version `run.json` is a visible error
    /// and never replaces the document.
    pub fn open_work(&self, root: &Path) -> Result<(), StoreError> {
        let root = fs::canonicalize(root).map_err(StoreError::Io)?;
        // F2: la ruta gestionada se resuelve ANTES del `exists()` — un
        // enlace colgante hacia fuera no se trata como «fichero ausente».
        let path = run_json_path(&root)?;
        if !path.exists() {
            return Err(StoreError::Message(format!(
                "{}: not a work folder (missing {RUN_JSON_REL})",
                root.display()
            )));
        }
        ensure_manager_dirs(&root)?;
        let text = fs::read_to_string(&path).map_err(StoreError::Io)?;
        let run = RunDoc::from_json(&text, &path)?;
        let mut fresh = StoreData {
            root: Some(root),
            source: run.source,
            definition: run.definition,
            values: run.values.into_iter().collect(),
            state: run.state,
            created_at: run.created_at,
            validation: Vec::new(),
        };
        resolve_input_sources(&mut fresh);
        initialize_pipeline_state(&mut fresh);
        *self.lock() = fresh;
        Ok(())
    }

    /// Create a new work folder at `root` with `Input/`, `steps/`, `outputs/`
    /// and `.pipeline-manager/{logs,done}`, then write the initial
    /// `run.json`. Nothing else is generated (no `project.config.json`, no
    /// `config.txt`); an existing `run.json` is never overwritten.
    pub fn create_work(&self, root: &Path, definition: &PipelineDoc) -> Result<(), StoreError> {
        ensure_supported(definition.format_version, &definition.name)?;
        fs::create_dir_all(root).map_err(StoreError::Io)?;
        let root = fs::canonicalize(root).map_err(StoreError::Io)?;
        let path = run_json_path(&root)?;
        if path.exists() {
            return Err(StoreError::Message(format!(
                "{}: work folder already exists (run.json present)",
                path.display()
            )));
        }
        for dirname in ["Input", "steps", "outputs"] {
            fs::create_dir_all(root.join(dirname)).map_err(StoreError::Io)?;
        }
        let mut doc = definition.clone();
        doc.normalize();
        if doc.created_at.is_empty() {
            doc.created_at = utc_iso_now();
        }
        let mut fresh = StoreData {
            root: Some(root.clone()),
            source: PipelineSource {
                name: doc.name.clone(),
                library_file: None,
            },
            definition: doc,
            values: HashMap::new(),
            state: BTreeMap::new(),
            created_at: utc_iso_now(),
            validation: Vec::new(),
        };
        resolve_input_sources(&mut fresh);
        initialize_pipeline_state(&mut fresh);
        for step_id in step_ids(&fresh.definition.pipeline) {
            ensure_step_folders(&root, &step_id).map_err(msg)?;
        }
        write_run(&mut fresh)?;
        *self.lock() = fresh;
        Ok(())
    }

    /// Drop the open work (in-memory state only; nothing on disk changes).
    pub fn close(&self) {
        *self.lock() = StoreData::default();
    }

    // ------------------------------------------------------- definition + save

    /// The embedded definition snapshot (library document of this work).
    pub fn definition(&self) -> PipelineDoc {
        self.lock().definition.clone()
    }

    /// Explicitly adopt a revised definition (the only way library edits can
    /// reach existing work):
    ///
    /// - steps whose fingerprint changed get their `.done` marker deleted and
    ///   their status reset to `Pending` (exit_code/message cleared);
    /// - steps removed from the definition are pruned entirely (values,
    ///   state, `.done`, log);
    /// - values are reconciled per kept step: keys still declared are kept,
    ///   unknown/stale keys dropped, declared defaults filled in;
    /// - unchanged steps keep their state and declared values;
    /// - the embedded snapshot and `source` are updated, step folders are
    ///   ensured, and everything is persisted atomically to `run.json` —
    ///   the library file itself is NEVER rewritten.
    ///
    /// Adoption is transactional with respect to artifacts: fallible,
    /// non-destructive work (step folders, serialization) runs first, the
    /// `.done`/log files being removed are parked as renames inside their
    /// managed directories, `run.json` commits atomically, and the parked
    /// copies are unlinked only after the commit. On any failure before the
    /// commit every staged artifact is restored — a failed adoption leaves
    /// memory, logs, markers and `run.json` exactly as they were.
    pub fn adopt_definition(&self, definition: &PipelineDoc) -> Result<AdoptReport, StoreError> {
        ensure_supported(definition.format_version, &definition.name)?;
        let mut current = self.lock();
        let root = current.root.clone().ok_or(StoreError::NoProject)?;

        let mut doc = definition.clone();
        doc.normalize();
        let mut fresh = StoreData {
            root: Some(root.clone()),
            source: PipelineSource {
                name: doc.name.clone(),
                library_file: None,
            },
            definition: doc,
            values: current.values.clone(),
            state: current.state.clone(),
            created_at: current.created_at.clone(),
            validation: Vec::new(),
        };
        // Resolve input sources of the incoming definition first, so the
        // fingerprint comparison is resolved-vs-resolved (the current
        // snapshot was resolved when it was opened or created).
        resolve_input_sources(&mut fresh);

        let new_ids: HashSet<String> = step_ids(&fresh.definition.pipeline);
        let mut changed = Vec::new();
        let mut added = Vec::new();
        let mut unchanged = Vec::new();
        for step in &fresh.definition.pipeline.steps {
            match current
                .definition
                .pipeline
                .steps
                .iter()
                .find(|previous| previous.id == step.id)
            {
                Some(previous) if fingerprint_changed(previous, step) => {
                    changed.push(step.id.clone())
                }
                Some(_) => unchanged.push(step.id.clone()),
                None => added.push(step.id.clone()),
            }
        }
        // Removed = every previously known step id (definition, state,
        // values) that the incoming definition no longer declares, reported
        // in deterministic order.
        let mut removed = Vec::new();
        {
            let mut seen = HashSet::new();
            let mut consider = |id: &str| {
                if !new_ids.contains(id) && seen.insert(id.to_string()) {
                    removed.push(id.to_string());
                }
            };
            for step in &current.definition.pipeline.steps {
                consider(&step.id);
            }
            let mut state_ids: Vec<&String> = current.state.keys().collect();
            state_ids.sort();
            for id in state_ids {
                consider(id);
            }
            let mut value_ids: Vec<&String> = current.values.keys().collect();
            value_ids.sort();
            for id in value_ids {
                consider(id);
            }
        }

        // In-memory pruning of removed steps, then the fallible,
        // non-destructive work (step folders) BEFORE any artifact is
        // touched: a failure here aborts with nothing staged.
        fresh.values.retain(|id, _| new_ids.contains(id));
        fresh.state.retain(|id, _| new_ids.contains(id));
        for step_id in &new_ids {
            ensure_step_folders(&root, step_id).map_err(msg)?;
        }
        if fresh.definition.created_at.is_empty() {
            fresh.definition.created_at = utc_iso_now();
        }
        // Reconcile declared values, seed state for new steps and downgrade
        // stale `ok` markers of unchanged steps; THEN invalidate the changed
        // steps (their markers are still on disk during the pass, so the
        // stale-`ok` check cannot resurrect them afterwards — `Pending` is
        // what gets serialized, and staging below parks the markers before
        // the commit).
        initialize_pipeline_state(&mut fresh);
        for id in &changed {
            if let Some(entry) = fresh.state.get_mut(id.as_str()) {
                entry.status = StepStatus::Pending;
                entry.exit_code = None;
                entry.message.clear();
            }
        }
        // Serialize BEFORE touching any artifact: a serialization failure
        // aborts with logs, markers and run.json untouched.
        let (run_path, run_text) = prepare_run(&mut fresh)?;

        // Stage the destructive removals as renames to unique siblings in
        // the same managed directories. Any staging failure (a poisoned
        // marker, a rename error) aborts the adoption and restores what was
        // already parked — nothing is silently skipped.
        let mut staged: Vec<StagedRemoval> = Vec::new();
        for id in &changed {
            match stage_removal(&root, &done_rel(id)) {
                Ok(Some(item)) => staged.push(item),
                Ok(None) => {}
                Err(err) => {
                    restore_all(staged);
                    return Err(err);
                }
            }
        }
        for id in &removed {
            for rel in [log_rel(id), done_rel(id)] {
                match stage_removal(&root, &rel) {
                    Ok(Some(item)) => staged.push(item),
                    Ok(None) => {}
                    Err(err) => {
                        restore_all(staged);
                        return Err(err);
                    }
                }
            }
        }
        // Commit: the atomic run.json swap is the point of no return. On
        // failure the parked artifacts go back where they were.
        if let Err(err) = write_atomic(&run_path, run_text.as_bytes()) {
            restore_all(staged);
            return Err(StoreError::Io(err));
        }
        for item in staged {
            item.commit();
        }
        let report = AdoptReport {
            changed,
            removed,
            added,
            unchanged,
        };
        *current = fresh;
        Ok(report)
    }

    /// Atomically write the current `.pipeline-manager/run.json`.
    pub fn save_run(&self) -> Result<(), StoreError> {
        let mut data = self.lock();
        write_run(&mut data)
    }

    // ------------------------------------------------------------- accessors

    /// Open work root, or [`StoreError::NoProject`].
    pub fn project_root(&self) -> Result<PathBuf, StoreError> {
        self.lock().root.clone().ok_or(StoreError::NoProject)
    }

    /// All pipeline steps of the embedded definition (empty before open).
    pub fn steps(&self) -> Vec<PipelineStep> {
        self.lock().definition.pipeline.steps.clone()
    }

    /// One step, if the definition declares it.
    pub fn step(&self, step_id: &str) -> Option<PipelineStep> {
        self.lock()
            .definition
            .pipeline
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .cloned()
    }

    /// Runtime values of a step (empty map when absent).
    pub fn params_for(&self, step_id: &str) -> ValueMap {
        self.lock().values.get(step_id).cloned().unwrap_or_default()
    }

    /// Runtime state of a step (default `pending` state when absent).
    pub fn state_of(&self, step_id: &str) -> StepState {
        self.lock()
            .state
            .get(step_id)
            .cloned()
            .unwrap_or_default()
    }

    /// `{project_path, pipeline, params, state, visual_layout, validation}`
    /// view for the UI; empty snapshot when no work is open. Validation is
    /// recalculated here (never persisted).
    pub fn snapshot(&self) -> ProjectSnapshot {
        let mut data = self.lock();
        if data.root.is_none() {
            return ProjectSnapshot::default();
        }
        recompute_validation(&mut data);
        snapshot_of(&data)
    }

    /// Step detail: step + values + state + this step's validation issues
    /// (recalculated on demand).
    pub fn step_detail(&self, step_id: &str) -> Result<StepDetail, StoreError> {
        let mut data = self.lock();
        let step = get_step(&data, step_id)?;
        let params = data.values.get(step_id).cloned().unwrap_or_default();
        let state = data.state.get(step_id).cloned().unwrap_or_default();
        recompute_validation(&mut data);
        let validation = data
            .validation
            .iter()
            .filter(|issue| issue.step_id.as_deref() == Some(step_id))
            .cloned()
            .collect();
        Ok(StepDetail {
            step,
            params,
            state,
            validation,
        })
    }

    // ------------------------------------------------------------------ steps

    /// `create_step`: normalize id (blank → error), default dependencies to
    /// the last step, normalize wd/name/environment, re-resolve input
    /// sources, ensure folders, seed values/state/layout, save.
    pub fn create_step(&self, step: PipelineStep) -> Result<(), StoreError> {
        let mut data = self.lock();
        if data.root.is_none() {
            return Err(StoreError::NoProject);
        }
        let step_id = normalize_step_id(&step.id)?;
        if data
            .definition
            .pipeline
            .steps
            .iter()
            .any(|existing| existing.id == step_id)
        {
            return Err(StoreError::StepExists(step_id));
        }
        let dependencies = if step.dependencies.is_empty() {
            data.definition
                .pipeline
                .steps
                .last()
                .map(|last| vec![last.id.clone()])
                .unwrap_or_default()
        } else {
            step.dependencies.clone()
        };
        let normalized = normalized_step(&step, &step_id, &dependencies);
        data.definition.pipeline.steps.push(normalized);
        resolve_input_sources(&mut data);
        let normalized = data
            .definition
            .pipeline
            .steps
            .last()
            .expect("step pushed")
            .clone();
        let root = data.root.clone().expect("work open");
        ensure_step_folders(&root, &normalized.id).map_err(msg)?;
        let runtime = runtime_params(&normalized, None);
        data.values.insert(normalized.id.clone(), runtime);
        data.state.insert(normalized.id.clone(), StepState::default());
        let index = data.definition.pipeline.steps.len() - 1;
        data.definition
            .layout
            .insert(normalized.id.clone(), default_step_position(index));
        write_run(&mut data)
    }

    /// `update_step`: rename propagates into dependencies + `source_step`
    /// refs, migrates values/state/layout and renames the log/done files when
    /// the target name is free.
    pub fn update_step(&self, old_id: &str, step: PipelineStep) -> Result<(), StoreError> {
        let mut data = self.lock();
        if data.root.is_none() {
            return Err(StoreError::NoProject);
        }
        let current_index = data
            .definition
            .pipeline
            .steps
            .iter()
            .position(|item| item.id == old_id)
            .ok_or_else(|| StoreError::UnknownStep(old_id.to_string()))?;
        let new_step_id = normalize_step_id(&step.id)?;
        if data
            .definition
            .pipeline
            .steps
            .iter()
            .any(|item| item.id == new_step_id && item.id != old_id)
        {
            return Err(StoreError::StepExists(new_step_id));
        }
        let normalized = normalized_step(&step, &new_step_id, &step.dependencies);
        data.definition.pipeline.steps[current_index] = normalized;
        if new_step_id != old_id {
            for item in &mut data.definition.pipeline.steps {
                for dependency in &mut item.dependencies {
                    if dependency == old_id {
                        *dependency = new_step_id.clone();
                    }
                }
                for input in &mut item.inputs {
                    if input.source_step.as_deref() == Some(old_id) {
                        input.source_step = Some(new_step_id.clone());
                    }
                }
            }
        }
        resolve_input_sources(&mut data);
        let normalized = data.definition.pipeline.steps[current_index].clone();
        if new_step_id != old_id {
            let mut existing_values = data.values.remove(old_id).unwrap_or_default();
            if let Some(newer) = data.values.remove(&new_step_id) {
                existing_values.extend(newer);
            }
            let runtime = runtime_params(&normalized, Some(&existing_values));
            data.values.insert(new_step_id.clone(), runtime);
            let state = data.state.remove(old_id).unwrap_or_default();
            data.state.insert(new_step_id.clone(), state);
            let position = data
                .definition
                .layout
                .remove(old_id)
                .unwrap_or_else(|| default_step_position(current_index));
            data.definition.layout.insert(new_step_id.clone(), position);
            if let Some(root) = data.root.clone() {
                // F2: contención + sin symlinks en ambos extremos; un
                // artefacto envenenado bloquea el rename con error claro.
                let old_log =
                    resolve_managed_path(&root, &log_rel(old_id)).map_err(StoreError::Io)?;
                let new_log =
                    resolve_managed_path(&root, &log_rel(&new_step_id)).map_err(StoreError::Io)?;
                rename_artifact(old_log, new_log)?;
                let old_done =
                    resolve_managed_path(&root, &done_rel(old_id)).map_err(StoreError::Io)?;
                let new_done =
                    resolve_managed_path(&root, &done_rel(&new_step_id)).map_err(StoreError::Io)?;
                rename_artifact(old_done, new_done)?;
            }
        } else {
            let existing = data.values.get(&new_step_id);
            let runtime = runtime_params(&normalized, existing);
            data.values.insert(new_step_id.clone(), runtime);
            data.state.entry(new_step_id.clone()).or_default();
            data.definition
                .layout
                .entry(new_step_id.clone())
                .or_insert_with(|| default_step_position(current_index));
        }
        let root = data.root.clone().expect("work open");
        ensure_step_folders(&root, &normalized.id).map_err(msg)?;
        write_run(&mut data)
    }

    /// `set_step_dependencies` (Python `set_step_dependencies`): dedup,
    /// reject self-dependency and unknown references, save.
    pub fn set_dependencies(
        &self,
        step_id: &str,
        dependencies: Vec<String>,
    ) -> Result<(), StoreError> {
        let mut data = self.lock();
        if data.root.is_none() {
            return Err(StoreError::NoProject);
        }
        get_step(&data, step_id)?;
        let known: HashSet<String> = step_ids(&data.definition.pipeline);
        let normalized = dedup(&dependencies);
        if normalized.iter().any(|dependency| dependency == step_id) {
            return Err(StoreError::SelfDependency);
        }
        if let Some(unknown) = normalized.iter().find(|dependency| !known.contains(*dependency)) {
            return Err(StoreError::UnknownDependency(unknown.clone()));
        }
        let step = data
            .definition
            .pipeline
            .steps
            .iter_mut()
            .find(|item| item.id == step_id)
            .expect("step exists");
        step.dependencies = normalized;
        write_run(&mut data)
    }

    /// Merge the given values into the step's runtime values and drop keys
    /// that are not declared inputs/parameters of the step.
    pub fn set_params(
        &self,
        step_id: &str,
        params: BTreeMap<String, Value>,
    ) -> Result<(), StoreError> {
        let mut data = self.lock();
        if data.root.is_none() {
            return Err(StoreError::NoProject);
        }
        let step = get_step(&data, step_id)?;
        let mut merged = data.values.get(step_id).cloned().unwrap_or_default();
        merged.extend(params);
        let runtime = runtime_params(&step, Some(&merged));
        data.values.insert(step_id.to_string(), runtime);
        write_run(&mut data)
    }

    /// Toggle the `selected` flag of a step (Python `set_step_selected`).
    pub fn toggle_selected(&self, step_id: &str) -> Result<StepState, StoreError> {
        let mut data = self.lock();
        if data.root.is_none() {
            return Err(StoreError::NoProject);
        }
        get_step(&data, step_id)?;
        let entry = data.state.entry(step_id.to_string()).or_default();
        entry.selected = !entry.selected;
        let state = entry.clone();
        write_run(&mut data)?;
        Ok(state)
    }

    /// Merge canvas positions into the embedded snapshot (no-op when no work
    /// is open, Python `update_layout`) and save.
    pub fn set_layout(&self, positions: HashMap<String, (f64, f64)>) -> Result<(), StoreError> {
        let mut data = self.lock();
        if data.root.is_none() {
            return Ok(());
        }
        for (step_id, (x, y)) in positions {
            data.definition.layout.insert(step_id, Position { x, y });
        }
        write_run(&mut data)
    }

    // ------------------------------------------------------------ persistence

    /// Recalculated issues (validation is never persisted; §4: no `Result` —
    /// persistence failures cannot surface here).
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut data = self.lock();
        recompute_validation(&mut data);
        data.validation.clone()
    }

    /// Recalculate + atomically save `run.json` (the engine calls it after
    /// each step). Persistence failures cannot surface (§4 signatures); the
    /// in-memory state stays authoritative.
    pub fn validate_and_save(&self) -> Vec<ValidationIssue> {
        let mut data = self.lock();
        recompute_validation(&mut data);
        let issues = data.validation.clone();
        let _ = write_run(&mut data);
        issues
    }

    /// `is_step_ok(id)`: `.done` marker AND all resolved outputs exist; an
    /// unsafe path → false.
    pub fn is_step_ok(&self, step_id: &str) -> bool {
        let data = self.lock();
        let (Some(root), step) = (
            data.root.as_ref(),
            data.definition
                .pipeline
                .steps
                .iter()
                .find(|step| step.id == step_id),
        ) else {
            return false;
        };
        match step {
            Some(step) => is_step_ok_at(root, step),
            None => false,
        }
    }

    // -------------------------------------------------- engine-facing mutations

    /// Replace the runtime state of a step and persist it (§11 A6).
    pub fn set_step_state(&self, step_id: &str, state: StepState) {
        let mut data = self.lock();
        data.state.insert(step_id.to_string(), state);
        let _ = write_run(&mut data);
    }

    /// `.pipeline-manager/logs/<step id>.log` (F3: `/`, `\` y `:` del id
    /// se mapean a `_`); empty path when no work is open.
    pub fn log_path(&self, step_id: &str) -> PathBuf {
        match self.lock().root.as_ref() {
            Some(root) => log_path_in(root, step_id),
            None => PathBuf::new(),
        }
    }

    /// `.pipeline-manager/done/<step id>.done` (F3: mismos mapeos de
    /// nombre); empty path when no work is open.
    pub fn done_path(&self, step_id: &str) -> PathBuf {
        match self.lock().root.as_ref() {
            Some(root) => done_path_in(root, step_id),
            None => PathBuf::new(),
        }
    }

    /// F2 guard for the engine preflight: the step's managed artifact paths
    /// (log + done) must be contained in the work folder and symlink-free,
    /// or the run is refused with this clear error before anything starts.
    pub fn ensure_step_artifacts_safe(&self, step_id: &str) -> Result<(), StoreError> {
        let data = self.lock();
        let root = data.root.as_ref().ok_or(StoreError::NoProject)?;
        resolve_managed_path(root, &log_rel(step_id)).map_err(StoreError::Io)?;
        resolve_managed_path(root, &done_rel(step_id)).map_err(StoreError::Io)?;
        Ok(())
    }

    /// Append raw text (log lines/headers) to the step log (§11 A6).
    ///
    /// F2: la ruta gestionada se comprueba antes de abrir — un log
    /// envenenado (symlink a un fichero de usuario, id con NUL…) se ignora
    /// con cero escrituras fuera del proyecto, jamás se sigue el enlace.
    pub fn append_log(&self, step_id: &str, text: &str) {
        let data = self.lock();
        let Some(root) = data.root.as_ref() else {
            return;
        };
        let Ok(path) = resolve_managed_path(root, &log_rel(step_id)) else {
            return;
        };
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(text.as_bytes());
            let _ = file.flush();
        }
    }

    /// Write the step's `.done` marker with a UTC ISO timestamp (§11 A6).
    /// F2: igual que [`Self::append_log`] — contención + sin symlinks o no
    /// se escribe.
    pub fn write_done_marker(&self, step_id: &str, timestamp: &str) {
        let data = self.lock();
        let Some(root) = data.root.as_ref() else {
            return;
        };
        if let Ok(path) = resolve_managed_path(root, &done_rel(step_id)) {
            let _ = fs::write(path, timestamp);
        }
    }

    /// Full log text of a step (empty when missing/unreadable).
    /// F2: una ruta envenenada se trata como inexistente — nunca se lee a
    /// través de un symlink hacia fuera del proyecto.
    pub fn log_content(&self, step_id: &str) -> String {
        let data = self.lock();
        let Some(root) = data.root.as_ref() else {
            return String::new();
        };
        match resolve_managed_path(root, &log_rel(step_id)) {
            Ok(path) => fs::read_to_string(path).unwrap_or_default(),
            Err(_) => String::new(),
        }
    }
}

/// `list(dict.fromkeys(items))` — dedup keeping first occurrence order.
fn dedup(items: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item.clone());
        }
    }
    out
}

/// `storage.py:_normalize_step_id`.
fn normalize_step_id(step_id: &str) -> Result<String, StoreError> {
    let requested = step_id.trim();
    if requested.is_empty() {
        return Err(StoreError::StepIdRequired);
    }
    Ok(safe_segment(requested))
}

/// `storage.py:_normalized_step`.
fn normalized_step(step: &PipelineStep, step_id: &str, dependencies: &[String]) -> PipelineStep {
    let working_directory = if step.working_directory.is_empty() {
        format!("steps/{step_id}/work")
    } else {
        step.working_directory.clone()
    };
    let trimmed_name = step.name.trim();
    let name = if trimmed_name.is_empty() {
        step_id.to_string()
    } else {
        trimmed_name.to_string()
    };
    let mut normalized = step.clone();
    normalized.id = step_id.to_string();
    normalized.name = name;
    normalized.environment = step.environment.trim().to_string();
    normalized.working_directory = working_directory;
    normalized.dependencies = dedup(dependencies);
    normalized
}
