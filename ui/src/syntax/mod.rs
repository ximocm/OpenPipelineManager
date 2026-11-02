//! Lexers puros de resaltado para los formatos de OPM (§11 A3): YAML, JSON,
//! shell y FASTA. Cada lexer emite spans en BYTES `[start, end)` sobre el
//! fuente con el [`SyntaxRole`] semántico, sin dependencias ni estado GTK;
//! [`crate::editor::tags`] los convierte a rangos de chars y a `TextTag`s.
//!
//! El despacho por extensión vive en [`kind_for_suffix`] y lo usa
//! [`crate::editor::pane`] (§5): `.md`/`.markdown` va al parser de
//! `opm_parser`, estos formatos a sus lexers y cualquier otro sin resaltado.

pub mod fasta;
pub mod json;
pub mod shell;
pub mod yaml;

/// Lenguaje con lexer propio, por extensión del fichero (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxKind {
    Yaml,
    Json,
    Shell,
    Fasta,
}

/// Rol semántico de un token emitido por los lexers.
///
/// Es el TIPO de la tabla de tags de la UI (§11 A3): cada rol tiene su
/// `gtk::TextTag` y su estilo de paleta en [`crate::editor::tags`]
/// (`syn_tag_name` / `syn_tag_style` / `all_syn_roles`), sin tocar
/// `opm-parser`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxRole {
    /// Clave de mapeo (YAML) o clave de objeto (JSON).
    Key,
    /// Cadena de texto entrecomillada o escalar de valor.
    String,
    /// Literal numérico.
    Number,
    /// Comentario (`#`, `;`).
    Comment,
    /// Palabra clave del lenguaje (`true`/`false`/`null`, builtins shell).
    Keyword,
    /// Puntuación estructural (`{}[]:,` y operadores shell).
    Punct,
    /// Línea de cabecera FASTA (`>`).
    Header,
    /// Tira de secuencia FASTA.
    Sequence,
    /// Ancla, alias o etiqueta YAML (`&`, `*`, `!`).
    Anchor,
    /// Marcador de lista YAML (`-`) o indicador de escalar de bloque
    /// (`|`, `>`).
    Marker,
    /// Variable shell (`$VAR`, `${VAR}`, `$1`, `$?`…).
    Variable,
    /// Palabra en posición de comando shell.
    Command,
}

/// Resuelve el lenguaje por sufijo de extensión en minúsculas (sin punto),
/// como el que [`crate::editor::pane`] extrae del path del documento.
pub fn kind_for_suffix(suffix: &str) -> Option<SyntaxKind> {
    match suffix {
        "yml" | "yaml" => Some(SyntaxKind::Yaml),
        "json" => Some(SyntaxKind::Json),
        "sh" | "bash" => Some(SyntaxKind::Shell),
        "fa" | "fasta" | "fna" | "ffn" | "faa" => Some(SyntaxKind::Fasta),
        _ => None,
    }
}

/// Resalta `text` con el lexer de `kind`. Los spans salen en orden de
/// aparición, sin solapes (los estilos se aplican por tag, §11 A3).
pub fn highlight(kind: SyntaxKind, text: &str) -> Vec<(usize, usize, SyntaxRole)> {
    match kind {
        SyntaxKind::Yaml => yaml::highlight(text),
        SyntaxKind::Json => json::highlight(text),
        SyntaxKind::Shell => shell::highlight(text),
        SyntaxKind::Fasta => fasta::highlight(text),
    }
}

/// Líneas de `text` con sus offsets de byte: `(inicio, línea sin `\n` ni
/// `\r`)`. Iterar por líneas con offset evita reposicionar índices y hace
/// que el CRLF pise el `\r` sin marcarlo.
pub(crate) fn lines_with_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for raw in text.split_inclusive('\n') {
        let content = raw
            .strip_suffix('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s))
            .unwrap_or(raw);
        out.push((offset, content));
        offset += raw.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §5: despacho por sufiltro; los formatos sin lexer no resaltan.
    #[test]
    fn dispatch_por_sufijo() {
        for (suffix, kind) in [
            ("yml", SyntaxKind::Yaml),
            ("yaml", SyntaxKind::Yaml),
            ("json", SyntaxKind::Json),
            ("sh", SyntaxKind::Shell),
            ("bash", SyntaxKind::Shell),
            ("fa", SyntaxKind::Fasta),
            ("fasta", SyntaxKind::Fasta),
            ("fna", SyntaxKind::Fasta),
            ("ffn", SyntaxKind::Fasta),
            ("faa", SyntaxKind::Fasta),
        ] {
            assert_eq!(kind_for_suffix(suffix), Some(kind), "{suffix}");
        }
        for suffix in ["md", "txt", "png", "csv", ""] {
            assert_eq!(kind_for_suffix(suffix), None, "{suffix}");
        }
    }

    /// El despacho de `pane` normaliza a minúsculas antes de consultar.
    #[test]
    fn sufijos_en_minusculas() {
        assert_eq!(kind_for_suffix("YML"), None, "el caller normaliza");
    }
}
