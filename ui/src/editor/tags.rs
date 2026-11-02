// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Plan de tags del Markdown Mode (ISSUE-015) — lógica pura, sin GTK.
//!
//! Traduce los tokens de [`opm_parser::highlight`] (spans en BYTES del
//! fuente) a rangos en CARÁCTERES unicode sobre el texto del `TextBuffer`,
//! junto con el estilo de cada `gtk::TextTag` a partir de una
//! [`opm_theme::Palette`]. Los tests de este módulo son puros: no
//! inicializan GTK ni crean widgets.
//!
//! Además, los lexers de [`crate::syntax`] (YAML/JSON/shell/FASTA) emiten
//! [`SyntaxRole`]s que aquí se mapean a la MISMA tabla de tags (§11 A3:
//! estilos propiedad de la UI, sin tocar `opm-parser`).

use crate::syntax::SyntaxRole;
use opm_parser::{Highlight, HighlightKind};
use opm_theme::Palette;

/// Rango de un token en CARÁCTERES unicode sobre el texto del buffer
/// (índices 0-based de `char`, como `gtk::TextIter::offset()`).
///
/// `end_char` es exclusivo: `[start_char, end_char)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRange {
    pub start_char: usize,
    pub end_char: usize,
    pub kind: HighlightKind,
}

/// Convierte highlights (spans en BYTES del fuente) a rangos en CARÁCTERES
/// sobre `text` (índices de char unicode), recortando a límites de char
/// válidos y descartando rangos vacíos; salida en orden de aparición.
///
/// Un span que empieza o termina a mitad de un char multibyte se recorta al
/// char contiguo completo (inicio hacia arriba, final hacia abajo); si tras el
/// recorte no queda ningún char entero, el rango se descarta sin más (nunca
/// panic ni slicing fuera de fronteras). A igual posición de inicio se
/// conserva el orden del parser (orden estable).
pub fn tag_plan(text: &str, highlights: &[Highlight]) -> Vec<TagRange> {
    let mut plan: Vec<TagRange> = Vec::with_capacity(highlights.len());
    for highlight in highlights {
        let Some((start_char, end_char)) = char_range(
            text,
            highlight.span.start as usize,
            highlight.span.end as usize,
        ) else {
            continue;
        };
        plan.push(TagRange {
            start_char,
            end_char,
            kind: highlight.kind,
        });
    }
    // Orden de aparición con empates estables (conservan el orden del parser).
    plan.sort_by_key(|range| range.start_char);
    plan
}

/// Convierte un rango `[start_byte, end_byte)` del fuente a `[start_char,
/// end_char)` sobre `text` (índices de char unicode), con las mismas reglas
/// de recorte que [`tag_plan`]; `None` si no queda ningún char entero.
pub(crate) fn char_range(text: &str, start_byte: usize, end_byte: usize) -> Option<(usize, usize)> {
    let start_byte = ceil_char_boundary(text, start_byte);
    let end_byte = floor_char_boundary(text, end_byte);
    if start_byte >= end_byte {
        return None;
    }
    let start_char = text[..start_byte].chars().count();
    let end_char = start_char + text[start_byte..end_byte].chars().count();
    Some((start_char, end_char))
}

/// Frontera de char ≥ `index` (para inicios de span).
fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// Frontera de char ≤ `index` (para finales de span, exclusivos).
fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Estilo de un `gtk::TextTag` para un tipo de token bajo una paleta (§23).
///
/// Los valores de color salen de la [`Palette`] (fuente única de color §23):
/// campos estructurales (`fg`, `muted`, `quote`, `rule`, `code_bg`) y la
/// paleta semántica/física vía [`Palette::color`]. Los colores de tema
/// completo de los widgets los fija el integrador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagStyle {
    pub fg: Option<&'static str>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub monospace: bool,
    pub bg: Option<&'static str>,
    /// Escala tipográfica en tanto por ciento (`100` = normal); `None` deja
    /// la escala del widget (headings de Visual, ISSUE-030).
    pub scale: Option<u32>,
    /// Sangrado izquierdo en px (`None` = sin sangrado; citas de Visual,
    /// ISSUE-031).
    pub left_margin: Option<u32>,
}

