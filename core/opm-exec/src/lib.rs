//! opm-exec — OPM execution engine.
//!
//! Port of `backend/app/services/execution.py` over the shared
//! [`ProjectStore`] (§11 A6: no store lock is held while a child process runs
//! or waits — the engine only calls short store methods between steps and log
//! Preflight (`ExecError::Blocked` over the dependency closure,
//! topological subset, project/shell checks) happens before any thread
//! starts; `stop` terminates the current process and cancels the rest.
//!
//! Known parity limitation (kept from Python `Popen.terminate()`): stop
//! signals only the IMMEDIATE child — no process groups or jobs are
//! implemented, so descendants the child spawned may outlive it.

mod command;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use opm_fs::resolve_pipeline_path;
use opm_pipeline::{IssueSeverity, PipelineStep, ValidationIssue, Value};
use opm_store::ProjectStore;

pub use opm_store::{StepState, StepStatus};

/// Engine events (replaces the Python SSE transport with an in-process
/// channel; §12 event pump in the UI drains them in bounded batches).
#[derive(Clone, Debug, PartialEq)]
pub enum ExecEvent {
    StepStarted { step_id: String, command: String },
    StepLog { step_id: String, line: String },
    StepFinished { step_id: String, state: StepState },
    StepSkipped { step_id: String, message: String },
    ExecutionFinished,
}

/// `models/state.py:ExecutionStatus`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExecStatus {
    pub running: bool,
    pub current_step_id: Option<String>,
    pub stop_requested: bool,
}

/// Engine failures. `Blocked` carries the formatted message
/// (`"Execution blocked by N validation blocker(s)"`, §6 UI appends the
/// `step.field: msg; …` summary) plus the raw issues.
#[derive(Debug)]
pub enum ExecError {
    AlreadyRunning,
    Blocked {
        message: String,
        blockers: Vec<ValidationIssue>,
    },
    Cycle,
    UnknownStep(String),
    MissingPlaceholders(Vec<String>),
    Io(std::io::Error),
    /// Windows: git-bash not found — the single-argv quoting guarantee is
    /// POSIX-only (§11 A8), so nothing may start.
    NoPosixShell,
    /// F1: a placeholder sits in a shell context OPM cannot substitute
    /// safely (comment, `$()`/`${...}`/backticks or heredoc body). The
    /// validation layer blocks the same commands before a run starts; this
    /// variant guards direct `build_command` callers.
    UnsupportedContext { placeholder: String },
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::AlreadyRunning => write!(f, "Execution is already running"),
            ExecError::Blocked { message, .. } => write!(f, "{message}"),
            ExecError::Cycle => write!(f, "Cycle detected while scheduling"),
            ExecError::UnknownStep(id) => write!(f, "Unknown step: {id}"),
            ExecError::MissingPlaceholders(keys) => {
                write!(f, "Missing placeholder values: {}", keys.join(", "))
            }
            ExecError::Io(err) => write!(f, "{err}"),
            ExecError::NoPosixShell => write!(
                f,
                "No POSIX shell available: install Git for Windows (bash)"
            ),
            ExecError::UnsupportedContext { placeholder } => write!(
                f,
                "placeholder {placeholder} sits in a shell context OPM cannot substitute safely (comment, $() or heredoc)"
            ),
        }
    }
}

impl std::error::Error for ExecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ExecError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ExecError {
    fn from(err: std::io::Error) -> Self {
        ExecError::Io(err)
    }
}

/// `ExecutionBlockedError` message base (the UI appends the formatted
/// blockers, §6).
fn blocked_message(count: usize) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("Execution blocked by {count} validation blocker{plural}")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn io_err(message: impl std::fmt::Display) -> ExecError {
    ExecError::Io(std::io::Error::new(
        std::io::ErrorKind::Other,
        message.to_string(),
    ))
}

