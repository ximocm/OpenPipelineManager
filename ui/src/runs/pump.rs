//! Drenado acotado de eventos del motor (§11 A12).
//!
//! El tick de 250 ms de `main.rs` llama a [`drain`]: recoge eventos con
//! `next_event(Duration::ZERO)` (nunca bloqueante) hasta el tope de lote
//! (≤256 eventos) o el presupuesto de tiempo (~8 ms) y los aplica a un
//! [`RunModel`]. Los logs se anexan agrupados al terminar el lote y el
//! refresco de dominio ocurre UNA vez por lote — y jamás recarga el editor
//! activo (solo widgets de dominio: canvas, paneles, contadores).

use std::time::{Duration, Instant};

use opm_exec::ExecutionManager;

use super::state::RunModel;

/// Tope de eventos por tick (§11 A12: lote acotado).
pub const MAX_EVENTS: usize = 256;
/// Presupuesto de tiempo por tick en milisegundos (§11 A12: ~8 ms).
pub const BATCH_BUDGET_MS: u64 = 8;

/// Drena eventos no bloqueantes de `exec` hacia `model` con el tope de
/// lote. Devuelve cuántos eventos se aplicaron.
pub fn drain(exec: &ExecutionManager, model: &mut RunModel) -> usize {
    let started = Instant::now();
    let budget = Duration::from_millis(BATCH_BUDGET_MS);
    let mut count = 0;
    while count < MAX_EVENTS && started.elapsed() < budget {
        match exec.next_event(Duration::ZERO) {
            Some(event) => {
                model.apply(event);
                count += 1;
            }
            None => break,
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Un motor recién creado no tiene eventos: el drenado es 0 y no
    /// bloquea (§11 A12: `next_event(ZERO)` nunca espera).
    #[test]
    fn drenado_vacio_es_inmediato() {
        let exec = ExecutionManager::new(Arc::new(opm_store::ProjectStore::new()));
        let mut model = RunModel::new();
        let started = Instant::now();
        let count = drain(&exec, &mut model);
        assert_eq!(count, 0);
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(model, RunModel::new());
    }
}