/// Estilo por tipo de token (§23): Heading/Strong en negrita, Emphasis en
/// cursiva, Code/CodeBlock monoespaciados con fondo de código.
pub fn tag_style(kind: HighlightKind, palette: &Palette) -> TagStyle {
    let mut style = TagStyle {
        fg: Some(palette.fg),
        bold: false,
        italic: false,
        underline: false,
        monospace: false,
        bg: None,
        scale: None,
        left_margin: None,
    };
    match kind {
        HighlightKind::FrontMatter => style.fg = Some(palette.muted),
        HighlightKind::Heading => style.bold = true,
        HighlightKind::Quote => {
            style.fg = Some(palette.quote);
            style.italic = true;
        }
        HighlightKind::List => style.fg = palette.color("blue"),
        HighlightKind::CodeBlock | HighlightKind::Code => {
            style.monospace = true;
            style.bg = Some(palette.code_bg);
        }
        HighlightKind::Table => style.fg = palette.color("green"),
        HighlightKind::Rule => style.fg = Some(palette.rule),
        HighlightKind::Emphasis => style.italic = true,
        HighlightKind::Strong => style.bold = true,
        HighlightKind::Strikethrough => style.fg = Some(palette.muted),
        HighlightKind::Link | HighlightKind::WikiLink => style.fg = palette.color("info"),
        HighlightKind::Image => style.fg = palette.color("purple"),
        HighlightKind::Math | HighlightKind::MathBlock => style.fg = palette.color("purple"),
        HighlightKind::Color | HighlightKind::Substitution => style.fg = palette.color("orange"),
        HighlightKind::Citation => style.fg = palette.color("green"),
        HighlightKind::Callout => style.fg = palette.color("note"),
    }
    style
}

/// Nombre estable del `gtk::TextTag` por tipo de token.
pub fn tag_name(kind: HighlightKind) -> &'static str {
    match kind {
        HighlightKind::FrontMatter => "opm-front-matter",
        HighlightKind::Heading => "opm-heading",
        HighlightKind::Quote => "opm-quote",
        HighlightKind::List => "opm-list",
        HighlightKind::CodeBlock => "opm-code-block",
        HighlightKind::Code => "opm-code",
        HighlightKind::Table => "opm-table",
        HighlightKind::Rule => "opm-rule",
        HighlightKind::Emphasis => "opm-emphasis",
        HighlightKind::Strong => "opm-strong",
        HighlightKind::Strikethrough => "opm-strikethrough",
        HighlightKind::Link => "opm-link",
        HighlightKind::Image => "opm-image",
        HighlightKind::Math => "opm-math",
        HighlightKind::MathBlock => "opm-math-block",
        HighlightKind::Color => "opm-color",
        HighlightKind::Citation => "opm-citation",
        HighlightKind::WikiLink => "opm-wiki-link",
        HighlightKind::Substitution => "opm-substitution",
        HighlightKind::Callout => "opm-callout",
    }
}

/// Todos los tipos, para crear la TextTagTable completa.
pub fn all_kinds() -> &'static [HighlightKind] {
    &[
        HighlightKind::FrontMatter,
        HighlightKind::Heading,
        HighlightKind::Quote,
        HighlightKind::List,
        HighlightKind::CodeBlock,
        HighlightKind::Code,
        HighlightKind::Table,
        HighlightKind::Rule,
        HighlightKind::Emphasis,
        HighlightKind::Strong,
        HighlightKind::Strikethrough,
        HighlightKind::Link,
        HighlightKind::Image,
        HighlightKind::Math,
        HighlightKind::MathBlock,
        HighlightKind::Color,
        HighlightKind::Citation,
        HighlightKind::WikiLink,
        HighlightKind::Substitution,
        HighlightKind::Callout,
    ]
}

/// Todos los roles de syntax (YAML/JSON/shell/FASTA), para crear sus
/// `gtk::TextTag` junto a las de [`all_kinds`] en la tabla completa.
pub fn all_syn_roles() -> &'static [SyntaxRole] {
    &[
        SyntaxRole::Key,
        SyntaxRole::String,
        SyntaxRole::Number,
        SyntaxRole::Comment,
        SyntaxRole::Keyword,
        SyntaxRole::Punct,
        SyntaxRole::Header,
        SyntaxRole::Sequence,
        SyntaxRole::Anchor,
        SyntaxRole::Marker,
        SyntaxRole::Variable,
        SyntaxRole::Command,
    ]
}

/// Nombre estable del `gtk::TextTag` para un rol de los lexers de
/// [`crate::syntax`] (§11 A3: estilos propiedad de la UI).
pub fn syn_tag_name(role: SyntaxRole) -> &'static str {
    match role {
        SyntaxRole::Key => "opm-syn-key",
        SyntaxRole::String => "opm-syn-string",
        SyntaxRole::Number => "opm-syn-number",
        SyntaxRole::Comment => "opm-syn-comment",
        SyntaxRole::Keyword => "opm-syn-keyword",
        SyntaxRole::Punct => "opm-syn-punct",
        SyntaxRole::Header => "opm-syn-header",
        SyntaxRole::Sequence => "opm-syn-sequence",
        SyntaxRole::Anchor => "opm-syn-anchor",
        SyntaxRole::Marker => "opm-syn-marker",
        SyntaxRole::Variable => "opm-syn-variable",
        SyntaxRole::Command => "opm-syn-command",
    }
}

