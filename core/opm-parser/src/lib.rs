// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Parser Markdown de minmark (§7).
//!
//! Wrapper sobre pulldown-cmark (parser único, §4.2; versión benchmarkada en
//! ISSUE-010) que produce la AST compartida de [`opm_document`].
//!
//! Capas:
//! 1. `blocks`: front matter (§21), callouts `:::` (§22), math `$$` (§24) y
//!    posiciones de atributos (§7.4.3);
//! 2. `events`: estructura CommonMark + GFM (§7.1) de pulldown-cmark;
//! 3. `inlines`: gramática de extensiones inline (§7.4) sobre el fuente crudo.
//!
//! Reglas normativas:
//! - §7.2: el HTML embebido nunca se interpreta; se preserva como texto.
//! - §6.2: cada bloque conserva su [`Span`] en el fuente original y parsear
//!   nunca modifica el buffer (la AST es derivada, §6.1).

mod blocks;
mod events;
mod highlight;
mod inlines;
mod visual;

use opm_document::Ast;

pub use highlight::{Highlight, HighlightKind};
pub use visual::{CalloutTone, VisualPlan, VisualSpan, VisualStyle};

/// Opciones de parseo del dialecto (§7.3, §58.2 niveles de proyecto).
#[derive(Debug, Clone, Default)]
pub struct ParseOptions<'a> {
    /// Nombres de colores personalizados del proyecto (§23, §58.2).
    pub custom_colors: &'a [String],
    /// Tipos `:::` personalizados del proyecto (§22, §58.2).
    pub custom_callout_types: &'a [String],
    /// Wiki links activados (§19.3). Por defecto: desactivados.
    pub wiki_links: bool,
}

/// Parsea un documento Markdown completo a la AST compartida (§7).
pub fn parse(source: &str) -> Ast {
    parse_with(source, &ParseOptions::default())
}

/// Parsea con las extensiones configurables (§7.3).
pub fn parse_with(source: &str, options: &ParseOptions) -> Ast {
    blocks::parse_document(source, 0, options, true).0
}

/// Tokens de syntax highlighting estilo CodeMirror (ISSUE-015).
///
/// Rangos de fuente con tipo semántico, derivados de la misma gramática que
/// la AST (parser único, §4.2). El texto plano no produce token.
pub fn highlight(source: &str) -> Vec<Highlight> {
    highlight_with(source, &ParseOptions::default())
}

/// Highlighting con las extensiones configurables (§7.3).
pub fn highlight_with(source: &str, options: &ParseOptions) -> Vec<Highlight> {
    blocks::parse_document(source, 0, options, true).1
}

/// Plan visual del modo Visual (ISSUE-030, §8.2–§8.4): tramos de sintaxis
/// ocultable (§8.3) y de contenido semántico sobre el fuente, más los
/// elementos (bloques) para revelar la sintaxis del elemento editado.
pub fn visual(source: &str) -> VisualPlan {
    visual_with(source, &ParseOptions::default())
}

/// Plan visual con las extensiones configurables (§7.3).
pub fn visual_with(source: &str, options: &ParseOptions) -> VisualPlan {
    visual::build(source, options)
}

/// Destinos navegables: headings y IDs explícitos de figuras/tablas/ecuaciones.
/// El nivel cero distingue un objeto de un heading sin crear otro índice.
pub fn destination_anchors(source: &str) -> Vec<opm_document::Anchor> {
    use opm_document::{Anchor, Attributes, Block, BlockKind, Inline};
    fn add(attrs: &Option<Attributes>, line: u32, out: &mut Vec<Anchor>) {
        if let Some(id) = attrs.as_ref().and_then(|a| a.id.as_ref()) {
            out.push(Anchor {
                id: id.clone(),
                level: 0,
                text: attrs
                    .as_ref()
                    .and_then(|a| a.caption.clone())
                    .unwrap_or_else(|| id.clone()),
                line,
            });
        }
    }
    fn inlines(items: &[Inline], line: u32, out: &mut Vec<Anchor>) {
        for item in items {
            match item {
                Inline::Image { attrs, .. } => add(attrs, line, out),
                Inline::Emphasis(c)
                | Inline::Strong(c)
                | Inline::Strikethrough(c)
                | Inline::Link { content: c, .. } => inlines(c, line, out),
                _ => {}
            }
        }
    }
    fn walk(blocks: &[Block], source: &str, out: &mut Vec<Anchor>) {
        for block in blocks {
            let line = source[..block.span.start as usize]
                .bytes()
                .filter(|&b| b == b'\n')
                .count() as u32
                + 1;
            match &block.kind {
                BlockKind::Paragraph { content } | BlockKind::Heading { content, .. } => {
                    inlines(content, line, out)
                }
                BlockKind::Table {
                    attrs,
                    header,
                    rows,
                    ..
                } => {
                    add(attrs, line, out);
                    for c in header.iter().chain(rows.iter().flatten()) {
                        inlines(c, line, out);
                    }
                }
                BlockKind::MathBlock { attrs, .. } => add(attrs, line, out),
                BlockKind::Quote { blocks } | BlockKind::Callout { blocks, .. } => {
                    walk(blocks, source, out)
                }
                BlockKind::List { items, .. } => {
                    for item in items {
                        walk(&item.blocks, source, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = opm_document::document_anchors(&opm_document::find_heading_nodes(source));
    walk(&parse(source).blocks, source, &mut out);
    out.sort_by_key(|a| a.line);
    out
}

#[cfg(test)]
mod intelligence_tests {
    #[test]
    fn destinos_de_objetos_y_outline_excluyen_codigo_y_comentarios() {
        let source = "<!--\n# Oculto\n-->\n\n````\n```\n# Tampoco\n````\n\nTítulo\n======\n\n![x](x){#fig:uno}\n\n| A |\n| - |\n| B |\n{#tbl:uno}\n\n$$\nx\n$$\n{#eq:uno}\n";
        let anchors = super::destination_anchors(source);
        assert_eq!(
            anchors.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["título", "fig:uno", "tbl:uno", "eq:uno"]
        );
        assert_eq!(anchors[0].level, 1);
        assert!(anchors[1..].iter().all(|a| a.level == 0));
    }
}
