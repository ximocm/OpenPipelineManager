// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Theme palette for Open Pipeline Manager: the light/dark [`Palette`] and
//! its CSS-variable generation, vendored from minmark's renderer. The UI
//! (GTK) reads the same values for widget styling; `TagStyle` and friends are
//! NOT part of this crate — they live in the editor subsystem.

mod palette;

pub use palette::{theme_css, Palette};