/// Estilo de un rol de syntax (§23): mismos campos estructurales que
/// [`tag_style`], colores desde la paleta (fuente única de color).
pub fn syn_tag_style(role: SyntaxRole, palette: &Palette) -> TagStyle {
    let mut style = TagStyle {
        fg: Some(palette.fg),
        bold: false,
        italic: false,
        underline: false,
        monospace: false,
        bg: None,
        scale: None,
        left_margin: None,
    };
    match role {
        SyntaxRole::Key => {
            style.fg = palette.color("blue");
            style.bold = true;
        }
        SyntaxRole::String => style.fg = palette.color("green"),
        SyntaxRole::Number => style.fg = palette.color("purple"),
        SyntaxRole::Comment => {
            style.fg = Some(palette.muted);
            style.italic = true;
        }
        SyntaxRole::Keyword => {
            style.fg = palette.color("orange");
            style.bold = true;
        }
        SyntaxRole::Punct => style.fg = Some(palette.muted),
        SyntaxRole::Header => {
            style.fg = palette.color("info");
            style.bold = true;
        }
        // La tira de secuencia FASTA: texto plano de la paleta (destaca la
        // cabecera coloreada sin saturar la línea).
        SyntaxRole::Sequence => style.fg = Some(palette.fg),
        SyntaxRole::Anchor => style.fg = palette.color("note"),
        SyntaxRole::Marker => style.fg = palette.color("yellow"),
        SyntaxRole::Variable => style.fg = palette.color("red"),
        SyntaxRole::Command => {
            style.fg = palette.color("accent");
            style.bold = true;
        }
    }
    style
}

#[cfg(test)]
mod tests {
    use opm_document::Span;
    use opm_parser::{Highlight, HighlightKind};
    use opm_theme::Palette;

    use super::{
        all_kinds, all_syn_roles, syn_tag_name, syn_tag_style, tag_name, tag_plan, tag_style,
        TagRange,
    };

    fn token(start: u32, end: u32, kind: HighlightKind) -> Highlight {
        Highlight {
            span: Span { start, end },
            kind,
        }
    }

    /// 1. Bytes → chars con `ñ` (2 bytes), `👍` (4 bytes) y `日本` (3+3).
    /// Texto: "ñ👍日本" = 12 bytes, 4 chars.
    #[test]
    fn tag_plan_bytes_a_chars_unicode() {
        let text = "ñ👍日本";
        assert_eq!(text.len(), 12);
        assert_eq!(text.chars().count(), 4);

        let plan = tag_plan(
            text,
            &[
                token(0, 2, HighlightKind::Heading), // "ñ"        → chars [0,1)
                token(2, 6, HighlightKind::Strong),  // "👍"       → chars [1,2)
                token(6, 12, HighlightKind::Code),   // "日本"     → chars [2,4)
                token(2, 9, HighlightKind::Link),    // "👍日"     → chars [1,3)
            ],
        );

        assert_eq!(
            plan,
            vec![
                TagRange {
                    start_char: 0,
                    end_char: 1,
                    kind: HighlightKind::Heading
                },
                TagRange {
                    start_char: 1,
                    end_char: 2,
                    kind: HighlightKind::Strong
                },
                TagRange {
                    start_char: 1,
                    end_char: 3,
                    kind: HighlightKind::Link
                },
                TagRange {
                    start_char: 2,
                    end_char: 4,
                    kind: HighlightKind::Code
                },
            ]
        );
    }

    /// 2. Span a mitad de char multibyte o vacío → descartado sin panic
    /// (recorte a límites de char).
    #[test]
    fn tag_plan_recorta_a_limites_de_char() {
        let text = "a日本"; // 1 + 3 + 3 = 7 bytes, 3 chars.

        // Vacíos e invertidos: descartados.
        assert!(tag_plan(text, &[token(3, 3, HighlightKind::Code)]).is_empty());
        assert!(tag_plan(text, &[token(5, 2, HighlightKind::Code)]).is_empty());
        // Entero dentro de un char multibyte: tras recortar no queda nada.
        assert!(tag_plan(text, &[token(1, 2, HighlightKind::Code)]).is_empty());
        assert!(tag_plan(text, &[token(4, 5, HighlightKind::Code)]).is_empty());
        // Fuera del texto: recortado a vacío, sin panic.
        assert!(tag_plan(text, &[token(7, 9, HighlightKind::Code)]).is_empty());
        assert!(tag_plan("", &[token(0, 1, HighlightKind::Code)]).is_empty());

        // Final a mitad de char (byte 3 dentro de `日`): se recorta al char
        // completo previo → [0,1).
        assert_eq!(
            tag_plan(text, &[token(0, 3, HighlightKind::Code)]),
            vec![TagRange {
                start_char: 0,
                end_char: 1,
                kind: HighlightKind::Code
            }]
        );
        // Inicio a mitad de char (byte 2 dentro de `日`): se recorta al char
        // completo siguiente → [2,3).
        assert_eq!(
            tag_plan(text, &[token(2, 7, HighlightKind::Code)]),
            vec![TagRange {
                start_char: 2,
                end_char: 3,
                kind: HighlightKind::Code
            }]
        );
    }

