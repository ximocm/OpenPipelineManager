//! Reducer puro del estado de ejecución y formateo de blockers (§6, A10).
//!
//! El motor (`opm-exec`) es la fuente de verdad de los estados; este módulo
//! traduce sus [`ExecEvent`] a la vista (estados por paso, lote de logs y la
//! marca «hay que refrescar el dominio») sin tocar GTK, para poder testear
//! las transiciones sin display.

use std::collections::BTreeMap;

use opm_exec::ExecEvent;
use opm_pipeline::{IssueSeverity, ValidationIssue};
use opm_store::StepStatus;

/// Vista reducida de la ejecución para el tick del event pump (§11 A12).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunModel {
    /// ¿Hay una ejecución en curso? (Arranca en `false`; lo confirma cada
    /// tick con `ExecutionManager::status().running`.)
    pub running: bool,
    /// Estado por paso visto en los eventos.
    pub statuses: BTreeMap<String, StepStatus>,
    /// Mensaje (`state.message`) por paso visto en los eventos.
    pub messages: BTreeMap<String, String>,
    /// Lote de líneas de log acumulado desde el último `take_logs`.
    pub logs: Vec<(String, String)>,
    /// Algún evento exige refrescar canvas/paneles (inicio, fin, skip).
    pub changed: bool,
    /// `ExecutionFinished` consumido desde el último [`Self::take_finished`]:
    /// la reconciliación final del log se ancla AQUÍ, no a la observación
    /// independiente de `status().running` (P2-5 — la cola sigue viva hasta
    /// que se consume, y sus `StepLog` no pueden caer sobre un log ya
    /// sustituido).
    finished: bool,
}

impl RunModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Aplica un evento del motor (§6: `step.started` → running,
    /// `step.finished`/`step.skipped` → estado + refresco,
    /// `execution.finished` → fin).
    pub fn apply(&mut self, event: ExecEvent) {
        match event {
            ExecEvent::StepStarted { step_id, command: _ } => {
                self.running = true;
                self.statuses.insert(step_id, StepStatus::Running);
                self.changed = true;
            }
            ExecEvent::StepLog { step_id, line } => {
                self.logs.push((step_id, line));
            }
            ExecEvent::StepFinished { step_id, state } => {
                self.statuses.insert(step_id.clone(), state.status);
                self.messages.insert(step_id, state.message);
                self.changed = true;
            }
            ExecEvent::StepSkipped { step_id, message } => {
                // El motor pone `error` + «Dependency is not complete: …»
                // antes de emitir el evento (execution.py port).
                self.statuses.insert(step_id.clone(), StepStatus::Error);
                self.messages.insert(step_id, message);
                self.changed = true;
            }
            ExecEvent::ExecutionFinished => {
                self.running = false;
                self.finished = true;
                self.changed = true;
            }
        }
    }

    /// Sustrae el lote de líneas acumulado (anexo en vivo agrupado, §11 A12).
    pub fn take_logs(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.logs)
    }

    /// Sustrae y limpia la marca de refresco.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    /// Sustrae la marca de `ExecutionFinished` consumido desde la última
    /// llamada (P2-5: dispara la reconciliación FINAL del log).
    pub fn take_finished(&mut self) -> bool {
        std::mem::take(&mut self.finished)
    }
}

/// `execution.py:format_validation_issue` — la ubicación del issue es
/// `step_id[.field]` (o `pipeline` sin paso).
pub fn format_blocker(issue: &ValidationIssue) -> String {
    let mut location = issue.step_id.clone().unwrap_or_else(|| "pipeline".to_owned());
    if let Some(field) = &issue.field {
        location.push('.');
        location.push_str(field);
    }
    if issue.message.is_empty() {
        location
    } else {
        format!("{location}: {}", issue.message)
    }
}

/// `client.ts:formatErrorDetail` — `message` + los blockers unidos con `"; "`.
///
/// Es el texto que ve el usuario cuando el motor devuelve
/// [`opm_exec::ExecError::Blocked`] (§6, A10: el preflight es autoritativo).
pub fn format_blocked(message: &str, blockers: &[ValidationIssue]) -> String {
    if blockers.is_empty() {
        return message.to_owned();
    }
    let joined = blockers
        .iter()
        .map(format_blocker)
        .collect::<Vec<_>>()
        .join("; ");
    format!("{message} {joined}")
}

