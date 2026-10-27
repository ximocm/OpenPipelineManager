// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Pestañas de documentos (§37).
//!
//! [`manager::TabManager`] es el modelo: mantiene los tabs (cada uno con su
//! [`opm_document::Editor`] sobre el buffer único, §6.1, y su historial
//! Undo/Redo propio, §8.5), el tab activo y el ciclo de vida de ficheros
//! (§12) sobre un [`opm_fs::LocalFs`] interno. [`bar::TabBar`] es la
//! barra GTK de pestañas: oculta con un solo documento abierto y con el
//! indicador `●` de cambios sin guardar junto al nombre (§37).

mod bar;
mod manager;
pub mod scope;

pub use bar::TabBar;
pub use manager::TabManager;
pub use scope::{OpenTarget, WindowScope};