/// Intervalo de sondeo del hijo durante el reap (F7): el candado del proceso
/// solo se sostiene durante el `try_wait` no bloqueante, nunca durante una
/// espera — `stop()` puede tomarlo entre sondeos y matar al hijo al instante.
const REAP_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// UTC ISO timestamp as `datetime.now(timezone.utc).isoformat()`
/// (`2026-10-27T10:11:12.131415+00:00`, fraction omitted at exact seconds).
fn utc_iso_now() -> String {
    let now = chrono::Utc::now();
    if now.timestamp_subsec_micros() == 0 {
        now.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        now.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// `Popen.returncode`: exit code, or `-signal` when killed by a signal
/// (Python reports `-15` for SIGTERM).
#[cfg(unix)]
fn exit_code_of(status: &ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        code
    } else if let Some(signal) = status.signal() {
        -signal
    } else {
        -1
    }
}

#[cfg(not(unix))]
fn exit_code_of(status: &ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

/// Python `Popen.terminate()` = SIGTERM on POSIX, TerminateProcess on Windows.
#[cfg(unix)]
fn terminate_child(child: &mut Child) {
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate_child(child: &mut Child) {
    let _ = child.kill();
}

/// `ExecutionManager._dependency_closure` (set semantics).
fn dependency_closure(store: &ProjectStore, ids: &[String]) -> HashSet<String> {
    let steps = store.steps();
    let map: HashMap<&str, &PipelineStep> = steps.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut seen = HashSet::new();
    let mut stack: Vec<String> = ids.to_vec();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(step) = map.get(id.as_str()) {
            for dependency in &step.dependencies {
                stack.push(dependency.clone());
            }
        }
    }
    seen
}

/// `ExecutionManager._topological_subset`: deps first, post-order DFS;
/// cycle → [`ExecError::Cycle`], unknown step → [`ExecError::UnknownStep`].
///
/// F8: DFS iterativo con una única pila mutable (la pila ES el camino) y
/// estado de visitación lineal — una cadena de miles de pasos desde un
/// fichero sin validar no desborda la pila ni memoriza cuadráticamente.
fn topological_subset(store: &ProjectStore, ids: &[String]) -> Result<Vec<String>, ExecError> {
    let steps = store.steps();
    let map: HashMap<&str, &PipelineStep> = steps.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut ordered = Vec::new();
    let mut temporary: HashSet<String> = HashSet::new();
    let mut permanent: HashSet<String> = HashSet::new();
    for id in ids {
        // Entrada de la raíz (los checks replican `visit` en orden).
        if permanent.contains(id.as_str()) {
            continue;
        }
        if temporary.contains(id.as_str()) {
            return Err(ExecError::Cycle);
        }
        if !map.contains_key(id.as_str()) {
            return Err(ExecError::UnknownStep(id.clone()));
        }
        let mut stack: Vec<(&str, usize)> = vec![(id.as_str(), 0)];
        temporary.insert(id.clone());
        loop {
            let Some(&(current, next)) = stack.last() else {
                break;
            };
            // Solo entran nodos presentes en el mapa (comprobado al apilar).
            let step = map.get(current).copied().expect("node checked at push");
            if next >= step.dependencies.len() {
                stack.pop();
                temporary.remove(current);
                permanent.insert(current.to_owned());
                ordered.push(current.to_owned());
                continue;
            }
            stack.last_mut().expect("node on the path").1 = next + 1;
            let dependency = step.dependencies[next].as_str();
            if permanent.contains(dependency) {
                continue;
            }
            if temporary.contains(dependency) {
                return Err(ExecError::Cycle);
            }
            if !map.contains_key(dependency) {
                return Err(ExecError::UnknownStep(dependency.to_owned()));
            }
            stack.push((dependency, 0));
            temporary.insert(dependency.to_owned());
        }
    }
    Ok(ordered)
}

/// State shared with the daemon thread.
struct ExecShared {
    events: Mutex<VecDeque<ExecEvent>>,
    condvar: Condvar,
    running: AtomicBool,
    stop_requested: AtomicBool,
    current_step: Mutex<Option<String>>,
    process: Mutex<Option<Child>>,
}

fn push_event(shared: &ExecShared, event: ExecEvent) {
    lock(&shared.events).push_back(event);
    shared.condvar.notify_one();
}

/// Runs a shared [`ProjectStore`] pipeline: `run_steps` / `run_selected` on a
/// daemon thread, `stop`, `status` and the `next_event` channel.
pub struct ExecutionManager {
    store: Arc<ProjectStore>,
    shared: Arc<ExecShared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Single-flights preflight + spawn (Python's `self.lock`).
    preflight: Mutex<()>,
}

impl ExecutionManager {
    pub fn new(store: Arc<ProjectStore>) -> Self {
        Self {
            store,
            shared: Arc::new(ExecShared {
                events: Mutex::new(VecDeque::new()),
                condvar: Condvar::new(),
                running: AtomicBool::new(false),
                stop_requested: AtomicBool::new(false),
                current_step: Mutex::new(None),
                process: Mutex::new(None),
            }),
            thread: Mutex::new(None),
            preflight: Mutex::new(()),
        }
    }

    /// Effective-values substitution + environment wrapper for one step.
    pub fn build_command(
        &self,
        step: &PipelineStep,
        params: &BTreeMap<String, Value>,
    ) -> Result<String, ExecError> {
        command::build_command(step, params)
    }

    /// Explicit step ids: already running → `AlreadyRunning`; blockers in the
    /// dependency closure → `Blocked` BEFORE any thread starts; topological
    /// subset order; then the daemon thread.
    pub fn run_steps(&self, ids: Vec<String>) -> Result<(), ExecError> {
        let _preflight = lock(&self.preflight);
        if self.shared.running.load(Ordering::SeqCst) {
            return Err(ExecError::AlreadyRunning);
        }
        let issues = self.store.validate();
        let closure = dependency_closure(&self.store, &ids);
        let blockers: Vec<ValidationIssue> = issues
            .into_iter()
            .filter(|issue| {
                issue.severity == IssueSeverity::Blocker
                    && (issue.step_id.is_none()
                        || issue
                            .step_id
                            .as_ref()
                            .is_some_and(|step_id| closure.contains(step_id)))
            })
            .collect();
        if !blockers.is_empty() {
            return Err(ExecError::Blocked {
                message: blocked_message(blockers.len()),
                blockers,
            });
        }
        let ordered = topological_subset(&self.store, &ids)?;
        // Project + shell must be available before anything starts (§11 A8).
        self.store.project_root().map_err(|err| io_err(err))?;
        // F2: los destinos gestionados (log/done) de cada paso del run
        // deben estar contenidos y sin symlinks — error claro antes de que
        // arranque ningún hilo (la escritura también los rechaza por si acaso).
        for id in &ordered {
            self.store.ensure_step_artifacts_safe(id).map_err(io_err)?;
        }
        command::check_posix_shell()?;
        if let Some(previous) = lock(&self.thread).take() {
            let _ = previous.join();
        }
        self.shared.stop_requested.store(false, Ordering::SeqCst);
        self.shared.running.store(true, Ordering::SeqCst);
        let store = Arc::clone(&self.store);
        let shared = Arc::clone(&self.shared);
        let handle = std::thread::Builder::new()
            .name("opm-exec".to_string())
            .spawn(move || run_ordered_steps(store, shared, ordered))
            .map_err(|err| {
                self.shared.running.store(false, Ordering::SeqCst);
                ExecError::Io(err)
            })?;
        *lock(&self.thread) = Some(handle);
        Ok(())
    }

    /// Explicitly selected steps → else every step whose status is not `ok`.
    pub fn run_selected(&self) -> Result<(), ExecError> {
        let steps = self.store.steps();
        let mut selected: Vec<String> = steps
            .iter()
            .filter(|step| self.store.state_of(&step.id).selected)
            .map(|step| step.id.clone())
            .collect();
        if selected.is_empty() {
            selected = steps
                .iter()
                .filter(|step| self.store.state_of(&step.id).status != StepStatus::Ok)
                .map(|step| step.id.clone())
                .collect();
        }
        self.run_steps(selected)
    }

    /// Terminate the current process and cancel the remaining steps.
    pub fn stop(&self) {
        self.shared.stop_requested.store(true, Ordering::SeqCst);
        if let Some(child) = lock(&self.shared.process).as_mut() {
            terminate_child(child);
        }
    }

    /// Escalation after [`Self::stop`] was not enough: SIGKILL (POSIX) /
    /// `TerminateProcess` (Windows) on the IMMEDIATE child, same
    /// child-only limitation as `stop` (no process groups — descendants the
    /// child spawned may outlive it, see the module docs). Marks the run as
    /// user-stopped so the step still finishes as `cancelled`.
    pub fn force_stop(&self) {
        self.shared.stop_requested.store(true, Ordering::SeqCst);
        if let Some(child) = lock(&self.shared.process).as_mut() {
            let _ = child.kill();
        }
    }

    pub fn status(&self) -> ExecStatus {
        ExecStatus {
            running: self.shared.running.load(Ordering::SeqCst),
            current_step_id: lock(&self.shared.current_step).clone(),
            stop_requested: self.shared.stop_requested.load(Ordering::SeqCst),
        }
    }

    /// Next queued event; `timeout == Duration::ZERO` never blocks (§11 A12).
    pub fn next_event(&self, timeout: Duration) -> Option<ExecEvent> {
        let mut events = lock(&self.shared.events);
        if events.is_empty() && !timeout.is_zero() {
            let (guard, _) = self
                .shared
                .condvar
                .wait_timeout(events, timeout)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            events = guard;
        }
        events.pop_front()
    }
}

/// `ExecutionManager._run_ordered_steps` (the daemon thread body).
fn run_ordered_steps(store: Arc<ProjectStore>, shared: Arc<ExecShared>, ordered: Vec<String>) {
    for step_id in &ordered {
        if shared.stop_requested.load(Ordering::SeqCst) {
            break;
        }
        let Some(step) = store.step(step_id) else {
            break;
        };
        let incomplete: Vec<String> = step
            .dependencies
            .iter()
            .filter(|dependency| !store.is_step_ok(dependency))
            .cloned()
            .collect();
        if !incomplete.is_empty() {
            let message = format!("Dependency is not complete: {}", incomplete.join(", "));
            let mut state = store.state_of(step_id);
            state.status = StepStatus::Error;
            state.message = message.clone();
            store.set_step_state(step_id, state);
            push_event(
                &shared,
                ExecEvent::StepSkipped {
                    step_id: step_id.clone(),
                    message,
                },
            );
            continue;
        }
        if let Err(_err) = run_one_step(&store, &shared, step_id, &step) {
            // Python: an exception in the step aborts the loop (the `finally`
            // block below still emits `ExecutionFinished`).
            break;
        }
    }
    *lock(&shared.current_step) = None;
    *lock(&shared.process) = None;
    shared.running.store(false, Ordering::SeqCst);
    push_event(&shared, ExecEvent::ExecutionFinished);
}

/// `ExecutionManager._run_one_step`.
fn run_one_step(
    store: &Arc<ProjectStore>,
    shared: &Arc<ExecShared>,
    step_id: &str,
    step: &PipelineStep,
) -> Result<(), ExecError> {
    *lock(&shared.current_step) = Some(step_id.to_string());
    let params = store.params_for(step_id);
    let command_text = command::build_command(step, &params)?;
    let root = store.project_root().map_err(|err| io_err(err))?;
    let cwd =
        resolve_pipeline_path(&root, &step.working_directory, &root).map_err(|err| io_err(err))?;

    let mut state = store.state_of(step_id);
    state.status = StepStatus::Running;
    state.exit_code = None;
    state.message = String::new();
    store.set_step_state(step_id, state.clone());
    push_event(
        shared,
        ExecEvent::StepStarted {
            step_id: step_id.to_string(),
            command: command_text.clone(),
        },
    );
    store.append_log(
        step_id,
        &format!("\n[{}] $ {}\n", utc_iso_now(), command_text),
    );

    let argv = command::shell_argv(&command_text)?;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *lock(&shared.process) = Some(child);
    // `stop()` may have raced the spawn: honor the flag before reading.
    if shared.stop_requested.load(Ordering::SeqCst) {
        if let Some(child) = lock(&shared.process).as_mut() {
            terminate_child(child);
        }
    }

    if let Some(stdout) = stdout {
        let mut reader = BufReader::new(stdout);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            let read = reader.read_until(b'\n', &mut buffer)?;
            if read == 0 {
                break;
            }
            let mut text = String::from_utf8_lossy(&buffer).into_owned();
            if text.ends_with("\r\n") {
                text.truncate(text.len() - 2);
                text.push('\n');
            }
            store.append_log(step_id, &text);
            push_event(
                shared,
                ExecEvent::StepLog {
                    step_id: step_id.to_string(),
                    line: text.trim_end_matches('\n').to_string(),
                },
            );
            if shared.stop_requested.load(Ordering::SeqCst) {
                let mut guard = lock(&shared.process);
                let Some(child) = guard.as_mut() else {
                    break;
                };
                match child.try_wait() {
                    Ok(None) => {
                        terminate_child(child);
                        drop(guard);
                        break;
                    }
                    Ok(Some(_)) => {} // already exited: drain to EOF like Python
                    Err(_) => break,
                }
            }
        }
    }

    // F7: el mutex del proceso NUNCA se mantiene durante un `wait()`
    // bloqueante — un hijo que haya cerrado su salida capturada
    // (`exec >/dev/null 2>&1; sleep 30`) dejaría a `stop()` esperando el
    // candado hasta que el comando termine. Se sondea `try_wait` con
    // candados cortos; entre sondeos el candado está libre para `stop()`.
    let status = loop {
        let mut guard = lock(&shared.process);
        let Some(child) = guard.as_mut() else {
            return Err(io_err("child process handle missing"));
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                *guard = None;
                break status;
            }
            Ok(None) => {
                drop(guard);
                std::thread::sleep(REAP_POLL_INTERVAL);
            }
            Err(source) => return Err(source.into()),
        }
    };
    // Pre-merge stderr writes (shell/profile output before `exec 2>&1`) are
    // still captured in the pipe: drain them into the log/events.
    if let Some(mut stderr_pipe) = stderr {
        let mut bytes = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut bytes);
        let text = String::from_utf8_lossy(&bytes);
        if !text.is_empty() {
            for line in text.split_inclusive('\n') {
                store.append_log(step_id, line);
                push_event(
                    shared,
                    ExecEvent::StepLog {
                        step_id: step_id.to_string(),
                        line: line.trim_end_matches('\n').to_string(),
                    },
                );
            }
        }
    }

    let code = exit_code_of(&status);
    state.exit_code = Some(code);
    state.last_run_at = Some(utc_iso_now());
    if shared.stop_requested.load(Ordering::SeqCst) {
        state.status = StepStatus::Cancelled;
        state.message = "Execution cancelled by user".to_string();
    } else if code == 0 {
        store.write_done_marker(step_id, &utc_iso_now());
        if store.is_step_ok(step_id) {
            state.status = StepStatus::Ok;
        } else {
            state.status = StepStatus::Error;
            state.message = "Command succeeded but expected outputs are missing".to_string();
        }
    } else {
        state.status = StepStatus::Error;
        state.message = format!("Command exited with {code}");
    }
    store.set_step_state(step_id, state.clone());
    let _ = store.validate_and_save();
    push_event(
        shared,
        ExecEvent::StepFinished {
            step_id: step_id.to_string(),
            state,
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::utc_iso_now;

    /// `datetime.now(timezone.utc).isoformat()` shape:
    /// `2026-10-27T10:11:12.131415+00:00` (fraction omitted at exact seconds,
    /// exactly 6 digits otherwise).
    #[test]
    fn utc_timestamp_matches_python_isoformat() {
        let stamp = utc_iso_now();
        assert!(stamp.contains('T'), "{stamp}");
        assert!(stamp.ends_with("+00:00"), "{stamp}");
        let head = &stamp[..19];
        assert_eq!(&head[4..5], "-", "{stamp}");
        assert_eq!(&head[10..11], "T", "{stamp}");
        assert_eq!(&head[13..14], ":", "{stamp}");
        match stamp[19..].strip_suffix("+00:00").expect("utc suffix") {
            "" => {}
            fraction => {
                let digits = fraction.strip_prefix('.').expect("fraction starts with a dot");
                assert_eq!(digits.len(), 6, "{fraction}");
                assert!(digits.bytes().all(|byte| byte.is_ascii_digit()));
            }
        }
    }

    // ----------------------------------------- long chains (F8, iteratividad)

    use super::{ExecError, ProjectStore, topological_subset};
    use std::path::PathBuf;
    use std::sync::Arc;

    const CHAIN: usize = 10_000;

    /// Proyecto importado con una cadena lineal de [`CHAIN`] pasos
    /// (`s_i` depende de `s_{i+1}`); opcionalmente el último cierra el
    /// ciclo contra `tail_dep`.
    fn chain_fixture(tag: &str, tail_dep: Option<&str>) -> (PathBuf, Arc<ProjectStore>, Vec<String>) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let parent =
            std::env::temp_dir().join(format!("opm-exec-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&parent).expect("temp parent");
        let store = Arc::new(ProjectStore::new());
        store.create_project(&parent, "proj").expect("scaffold");
        let root = store.project_root().expect("project open");
        let mut steps = String::new();
        for i in 0..CHAIN {
            if i > 0 {
                steps.push(',');
            }
            steps.push_str(&format!(
                r#"{{"id": "s{i}", "name": "S{i}", "command": "true""#
            ));
            if i + 1 < CHAIN {
                steps.push_str(&format!(r#", "dependencies": ["s{}"]"#, i + 1));
            } else if let Some(dep) = tail_dep {
                steps.push_str(&format!(r#", "dependencies": ["{dep}"]"#));
            }
            steps.push('}');
        }
        let pipeline = root.join("pipeline.json");
        std::fs::write(&pipeline, format!("{{\"steps\": [{steps}]}}")).expect("chain json");
        store.import_pipeline(&pipeline).expect("chain imports");
        let ids: Vec<String> = (0..CHAIN).map(|i| format!("s{i}")).collect();
        (parent, store, ids)
    }

    /// F8: la programación de una cadena de 10.000 pasos (fichero sin
    /// validar) no desborda la pila — el recorrido es un DFS iterativo.
    #[test]
    fn topological_subset_orders_a_chain_of_10000_steps() {
        let (parent, store, ids) = chain_fixture("chain", None);
        let ordered = topological_subset(&store, &ids).expect("acyclic chain");
        assert_eq!(ordered.len(), CHAIN);
        // Dependencias primero: la cola de la cadena sale antes.
        assert_eq!(ordered.first().map(String::as_str), Some("s9999"));
        assert_eq!(ordered.last().map(String::as_str), Some("s0"));
        let _ = std::fs::remove_dir_all(&parent);
    }

    /// F8: la misma cadena con el ciclo cerrado en la cola → `Cycle`.
    #[test]
    fn topological_subset_rejects_a_tail_cycle_on_a_long_chain() {
        let (parent, store, ids) = chain_fixture("chain-cycle", Some("s0"));
        match topological_subset(&store, &ids) {
            Err(ExecError::Cycle) => {}
            other => panic!("expected Cycle, got {:?}", other.map(|ordered| ordered.len())),
        }
        let _ = std::fs::remove_dir_all(&parent);
    }
}
