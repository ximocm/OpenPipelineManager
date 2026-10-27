// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Módulo `editor` del binario GTK4 `opm` (ISSUE-015 / ISSUE-020).
//!
//! Markdown Mode (TextView monoespaciado con syntax highlighting) y modo
//! Visual (§8.2, ISSUE-030: sintaxis oculta + estilos semánticos) sobre el
//! buffer Markdown único (§6.1), como planes de tags excluyentes del mismo
//! `TextBuffer`. Ver [`pane`] para el puente TextView ↔ modelo y la
//! semántica §8.5/§8.6.

pub mod mode;
pub mod pane;
pub mod tags;
pub mod visual;

pub use mode::EditMode;
pub use pane::{EditSink, EditorPane};
