//! [`ProjectStore`] — port of `backend/app/services/storage.py`.
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
    ensure_project_scaffold, ensure_step_folders, relative_project_path, resolve_managed_path,
    resolve_pipeline_path, safe_segment, validate_project_folder, PathError,
};
use opm_pipeline::{
    parse_pipeline, validate_pipeline, PipelineConfig, PipelineStep, ValidationIssue, Value,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::state::{Position, ProjectSnapshot, StepDetail, StepState, StepStatus};
use crate::ValueMap;

/// `LAYOUT_START_X` (`storage.py`).
const LAYOUT_START_X: f64 = 80.0;
/// `LAYOUT_START_Y`.
const LAYOUT_START_Y: f64 = 120.0;
/// `LAYOUT_GAP_X`.
const LAYOUT_GAP_X: f64 = 330.0;
/// Manager folder below the project root.
const MANAGER_DIR: &str = ".pipeline-manager";

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

/// In-memory project state; all fields are replaced wholesale on open (staged
/// load, so a failed open leaves the previous project untouched).
#[derive(Default)]
struct StoreData {
    current_path: Option<PathBuf>,
    pipeline: Option<PipelineConfig>,
    params: HashMap<String, ValueMap>,
    state: HashMap<String, StepState>,
    visual_layout: HashMap<String, Position>,
    validation: Vec<ValidationIssue>,
}

/// F3: segmento de nombre de artefacto — `/`, `\` y `:` se mapean a `_`.
/// El mapeo `/`→`_` es exactamente el de Python (paridad POSIX); `\` y `:`
/// cierran la traversión de Windows (`C:\victim\x`, `..\..\x`). Los ids
/// creados por la UI ya pasan por `safe_segment`; los importados llegan
/// crudos y esta capa los reduce a un segmento plano inofensivo.
fn artifact_segment(step_id: &str) -> String {
    step_id.replace(['/', '\\', ':'], "_")
}

/// Ruta relativa (con `/`) del log de un paso dentro del proyecto.
fn log_rel(step_id: &str) -> String {
    format!("{MANAGER_DIR}/logs/{}.log", artifact_segment(step_id))
}

/// Ruta relativa (con `/`) del `.done` de un paso dentro del proyecto.
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

/// F2: lectura de un JSON de runtime — ruta gestionada contenida y sin
/// symlinks; un enlace colgante jamás se sigue (ni lectura ni — vía
/// [`write_json`] — creación fuera del proyecto).
fn read_json<T: DeserializeOwned>(root: &Path, rel: &str, default: T) -> Result<T, StoreError> {
    let path = resolve_managed_path(root, rel).map_err(StoreError::Io)?;
    if !path.exists() {
        return Ok(default);
    }
    let text = fs::read_to_string(&path).map_err(StoreError::Io)?;
    serde_json::from_str(&text)
        .map_err(|e| StoreError::Message(format!("{}: {e}", path.display())))
}

fn write_json<T: Serialize>(root: &Path, rel: &str, data: &T) -> Result<(), StoreError> {
    let path = resolve_managed_path(root, rel).map_err(StoreError::Io)?;
    let text = serde_json::to_string_pretty(data)
        .map_err(|e| StoreError::Message(e.to_string()))?;
    fs::write(path, text).map_err(StoreError::Io)
}

/// `storage.py:_default_step_position`.
fn default_step_position(index: usize) -> Position {
    Position {
        x: LAYOUT_START_X + index as f64 * LAYOUT_GAP_X,
        y: LAYOUT_START_Y,
    }
}

fn default_visual_layout(pipeline: &PipelineConfig) -> HashMap<String, Position> {
    pipeline
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.clone(), default_step_position(index)))
        .collect()
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

