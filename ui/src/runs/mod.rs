//! Ejecución visible (§6 `runs/`): panel de logs, resumen de validación,
//! contadores del toolbar y el reducer puro de eventos del motor (§11 A12).
//!
//! - [`logs::LogsPanel`]: cabecera `Logs: <stepId>` con botón Stop siempre
//!   visible, contenido monoespaciado y anexo en vivo (`append_line`).
//! - [`summary::Counters`]: contadores `N blockers` / `M warnings` del
//!   toolbar con el panel-resumen de validación en un popover (lista
//!   `gtk::ListBox` con [`crate::list_scroll::follow_selection`]).
//! - [`state::RunModel`]: reducer puro de [`opm_exec::ExecEvent`] (transiciones
//!   de estado, lote de logs, marca de refresco) + el formateo de blockers
//!   `message + "step.field: msg; …"` del SPA (`client.ts:74-86`).
//! - [`pump`]: drenado acotado (≤256 eventos / ~8 ms) para el tick de 250 ms
//!   de `main.rs` (§11 A12).

pub mod logs;
pub mod pump;
pub mod state;
pub mod summary;

pub use logs::LogsPanel;
pub use pump::drain;
pub use state::{count_severities, format_blocker, format_blocked, RunModel};
pub use summary::Counters;