/// Contadores del toolbar: `(blockers, warnings)` (§6).
pub fn count_severities(issues: &[ValidationIssue]) -> (usize, usize) {
    let mut blockers = 0;
    let mut warnings = 0;
    for issue in issues {
        match issue.severity {
            IssueSeverity::Blocker => blockers += 1,
            IssueSeverity::Warning => warnings += 1,
        }
    }
    (blockers, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(severity: IssueSeverity, step: Option<&str>, field: Option<&str>, message: &str) -> ValidationIssue {
        ValidationIssue {
            severity,
            message: message.to_owned(),
            step_id: step.map(str::to_owned),
            field: field.map(str::to_owned),
        }
    }

    fn state(status: StepStatus, message: &str) -> opm_store::StepState {
        opm_store::StepState {
            status,
            message: message.to_owned(),
            ..opm_store::StepState::default()
        }
    }

    /// Transiciones de estado a partir de los eventos del motor (§6).
    #[test]
    fn reducidor_transiciones_desde_eventos() {
        let mut model = RunModel::new();
        assert!(!model.running);

        model.apply(ExecEvent::StepStarted {
            step_id: "prepare".into(),
            command: "echo hi".into(),
        });
        assert!(model.running);
        assert_eq!(model.statuses["prepare"], StepStatus::Running);
        assert!(model.take_changed());
        // P2-5: solo `ExecutionFinished` arma la reconciliación final del log.
        assert!(!model.take_finished());

        model.apply(ExecEvent::StepLog {
            step_id: "prepare".into(),
            line: "hi".into(),
        });
        // El log NO marca refresco (solo anexo en vivo).
        assert!(!model.take_changed());
        assert_eq!(model.take_logs(), vec![("prepare".into(), "hi".into())]);
        assert!(model.logs.is_empty());

        model.apply(ExecEvent::StepFinished {
            step_id: "prepare".into(),
            state: state(StepStatus::Ok, ""),
        });
        assert_eq!(model.statuses["prepare"], StepStatus::Ok);
        assert!(model.take_changed());

        model.apply(ExecEvent::ExecutionFinished);
        assert!(!model.running);
        assert!(model.take_changed());
        // P2-5: la marca de fin se arma al consumir el evento y se consume
        // una sola vez (el tick la usa y no se repite).
        assert!(model.take_finished());
        assert!(!model.take_finished());
    }

    /// `StepSkipped` deja el paso en `error` con el mensaje del motor.
    #[test]
    fn reducidor_skip_con_mensaje_de_dependencia() {
        let mut model = RunModel::new();
        model.apply(ExecEvent::StepSkipped {
            step_id: "summarize".into(),
            message: "Dependency is not complete: analyze".into(),
        });
        assert_eq!(model.statuses["summarize"], StepStatus::Error);
        assert_eq!(
            model.messages["summarize"],
            "Dependency is not complete: analyze"
        );
        assert!(model.changed);
    }

    /// `execution.py:format_validation_issue` / `client.ts:formatErrorDetail`
    /// exactos: `message + " " + "step.field: msg; …"` (§6).
    #[test]
    fn bloqueo_formateado_como_el_spa() {
        let blockers = vec![
            issue(
                IssueSeverity::Blocker,
                Some("analyze"),
                Some("input_file"),
                "required value is missing",
            ),
            issue(IssueSeverity::Blocker, None, None, "pipeline needs steps"),
            issue(IssueSeverity::Blocker, Some("prep"), None, ""),
        ];
        assert_eq!(
            format_blocked("Execution blocked by 3 validation blockers", &blockers),
            "Execution blocked by 3 validation blockers \
             analyze.input_file: required value is missing; pipeline: pipeline needs steps; prep"
        );
        // Sin blockers: solo el mensaje.
        assert_eq!(format_blocked("Execution blocked by 0 validation blockers", &[]), "Execution blocked by 0 validation blockers");
    }

    /// Contadores del toolbar (§6).
    #[test]
    fn contadores_por_severidad() {
        let issues = vec![
            issue(IssueSeverity::Blocker, Some("a"), None, "b"),
            issue(IssueSeverity::Warning, Some("a"), None, "w"),
            issue(IssueSeverity::Blocker, None, None, "b2"),
            issue(IssueSeverity::Warning, Some("c"), None, "w2"),
        ];
        assert_eq!(count_severities(&issues), (2, 2));
        assert_eq!(count_severities(&[]), (0, 0));
    }
}