/// `storage.py:_runtime_params`: declared defaults, then the existing values
/// whose keys are declared as inputs/parameters of the step (so stale output
/// keys are dropped).
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
/// to project-relative paths (consuming-step wd → source output), mirrors the
/// value into `params[step][key]` and appends the implicit dependency.
fn resolve_input_sources(data: &mut StoreData) {
    let StoreData {
        current_path,
        pipeline,
        params,
        ..
    } = data;
    let (Some(root), Some(pipeline)) = (current_path.as_ref(), pipeline.as_mut()) else {
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
                params
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

/// `storage.py:_connect_linear_pipeline_if_unconnected`.
fn connect_linear_pipeline(pipeline: &mut PipelineConfig) {
    if pipeline.steps.iter().any(|step| !step.dependencies.is_empty()) {
        return;
    }
    for index in 1..pipeline.steps.len() {
        let previous_id = pipeline.steps[index - 1].id.clone();
        pipeline.steps[index].dependencies = vec![previous_id];
    }
}

/// `storage.py:_prune_removed_step_runtime`, incl. deleting logs/done files.
fn prune_removed_runtime(
    data: &mut StoreData,
    root: &Path,
    current_step_ids: &HashSet<String>,
    known_step_ids: &HashSet<String>,
) {
    data.params.retain(|id, _| current_step_ids.contains(id));
    data.state.retain(|id, _| current_step_ids.contains(id));
    data.visual_layout.retain(|id, _| current_step_ids.contains(id));
    for step_id in known_step_ids.difference(current_step_ids) {
        // F2/F3: contención ANTES de borrar — jamás se borra a través de un
        // symlink ni fuera del proyecto, aunque el id venga del runtime
        // (`C:\victim\x`, `..\..\x`) o `.pipeline-manager/logs` esté
        // enlazado hacia fuera.
        if let Ok(path) = resolve_managed_path(root, &log_rel(step_id)) {
            let _ = fs::remove_file(path);
        }
        if let Ok(path) = resolve_managed_path(root, &done_rel(step_id)) {
            let _ = fs::remove_file(path);
        }
    }
}

/// `storage.py:_initialize_pipeline_state`: `ok` iff `is_step_ok`, stale
/// `ok` → `pending` (clearing exit_code/message).
fn initialize_pipeline_state(data: &mut StoreData) {
    let StoreData {
        current_path,
        pipeline,
        params,
        state,
        ..
    } = data;
    let (Some(root), Some(pipeline)) = (current_path.as_ref(), pipeline.as_ref()) else {
        return;
    };
    for step in &pipeline.steps {
        let existing = params.get(&step.id);
        let runtime = runtime_params(step, existing);
        params.insert(step.id.clone(), runtime);
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
    let pipeline = data.pipeline.as_ref().ok_or(StoreError::NoPipeline)?;
    pipeline
        .steps
        .iter()
        .find(|step| step.id == step_id)
        .cloned()
        .ok_or_else(|| StoreError::UnknownStep(step_id.to_string()))
}

fn step_ids(pipeline: &PipelineConfig) -> HashSet<String> {
    pipeline.steps.iter().map(|step| step.id.clone()).collect()
}

/// `storage.py:validate` — re-resolve input sources, validate, persist
/// `validation.json`. Errors propagate so callers that hold a `Result` can
/// report them; the `Result`-less public wrappers swallow (§4 signatures).
fn validate_inner(data: &mut StoreData) -> Result<(), StoreError> {
    if data.pipeline.is_some() {
        resolve_input_sources(data);
        let StoreData {
            current_path,
            pipeline,
            params,
            validation,
            ..
        } = data;
        let root = current_path.as_ref().ok_or(StoreError::NoProject)?;
        let pipeline = pipeline.as_ref().ok_or(StoreError::NoPipeline)?;
        *validation = validate_pipeline(pipeline, root, params);
    } else {
        data.validation.clear();
        if data.current_path.is_none() {
            return Ok(());
        }
    }
    let root = data.current_path.as_ref().ok_or(StoreError::NoProject)?;
    ensure_manager_dirs(root)?;
    write_json(
        root,
        &format!("{MANAGER_DIR}/validation.json"),
        &data.validation,
    )?;
    Ok(())
}

/// `storage.py:save_all` — params/state/visual-layout JSON files +
/// `pipeline.project.json` (by-alias JSON).
fn save_all_inner(data: &StoreData) -> Result<(), StoreError> {
    let root = data.current_path.as_ref().ok_or(StoreError::NoProject)?;
    ensure_manager_dirs(root)?;
    write_json(root, &format!("{MANAGER_DIR}/params.json"), &data.params)?;
    write_json(root, &format!("{MANAGER_DIR}/state.json"), &data.state)?;
    write_json(
        root,
        &format!("{MANAGER_DIR}/visual-layout.json"),
        &data.visual_layout,
    )?;
    let snapshot = snapshot_of(data);
    let text = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| StoreError::Message(e.to_string()))?;
    let snapshot_path =
        resolve_managed_path(root, "pipeline.project.json").map_err(StoreError::Io)?;
    fs::write(snapshot_path, text).map_err(StoreError::Io)?;
    Ok(())
}

fn snapshot_of(data: &StoreData) -> ProjectSnapshot {
    let project_path = data
        .current_path
        .as_ref()
        .map(|root| root.to_string_lossy().into_owned())
        .unwrap_or_default();
    ProjectSnapshot {
        project_path,
        pipeline: data.pipeline.clone(),
        params: data.params.clone(),
        state: data.state.clone(),
        visual_layout: data.visual_layout.clone(),
        validation: data.validation.clone(),
    }
}

fn load_runtime_files(data: &mut StoreData, root: &Path) -> Result<(), StoreError> {
    data.params = read_json(root, &format!("{MANAGER_DIR}/params.json"), HashMap::new())?;
    data.state = read_json(root, &format!("{MANAGER_DIR}/state.json"), HashMap::new())?;
    data.visual_layout = read_json(
        root,
        &format!("{MANAGER_DIR}/visual-layout.json"),
        HashMap::new(),
    )?;
    data.validation = read_json(root, &format!("{MANAGER_DIR}/validation.json"), Vec::new())?;
    Ok(())
}

/// `storage.py:_load_snapshot`: snapshot replaces `pipeline` outright and
/// overrides each non-empty runtime collection (`storage.py:408-421`), then
/// re-resolves input sources and initializes the pipeline state.
fn load_snapshot(data: &mut StoreData, root: &Path) -> Result<(), StoreError> {
    // F2: la ruta gestionada se comprueba ANTES del `exists()` — un enlace
    // colgante hacia fuera no se trata como «fichero ausente» (y nunca se
    // seguirá al guardar después).
    let snapshot_path =
        resolve_managed_path(root, "pipeline.project.json").map_err(StoreError::Io)?;
    if !snapshot_path.exists() {
        data.pipeline = None;
        return Ok(());
    }
    let text = fs::read_to_string(&snapshot_path).map_err(StoreError::Io)?;
    let snapshot: ProjectSnapshot = serde_json::from_str(&text)
        .map_err(|e| StoreError::Message(format!("pipeline.project.json: {e}")))?;
    data.pipeline = snapshot.pipeline;
    if !snapshot.params.is_empty() {
        data.params = snapshot.params;
    }
    if !snapshot.state.is_empty() {
        data.state = snapshot.state;
    }
    if !snapshot.visual_layout.is_empty() {
        data.visual_layout = snapshot.visual_layout;
    }
    if !snapshot.validation.is_empty() {
        data.validation = snapshot.validation;
    }
    resolve_input_sources(data);
    initialize_pipeline_state(data);
    Ok(())
}

fn rename_artifact(old_path: PathBuf, new_path: PathBuf) -> Result<(), StoreError> {
    if old_path.exists() && !new_path.exists() {
        fs::rename(old_path, new_path).map_err(StoreError::Io)?;
    }
    Ok(())
}

/// The persisted project state (interior mutability behind a `Mutex`, §11 A6).
pub struct ProjectStore {
    inner: Mutex<StoreData>,
}

impl Default for ProjectStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectStore {
    /// Empty store; open/create a project to load state.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(StoreData::default()),
        }
    }

    /// Lock with poison recovery: a panicking caller must not brick the store.
    fn lock(&self) -> MutexGuard<'_, StoreData> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ---------------------------------------------------------- project lifecycle

    /// `create_project(parent, name)`: `parent.resolve() / safe_segment(name)`
    /// + scaffold, then a full load.
    pub fn create_project(&self, parent: &Path, name: &str) -> Result<(), StoreError> {
        let parent = fs::canonicalize(parent).map_err(StoreError::Io)?;
        let safe_name = safe_segment(name);
        let root = parent.join(&safe_name);
        ensure_project_scaffold(&root, &safe_name).map_err(msg)?;
        let root = fs::canonicalize(&root).map_err(StoreError::Io)?;
        self.load_project(root)
    }

    /// `open_project(root)`: the folder must contain `project.config.json`.
    pub fn open_project(&self, root: &Path) -> Result<(), StoreError> {
        let root = fs::canonicalize(root).map_err(StoreError::Io)?;
        validate_project_folder(&root).map_err(msg)?;
        self.load_project(root)
    }

    fn load_project(&self, root: PathBuf) -> Result<(), StoreError> {
        let mut fresh = StoreData {
            current_path: Some(root.clone()),
            ..StoreData::default()
        };
        ensure_manager_dirs(&root)?;
        load_runtime_files(&mut fresh, &root)?;
        load_snapshot(&mut fresh, &root)?;
        *self.lock() = fresh;
        Ok(())
    }

    /// Drop the open project (in-memory state only; nothing on disk changes).
    pub fn close(&self) {
        *self.lock() = StoreData::default();
    }

    // ------------------------------------------------------------- accessors

    /// Open project root, or [`StoreError::NoProject`].
    pub fn project_root(&self) -> Result<PathBuf, StoreError> {
        self.lock().current_path.clone().ok_or(StoreError::NoProject)
    }

    /// All pipeline steps (empty when no pipeline has been imported).
    pub fn steps(&self) -> Vec<PipelineStep> {
        self.lock()
            .pipeline
            .as_ref()
            .map(|pipeline| pipeline.steps.clone())
            .unwrap_or_default()
    }

    /// One step, if the pipeline declares it.
    pub fn step(&self, step_id: &str) -> Option<PipelineStep> {
        self.lock()
            .pipeline
            .as_ref()?
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .cloned()
    }

    /// Runtime params of a step (empty map when absent).
    pub fn params_for(&self, step_id: &str) -> ValueMap {
        self.lock().params.get(step_id).cloned().unwrap_or_default()
    }

    /// Runtime state of a step (default `pending` state when absent).
    pub fn state_of(&self, step_id: &str) -> StepState {
        self.lock()
            .state
            .get(step_id)
            .cloned()
            .unwrap_or_default()
    }

    /// `{project_path, pipeline, params, state, visual_layout, validation}`;
    /// empty snapshot when no project is open (Python parity).
    pub fn snapshot(&self) -> ProjectSnapshot {
        let data = self.lock();
        if data.current_path.is_none() {
            return ProjectSnapshot::default();
        }
        snapshot_of(&data)
    }

    /// Step detail: step + params + state + this step's validation issues.
    pub fn step_detail(&self, step_id: &str) -> Result<StepDetail, StoreError> {
        let mut data = self.lock();
        let step = get_step(&data, step_id)?;
        let params = data.params.get(step_id).cloned().unwrap_or_default();
        let state = data.state.get(step_id).cloned().unwrap_or_default();
        validate_inner(&mut data)?;
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

    // ---------------------------------------------------------------- import/export

    /// Import a `.yml`/`.yaml`/`.json` pipeline: resolve input sources, chain
    /// linearly when fully unconnected, prune runtime of removed steps (their
    /// logs/done files included), reset the layout to the default left-to-right
    /// one, ensure step folders, initialize state, validate + save.
    /// Returns the step ids in pipeline order.
    pub fn import_pipeline(&self, file: &Path) -> Result<Vec<String>, StoreError> {
        let mut data = self.lock();
        let root = data.current_path.clone().ok_or(StoreError::NoProject)?;
        let mut pipeline_path = file.to_path_buf();
        if pipeline_path.is_relative() {
            pipeline_path = root.join(&pipeline_path);
        }

        let mut known_step_ids: HashSet<String> = data.params.keys().cloned().collect();
        known_step_ids.extend(data.state.keys().cloned());
        known_step_ids.extend(data.visual_layout.keys().cloned());
        if let Some(pipeline) = &data.pipeline {
            known_step_ids.extend(step_ids(pipeline));
        }

        let text = fs::read_to_string(&pipeline_path).map_err(StoreError::Io)?;
        let pipeline = parse_pipeline(&pipeline_path.to_string_lossy(), &text)
            .map_err(|e| StoreError::Message(e.to_string()))?;

        data.pipeline = Some(pipeline);
        resolve_input_sources(&mut data);
        if let Some(pipeline) = data.pipeline.as_mut() {
            connect_linear_pipeline(pipeline);
        }
        let ordered_ids: Vec<String> = data
            .pipeline
            .as_ref()
            .map(|pipeline| pipeline.steps.iter().map(|step| step.id.clone()).collect())
            .unwrap_or_default();
        let current_step_ids: HashSet<String> = ordered_ids.iter().cloned().collect();
        prune_removed_runtime(&mut data, &root, &current_step_ids, &known_step_ids);
        data.visual_layout = data
            .pipeline
            .as_ref()
            .map(default_visual_layout)
            .unwrap_or_default();
        for step_id in &ordered_ids {
            ensure_step_folders(&root, step_id).map_err(msg)?;
        }
        initialize_pipeline_state(&mut data);
        validate_inner(&mut data)?;
        save_all_inner(&data)?;
        Ok(ordered_ids)
    }

    /// Export the pipeline as by-alias JSON (`routes.py:export_pipeline`:
    /// relative destinations resolve against the project).
    pub fn export_pipeline(&self, dest: &Path) -> Result<(), StoreError> {
        let data = self.lock();
        let pipeline = data.pipeline.as_ref().ok_or(StoreError::NoPipeline)?;
        let mut target = dest.to_path_buf();
        if target.is_relative() {
            let root = data.current_path.as_ref().ok_or(StoreError::NoProject)?;
            target = root.join(&target);
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(StoreError::Io)?;
        }
        let text = serde_json::to_string_pretty(pipeline)
            .map_err(|e| StoreError::Message(e.to_string()))?;
        fs::write(target, text).map_err(StoreError::Io)?;
        Ok(())
    }

    // ------------------------------------------------------------------ steps

    /// `create_step`: normalize id (blank → error), default dependencies to
    /// the last step, normalize wd/name/environment, re-resolve input sources,
    /// ensure folders, seed params/state/layout, validate + save.
    pub fn create_step(&self, step: PipelineStep) -> Result<(), StoreError> {
        let mut data = self.lock();
        if data.current_path.is_none() {
            return Err(StoreError::NoProject);
        }
        if data.pipeline.is_none() {
            data.pipeline = Some(PipelineConfig::default());
        }
        let pipeline = data.pipeline.as_mut().expect("pipeline initialized");
        let step_id = normalize_step_id(&step.id)?;
        if pipeline.steps.iter().any(|existing| existing.id == step_id) {
            return Err(StoreError::StepExists(step_id));
        }
        let dependencies = if step.dependencies.is_empty() {
            pipeline
                .steps
                .last()
                .map(|last| vec![last.id.clone()])
                .unwrap_or_default()
        } else {
            step.dependencies.clone()
        };
        let normalized = normalized_step(&step, &step_id, &dependencies);
        pipeline.steps.push(normalized);
        resolve_input_sources(&mut data);
        let normalized = data
            .pipeline
            .as_ref()
            .expect("pipeline present")
            .steps
            .last()
            .expect("step pushed")
            .clone();
        let root = data.current_path.as_ref().expect("project open").clone();
        ensure_step_folders(&root, &normalized.id).map_err(msg)?;
        let runtime = runtime_params(&normalized, None);
        data.params.insert(normalized.id.clone(), runtime);
        data.state.insert(normalized.id.clone(), StepState::default());
        let index = data.pipeline.as_ref().expect("pipeline present").steps.len() - 1;
        data.visual_layout
            .insert(normalized.id.clone(), default_step_position(index));
        validate_inner(&mut data)?;
        save_all_inner(&data)
    }

    /// `update_step`: rename propagates into dependencies + `source_step`
    /// refs, migrates params/state/layout and renames the log/done files when
    /// the target name is free.
    pub fn update_step(&self, old_id: &str, step: PipelineStep) -> Result<(), StoreError> {
        let mut data = self.lock();
        if data.current_path.is_none() {
            return Err(StoreError::NoProject);
        }
        let pipeline = data.pipeline.as_mut().ok_or(StoreError::NoPipeline)?;
        let current_index = pipeline
            .steps
            .iter()
            .position(|item| item.id == old_id)
            .ok_or_else(|| StoreError::UnknownStep(old_id.to_string()))?;
        let new_step_id = normalize_step_id(&step.id)?;
        if pipeline
            .steps
            .iter()
            .any(|item| item.id == new_step_id && item.id != old_id)
        {
            return Err(StoreError::StepExists(new_step_id));
        }
        let normalized = normalized_step(&step, &new_step_id, &step.dependencies);
        pipeline.steps[current_index] = normalized;
        if new_step_id != old_id {
            for item in &mut pipeline.steps {
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
        let normalized = data
            .pipeline
            .as_ref()
            .expect("pipeline present")
            .steps[current_index]
            .clone();
        if new_step_id != old_id {
            let mut existing_params = data.params.remove(old_id).unwrap_or_default();
            if let Some(newer) = data.params.remove(&new_step_id) {
                existing_params.extend(newer);
            }
            let runtime = runtime_params(&normalized, Some(&existing_params));
            data.params.insert(new_step_id.clone(), runtime);
            let state = data.state.remove(old_id).unwrap_or_default();
            data.state.insert(new_step_id.clone(), state);
            let position = data
                .visual_layout
                .remove(old_id)
                .unwrap_or_else(|| default_step_position(current_index));
            data.visual_layout.insert(new_step_id.clone(), position);
            if let Some(root) = data.current_path.clone() {
                // F2: contención + sin symlinks en ambos extremos; un
                // artefactual envenenado bloquea el rename con error claro.
                let old_log =
                    resolve_managed_path(&root, &log_rel(old_id)).map_err(StoreError::Io)?;
                let new_log = resolve_managed_path(&root, &log_rel(&new_step_id))
                    .map_err(StoreError::Io)?;
                rename_artifact(old_log, new_log)?;
                let old_done =
                    resolve_managed_path(&root, &done_rel(old_id)).map_err(StoreError::Io)?;
                let new_done = resolve_managed_path(&root, &done_rel(&new_step_id))
                    .map_err(StoreError::Io)?;
                rename_artifact(old_done, new_done)?;
            }
        } else {
            let existing = data.params.get(&new_step_id);
            let runtime = runtime_params(&normalized, existing);
            data.params.insert(new_step_id.clone(), runtime);
            data.state.entry(new_step_id.clone()).or_default();
            data.visual_layout
                .entry(new_step_id.clone())
                .or_insert_with(|| default_step_position(current_index));
        }
        let root = data.current_path.as_ref().expect("project open").clone();
        ensure_step_folders(&root, &normalized.id).map_err(msg)?;
        validate_inner(&mut data)?;
        save_all_inner(&data)
    }

    /// `set_step_dependencies` (Python `set_step_dependencies`): dedup,
    /// reject self-dependency and unknown references, validate + save.
    pub fn set_dependencies(&self, step_id: &str, dependencies: Vec<String>) -> Result<(), StoreError> {
        let mut data = self.lock();
        if data.pipeline.is_none() {
            return Err(StoreError::NoPipeline);
        }
        get_step(&data, step_id)?;
        let known: HashSet<String> = step_ids(data.pipeline.as_ref().expect("checked"));
        let normalized = dedup(&dependencies);
        if normalized.iter().any(|dependency| dependency == step_id) {
            return Err(StoreError::SelfDependency);
        }
        if let Some(unknown) = normalized.iter().find(|dependency| !known.contains(*dependency)) {
            return Err(StoreError::UnknownDependency(unknown.clone()));
        }
        let pipeline = data.pipeline.as_mut().expect("checked");
        let step = pipeline
            .steps
            .iter_mut()
            .find(|item| item.id == step_id)
            .expect("step exists");
        step.dependencies = normalized;
        validate_inner(&mut data)?;
        save_all_inner(&data)
    }

    /// `update_step_params`: merge the given values into the runtime params
    /// and drop keys that are not declared inputs/parameters of the step.
    pub fn set_params(&self, step_id: &str, params: BTreeMap<String, Value>) -> Result<(), StoreError> {
        let mut data = self.lock();
        let step = get_step(&data, step_id)?;
        let mut merged = data.params.get(step_id).cloned().unwrap_or_default();
        merged.extend(params);
        let runtime = runtime_params(&step, Some(&merged));
        data.params.insert(step_id.to_string(), runtime);
        validate_inner(&mut data)?;
        save_all_inner(&data)
    }

    /// Toggle the `selected` flag of a step (Python `set_step_selected`).
    pub fn toggle_selected(&self, step_id: &str) -> Result<StepState, StoreError> {
        let mut data = self.lock();
        get_step(&data, step_id)?;
        let entry = data.state.entry(step_id.to_string()).or_default();
        entry.selected = !entry.selected;
        let state = entry.clone();
        save_all_inner(&data)?;
        Ok(state)
    }

    /// Merge canvas positions (no-op when no project is open, Python
    /// `update_layout`) and save.
    pub fn set_layout(&self, positions: HashMap<String, (f64, f64)>) -> Result<(), StoreError> {
        let mut data = self.lock();
        if data.current_path.is_none() {
            return Ok(());
        }
        for (step_id, (x, y)) in positions {
            data.visual_layout.insert(step_id, Position { x, y });
        }
        save_all_inner(&data)
    }

    // ------------------------------------------------------------ persistence

    /// Persist `validation.json` and return the current issues (§4: no
    /// `Result` — persistence failures cannot surface here).
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut data = self.lock();
        let _ = validate_inner(&mut data);
        data.validation.clone()
    }

    /// Re-validate, persist `validation.json` + save everything (the engine
    /// calls it after each step).
    pub fn validate_and_save(&self) -> Vec<ValidationIssue> {
        let mut data = self.lock();
        let _ = validate_inner(&mut data);
        let issues = data.validation.clone();
        let _ = save_all_inner(&data);
        issues
    }

    /// Write `.pipeline-manager/{params,state,visual-layout}.json` +
    /// `pipeline.project.json`.
    pub fn save_all(&self) -> Result<(), StoreError> {
        let data = self.lock();
        save_all_inner(&data)
    }

    /// `is_step_ok(id)`: `.done` marker AND all resolved outputs exist; an
    /// unsafe path → false.
    pub fn is_step_ok(&self, step_id: &str) -> bool {
        let data = self.lock();
        let (Some(root), Some(step)) = (
            data.current_path.as_ref(),
            data.pipeline.as_ref().and_then(|pipeline| {
                pipeline.steps.iter().find(|step| step.id == step_id)
            }),
        ) else {
            return false;
        };
        is_step_ok_at(root, step)
    }

    // -------------------------------------------------- engine-facing mutations

    /// Replace the runtime state of a step and persist it (§11 A6). Persistence
    /// failures cannot surface (unit return per the contract); the in-memory
    /// state stays authoritative.
    pub fn set_step_state(&self, step_id: &str, state: StepState) {
        let mut data = self.lock();
        data.state.insert(step_id.to_string(), state);
        let _ = save_all_inner(&data);
    }

    /// `.pipeline-manager/logs/<step id>.log` (F3: `/`, `\` y `:` del id
    /// se mapean a `_`); empty path when no project is open.
    pub fn log_path(&self, step_id: &str) -> PathBuf {
        match self.lock().current_path.as_ref() {
            Some(root) => log_path_in(root, step_id),
            None => PathBuf::new(),
        }
    }

    /// `.pipeline-manager/done/<step id>.done` (F3: mismos mapeos de
    /// nombre); empty path when no project is open.
    pub fn done_path(&self, step_id: &str) -> PathBuf {
        match self.lock().current_path.as_ref() {
            Some(root) => done_path_in(root, step_id),
            None => PathBuf::new(),
        }
    }

    /// F2 guard for the engine preflight: the step's managed artifact paths
    /// (log + done) must be contained in the project and symlink-free, or
    /// the run is refused with this clear error before anything starts.
    pub fn ensure_step_artifacts_safe(&self, step_id: &str) -> Result<(), StoreError> {
        let data = self.lock();
        let root = data.current_path.as_ref().ok_or(StoreError::NoProject)?;
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
        let Some(root) = data.current_path.as_ref() else {
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
        let Some(root) = data.current_path.as_ref() else {
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
        let Some(root) = data.current_path.as_ref() else {
            return String::new();
        };
        match resolve_managed_path(root, &log_rel(step_id)) {
            Ok(path) => fs::read_to_string(path).unwrap_or_default(),
            Err(_) => String::new(),
        }
    }
}
