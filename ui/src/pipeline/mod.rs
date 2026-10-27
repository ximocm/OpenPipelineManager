//! Vistas de pipeline (§6 `pipeline/`): lienzo del grafo, panel de detalle
//! del paso y diálogo de creación/edición.
//!
//! - [`canvas`]: `DrawingArea` con la transformada única, las tres clases de
//!   arista (A9) y los gestos; el modelo puro vive en `canvas_model` (mismo
//!   directorio) para poder testearlo sin display.
//! - [`step_panel`]: panel derecho (port de `StepPanel.tsx`).
//! - [`step_dialog`]: formulario compartido crear/editar paso (port de
//!   `CreateStepDialog.tsx`).

pub mod canvas;
mod canvas_model;
pub mod step_dialog;
pub mod step_panel;

pub use canvas::{CanvasCallbacks, PipelineCanvas};
pub use canvas_model::CanvasData;
pub use step_dialog::StepDraft;
pub use step_panel::{StepCallbacks, StepPanel};
