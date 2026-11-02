// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Modos de edición del panel (ISSUE-015 / ISSUE-020).
//!
//! Dos modos sobre el MISMO buffer Markdown (§6.1): `Markdown` (texto fuente,
//! editable) y `Visual` (representación semántica §8.2). Alternar modos NO
//! reinicia el historial Undo/Redo (§8.5) ni pierde el cursor (§8.6): son
//! estados del modelo, no de la vista.

/// Modo de edición activo del panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    /// Markdown Mode: el texto fuente sobre el TextBuffer (ISSUE-015).
    Markdown,
    /// Visual Mode: representación semántica §8.2 editable sobre el MISMO
    /// buffer (ISSUE-030): sintaxis oculta + estilos semánticos.
    Visual,
}

impl EditMode {
    /// Modo opuesto (conmutador de la barra de cabecera).
    pub fn toggled(self) -> Self {
        match self {
            EditMode::Markdown => EditMode::Visual,
            EditMode::Visual => EditMode::Markdown,
        }
    }

    /// Etiqueta de UI estable: "Markdown" / "Visual".
    pub fn label(self) -> &'static str {
        match self {
            EditMode::Markdown => "Markdown",
            EditMode::Visual => "Visual",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EditMode;

    #[test]
    fn toggled_es_involucion() {
        assert_eq!(EditMode::Markdown.toggled(), EditMode::Visual);
        assert_eq!(EditMode::Visual.toggled(), EditMode::Markdown);
        assert_eq!(EditMode::Markdown.toggled().toggled(), EditMode::Markdown);
        assert_eq!(EditMode::Visual.toggled().toggled(), EditMode::Visual);
    }

    #[test]
    fn etiquetas_estables() {
        assert_eq!(EditMode::Markdown.label(), "Markdown");
        assert_eq!(EditMode::Visual.label(), "Visual");
    }
}
