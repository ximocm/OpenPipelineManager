// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
use std::path::{Path, PathBuf};

/// Formato de salto de línea del buffer (§12.6).
///
/// LF permanece LF y CRLF permanece CRLF; los archivos nuevos usan LF. El
/// contenido se almacena byte a byte como se leyó (§6.2), por lo que el formato
/// original se preserva al guardar sin normalización.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

impl LineEnding {
    /// Detecta el formato dominante: CRLF si el texto contiene algún `\r\n`.
    pub fn detect(text: &str) -> Self {
        if text.contains("\r\n") {
            Self::Crlf
        } else {
            Self::Lf
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

/// Buffer Markdown único (§6.1): fuente de verdad del documento.
///
/// No existen dos versiones independientes del documento: Markdown y Visual
/// comparten este buffer y la AST derivada de `content()`.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    path: Option<PathBuf>,
    content: String,
    line_ending: LineEnding,
    modified: bool,
}

impl Document {
    /// Documento nuevo, vacío y sin path asignado (§12.1, §12.6: LF).
    pub fn new() -> Self {
        Self {
            path: None,
            content: String::new(),
            line_ending: LineEnding::Lf,
            modified: false,
        }
    }

    /// Construye el buffer con contenido ya validado como UTF-8 (§12.5),
    /// preservándolo byte a byte (§6.2).
    pub fn from_text(path: Option<PathBuf>, content: String) -> Self {
        let line_ending = LineEnding::detect(&content);
        Self {
            path,
            content,
            line_ending,
            modified: false,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Asigna el path del archivo (Save As, §12.4).
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path);
    }

    /// Retira el path asignado sin tocar el buffer: la eliminación del
    /// archivo (§14.9) deja el documento vivo como «Sin título N» (§3.7).
    pub fn clear_path(&mut self) {
        self.path = None;
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    /// Sustituye el contenido del buffer. Marca el documento como modificado
    /// solo si el contenido cambia.
    pub fn replace_content(&mut self, content: &str) {
        if self.content != content {
            self.content.clear();
            self.content.push_str(content);
            self.modified = true;
        }
    }

    /// Sustituye el rango de bytes `[start, end)` por `text` (edición del
    /// buffer). Marca el documento como modificado.
    ///
    /// # Panics
    ///
    /// Si el rango está fuera del contenido o no cae en límites de carácter
    /// UTF-8. Los rangos válidos provienen de posiciones del editor.
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str) {
        self.content.replace_range(start..end, text);
        self.modified = true;
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// El buffer ya coincide con el archivo en disco (guardado explícito, §12.3).
    pub fn mark_saved(&mut self) {
        self.modified = false;
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detecta_line_endings() {
        assert_eq!(LineEnding::detect("a\nb\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\r\nb\r\n"), LineEnding::Crlf);
        // Mezcla: gana CRLF (se preserva el contenido tal cual, §6.2).
        assert_eq!(LineEnding::detect("a\r\nb\nc"), LineEnding::Crlf);
        assert_eq!(LineEnding::detect(""), LineEnding::Lf);
    }

    #[test]
    fn documento_nuevo_es_lf_sin_modificar() {
        let doc = Document::new();
        assert_eq!(doc.path(), None);
        assert_eq!(doc.content(), "");
        assert_eq!(doc.line_ending(), LineEnding::Lf);
        assert!(!doc.is_modified());
    }

    #[test]
    fn from_text_detecta_crlf_y_preserva_contenido() {
        let raw = "# Título\r\n\r\nCuerpo\r\n".to_string();
        let doc = Document::from_text(Some(PathBuf::from("a.md")), raw.clone());
        assert_eq!(doc.content(), raw);
        assert_eq!(doc.line_ending(), LineEnding::Crlf);
        assert_eq!(doc.path(), Some(Path::new("a.md")));
        assert!(!doc.is_modified());
    }

    #[test]
    fn replace_content_marca_modificado_solo_si_cambia() {
        let mut doc = Document::from_text(None, "hola".to_string());
        doc.replace_content("hola");
        assert!(!doc.is_modified());

        doc.replace_content("adiós");
        assert!(doc.is_modified());
        assert_eq!(doc.content(), "adiós");

        doc.mark_saved();
        assert!(!doc.is_modified());
    }

    #[test]
    fn save_as_asigna_path() {
        let mut doc = Document::new();
        doc.set_path(PathBuf::from("/tmp/nuevo.md"));
        assert_eq!(doc.path(), Some(Path::new("/tmp/nuevo.md")));
    }
}
