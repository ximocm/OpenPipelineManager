// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Tokens de syntax highlighting (ISSUE-015).
//!
//! Estilo CodeMirror: rangos de fuente con un tipo semántico. Los tokens salen
//! de la misma gramática que la AST (parser único, §4.2): bloques desde la
//! estructura parseada y constructos inline desde el escáner §7.4. Lo que no
//! es sintaxis (texto plano) no produce token: el editor lo pinta como texto.

use opm_document::{Block, BlockKind, Span};

/// Tipo semántico de un token de highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    /// Front matter YAML (§21).
    FrontMatter,
    Heading,
    Quote,
    List,
    CodeBlock,
    /// Code span (§7.4.1: código primero).
    Code,
    Table,
    Rule,
    Emphasis,
    Strong,
    Strikethrough,
    Link,
    Image,
    /// Math inline `$...$` (§24).
    Math,
    /// Ecuación de bloque `$$...$$` (§24).
    MathBlock,
    /// Color inline `{...}` (§23).
    Color,
    /// Cita `[@...]` (§25.2).
    Citation,
    /// Wiki link `[[...]]` (§19.3).
    WikiLink,
    /// Sustitución `{{...}}` (§7.4.2).
    Substitution,
    /// Callout `:::` (§22).
    Callout,
}

/// Token de highlighting con su rango en el fuente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    pub span: Span,
    pub kind: HighlightKind,
}

/// Tokens de highlighting de los bloques (recursivo sobre citas, callouts y
/// listas).
pub(crate) fn block_highlights(blocks: &[Block], out: &mut Vec<Highlight>) {
    for block in blocks {
        let kind = match &block.kind {
            BlockKind::Heading { .. } => Some(HighlightKind::Heading),
            BlockKind::Quote { blocks } => {
                out.push(Highlight {
                    span: block.span,
                    kind: HighlightKind::Quote,
                });
                block_highlights(blocks, out);
                None
            }
            BlockKind::List { items, .. } => {
                out.push(Highlight {
                    span: block.span,
                    kind: HighlightKind::List,
                });
                for item in items {
                    block_highlights(&item.blocks, out);
                }
                None
            }
            BlockKind::CodeBlock { .. } => Some(HighlightKind::CodeBlock),
            BlockKind::Table { .. } => Some(HighlightKind::Table),
            BlockKind::MathBlock { .. } => Some(HighlightKind::MathBlock),
            BlockKind::Callout { blocks, .. } => {
                out.push(Highlight {
                    span: block.span,
                    kind: HighlightKind::Callout,
                });
                block_highlights(blocks, out);
                None
            }
            BlockKind::Rule => Some(HighlightKind::Rule),
            // Los párrafos no son sintaxis; su contenido lleva tokens propios.
            BlockKind::Paragraph { .. } | BlockKind::FootnoteDefinition { .. } | BlockKind::Bibliography { .. } => None,
        };
        if let Some(kind) = kind {
            out.push(Highlight {
                span: block.span,
                kind,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight_with;
    use crate::ParseOptions;

    fn tokens(source: &str) -> Vec<Highlight> {
        highlight_with(source, &Default::default())
    }

    fn text_of<'a>(source: &'a str, token: &Highlight) -> &'a str {
        &source[token.span.start as usize..token.span.end as usize]
    }

    #[test]
    fn bloques_e_inlines_basicos() {
        let source = "# Título **negrita**\n";
        let tokens = tokens(source);
        let kinds: Vec<HighlightKind> = tokens.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![HighlightKind::Heading, HighlightKind::Strong]
        );
        assert_eq!(text_of(source, &tokens[0]), "# Título **negrita**\n");
        assert_eq!(text_of(source, &tokens[1]), "**negrita**");
    }

    #[test]
    fn codigo_primero_no_produce_tokens_de_extension() {
        // §7.4.1: dentro de code spans no se interpreta ninguna extensión.
        let source = "`{danger: x} {{toc}} [@a]`\n";
        let tokens = tokens(source);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, HighlightKind::Code);
    }

    #[test]
    fn extensiones_inline_con_spans_exactos() {
        // wiki_links activado para que `[[w]]` sea sintaxis (§19.3).
        let source = "{danger: x} {{ref:fig:a}} [@s, p. 1] $E$ [[w]]\n";
        let options = ParseOptions {
            wiki_links: true,
            ..Default::default()
        };
        let tokens = highlight_with(source, &options);
        let kinds: Vec<HighlightKind> = tokens.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                HighlightKind::Color,
                HighlightKind::Substitution,
                HighlightKind::Citation,
                HighlightKind::Math,
                HighlightKind::WikiLink,
            ]
        );
        for (token, expected) in tokens.iter().zip([
            "{danger: x}",
            "{{ref:fig:a}}",
            "[@s, p. 1]",
            "$E$",
            "[[w]]",
        ]) {
            assert_eq!(text_of(source, token), expected);
        }
    }

    #[test]
    fn bloques_extendidos_y_front_matter() {
        let source = "---\ntitle: x\n---\n\n::: warning\nx\n:::\n\n$$\ny\n$$\n";
        let tokens = tokens(source);
        let kinds: Vec<HighlightKind> = tokens.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                HighlightKind::FrontMatter,
                HighlightKind::Callout,
                HighlightKind::MathBlock,
            ]
        );
    }

    #[test]
    fn tokens_ordenados_y_no_solapados_en_inicio() {
        let source = "# a\n\n> b\n\n- c\n\n---\n";
        let tokens = tokens(source);
        assert!(tokens.windows(2).all(|w| w[0].span.start <= w[1].span.start));
        let kinds: Vec<HighlightKind> = tokens.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                HighlightKind::Heading,
                HighlightKind::Quote,
                HighlightKind::List,
                HighlightKind::Rule,
            ]
        );
    }
}