    /// 3. Orden estable: salida en orden de aparición; empates conservan el
    /// orden del parser.
    #[test]
    fn tag_plan_orden_estable() {
        let text = "ñ👍日本"; // 4 chars.

        // Fuera de orden: la salida sigue el orden de aparición.
        let plan = tag_plan(
            text,
            &[
                token(6, 12, HighlightKind::Code),   // chars [2,4)
                token(0, 2, HighlightKind::Heading), // chars [0,1)
            ],
        );
        assert_eq!(
            plan,
            vec![
                TagRange {
                    start_char: 0,
                    end_char: 1,
                    kind: HighlightKind::Heading
                },
                TagRange {
                    start_char: 2,
                    end_char: 4,
                    kind: HighlightKind::Code
                },
            ]
        );

        // Empate de inicio (contenedor + token interno): estable = orden del
        // parser, contenedor primero.
        let plan = tag_plan(
            text,
            &[
                token(0, 12, HighlightKind::Quote),
                token(0, 6, HighlightKind::Strong),
                token(2, 6, HighlightKind::Emphasis),
            ],
        );
        assert_eq!(
            plan.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![
                HighlightKind::Quote,
                HighlightKind::Strong,
                HighlightKind::Emphasis
            ]
        );
        assert_eq!(
            plan.iter()
                .map(|r| (r.start_char, r.end_char))
                .collect::<Vec<_>>(),
            vec![(0, 4), (0, 2), (1, 2)]
        );
    }

    /// 4. `tag_style`: Heading/Strong en negrita, Emphasis en cursiva,
    /// Code/CodeBlock monoespaciados con `bg`; colores distintos entre
    /// `Palette::LIGHT` y `Palette::DARK`.
    #[test]
    fn tag_style_tipos_y_paletas() {
        for palette in [&Palette::LIGHT, &Palette::DARK] {
            assert!(tag_style(HighlightKind::Heading, palette).bold);
            let strong = tag_style(HighlightKind::Strong, palette);
            assert!(strong.bold && !strong.italic);

            let emphasis = tag_style(HighlightKind::Emphasis, palette);
            assert!(emphasis.italic && !emphasis.bold);

            for kind in [HighlightKind::Code, HighlightKind::CodeBlock] {
                let code = tag_style(kind, palette);
                assert!(code.monospace);
                assert_eq!(code.bg, Some(palette.code_bg));
            }
        }

        for &kind in all_kinds() {
            assert_ne!(
                tag_style(kind, &Palette::LIGHT),
                tag_style(kind, &Palette::DARK),
                "el estilo de {kind:?} debe depender de la paleta"
            );
        }
    }

    #[test]
    fn nombres_de_tag_estables_y_unicos() {
        assert_eq!(all_kinds().len(), 20);
        let mut names: Vec<&str> = all_kinds().iter().map(|&kind| tag_name(kind)).collect();
        names.sort_unstable();
        let unique = {
            let before = names.len();
            names.dedup();
            names.len() == before
        };
        assert!(unique, "los nombres de tag deben ser únicos por tipo");
    }

    /// Roles de syntax: nombres estables y únicos, sin solaparse con los de
    /// Markdown, y estilos que dependen de la paleta (§23).
    #[test]
    fn nombres_y_estilos_de_syntax_roles() {
        let mut names: Vec<&str> = all_syn_roles()
            .iter()
            .map(|&role| syn_tag_name(role))
            .collect();
        names.sort_unstable();
        let unique = {
            let before = names.len();
            names.dedup();
            names.len() == before
        };
        assert!(unique, "los nombres de tag de syntax deben ser únicos");

        let markdown: Vec<&str> = all_kinds().iter().map(|&kind| tag_name(kind)).collect();
        for name in &names {
            assert!(
                !markdown.contains(name),
                "{name} no debe colisionar con un tag de Markdown"
            );
        }

        for &role in all_syn_roles() {
            let style = syn_tag_style(role, &Palette::LIGHT);
            let dark = syn_tag_style(role, &Palette::DARK);
            assert_ne!(
                style, dark,
                "el estilo de {role:?} debe depender de la paleta"
            );
        }
    }
}
