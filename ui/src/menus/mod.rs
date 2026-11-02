// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Menús, atajos y diálogos de OPM (ISSUE-020).
//!
//! - [`actions`]: comandos explícitos y observables (§3.7), barra de menú §36
//!   (File, Edit, View, Help) y atajos de teclado §39.
//! - [`dialogs`]: diálogos GTK4 asíncronos — abrir/guardar (§12.4) y
//!   confirmaciones explícitas ante descartes (§3.7) y cambios externos (§12.7).
//!
//! El integrador (`main.rs`) declara `mod menus;` y consume esta API:
//! [`build_menubar`] + [`install_actions`] para la ventana, [`Commands`] como
//! receptor de comandos y [`dialogs`] para los diálogos.

pub mod actions;
pub mod dialogs;

pub use actions::{build_menubar, install_actions, Commands, ACCELERATORS};
