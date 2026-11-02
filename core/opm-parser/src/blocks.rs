// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Segmentación de bloques extendidos y post-proceso de atributos.
//!
//! - Front matter YAML (§21) al inicio del documento.
//! - Callouts `::: tipo "título"` (§22) y ecuaciones `$$ ... $$` (§24) como
//!   constructos delimitados por líneas; su contenido se parsea recursivamente.
//! - Atributos (§7.4.3) solo en sus tres posiciones: tras imagen inline, en
//!   línea propia tras tabla o ecuación de bloque, y al final de un heading.

use crate::events;
use crate::highlight::{block_highlights, Highlight, HighlightKind};
use crate::inlines::BUILTIN_CALLOUTS;
use crate::ParseOptions;
use opm_document::parse_attributes;
use opm_document::{Ast, Attributes, Block, BlockKind, Inline, Span};

/// Parsea un documento completo: segmentación + eventos + atributos.
///
/// Devuelve la AST y los tokens de highlighting (ISSUE-015), ambos con spans
/// en coordenadas del documento completo.
pub(crate) fn parse_document(
    source: &str,
    base: usize,
    options: &ParseOptions,
    top_level: bool,
) -> (Ast, Vec<Highlight>) {
    let (front_matter, body, body_base) = if top_level {
        split_front_matter(source, base)
    } else {
        (None, source, base)
    };

    let mut blocks = Vec::new();
    let mut tokens = Vec::new();
    if front_matter.is_some() {
        tokens.push(Highlight {
            span: Span {
                start: base as u32,
                end: body_base as u32,
            },
            kind: HighlightKind::FrontMatter,
        });
    }

    for segment in segment(body, body_base, options) {
        match segment {
            Segment::Raw { text, base } => {
                let (parsed, scanned) = events::parse_segment(text, base, options);
                blocks.extend(parsed);
                tokens.extend(scanned);
            }
            Segment::Callout {
                callout_type,
                title,
                inner,
                inner_base,
                span,
            } => {
                let (inner_ast, inner_tokens) = parse_document(inner, inner_base, options, false);
                tokens.extend(inner_tokens);
                blocks.push(Block {
                    span,
                    kind: BlockKind::Callout {
                        callout_type,
                        title,
                        blocks: inner_ast.blocks,
                    },
                });
            }
            Segment::Math { latex, span } => blocks.push(Block {
                span,
                kind: BlockKind::MathBlock { latex, attrs: None },
            }),
        }
    }

    attach_attributes(&mut blocks, source, base);
    block_highlights(&blocks, &mut tokens);
    tokens.sort_by_key(|t| (t.span.start, t.span.end));
    (
        Ast {
            blocks,
            front_matter,
        },
        tokens,
    )
}

/// Front matter YAML (§21): primera línea `---` y cierre `---`.
fn split_front_matter(source: &str, base: usize) -> (Option<String>, &str, usize) {
    let mut lines = source.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, source, base);
    };
    if first.trim() != "---" {
        return (None, source, base);
    }
    let mut offset = first.len();
    for line in lines {
        if line.trim() == "---" {
            let content = source[first.len()..offset].to_string();
            let body_start = offset + line.len();
            return (Some(content), &source[body_start..], base + body_start);
        }
        offset += line.len();
    }
    // Sin línea de cierre no es front matter: el `---` inicial es un Rule.
    (None, source, base)
}

enum Segment<'s> {
    Raw {
        text: &'s str,
        base: usize,
    },
    Callout {
        callout_type: String,
        title: Option<String>,
        inner: &'s str,
        inner_base: usize,
        span: Span,
    },
    Math {
        latex: String,
        span: Span,
    },
}

/// Divide el texto en segmentos crudos y constructos de bloque extendidos.
fn segment<'s>(text: &'s str, base: usize, options: &ParseOptions) -> Vec<Segment<'s>> {
    let lines: Vec<(&str, usize, usize)> = {
        let mut acc = Vec::new();
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            acc.push((line, offset, offset + line.len()));
            offset += line.len();
        }
        acc
    };
    // §7.4.1 «código primero»: dentro de un fence no se interpreta ningún
    // constructo de bloque extendido (`:::`, `$$`).
    let in_code = fenced_code_mask(&lines);

    let mut segments = Vec::new();
    let mut raw_start: Option<usize> = None;
    let mut idx = 0;

    while idx < lines.len() {
        let trimmed = lines[idx].0.trim();

        // Callout `::: tipo "título"` (§22) con anidamiento.
        if !in_code[idx] {
            if let Some((callout_type, title)) = parse_callout_open(trimmed, options) {
                let mut depth = 1;
                let mut close = None;
                for (j, (line, _, _)) in lines.iter().enumerate().skip(idx + 1) {
                    if in_code[j] {
                        continue;
                    }
                    let t = line.trim();
                    if parse_callout_open(t, options).is_some() {
                        depth += 1;
                    } else if t == ":::" {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(j);
                            break;
                        }
                    }
                }
                if let Some(close_idx) = close {
                    flush_raw(&mut raw_start, &mut segments, &lines, text, base, idx);
                    let inner_start = lines[idx].2;
                    let inner_end = lines[close_idx].1;
                    segments.push(Segment::Callout {
                        callout_type,
                        title,
                        inner: &text[inner_start..inner_end],
                        inner_base: base + inner_start,
                        span: Span {
                            start: (base + lines[idx].1) as u32,
                            end: (base + lines[close_idx].2) as u32,
                        },
                    });
                    idx = close_idx + 1;
                    continue;
                }
            }

            // Ecuación de bloque `$$ ... $$` (§24).
            if trimmed == "$$" {
                if let Some(close_idx) = lines[idx + 1..]
                    .iter()
                    .enumerate()
                    .find(|(offset, (line, _, _))| {
                        !in_code[idx + 1 + *offset] && line.trim() == "$$"
                    })
                    .map(|(offset, _)| idx + 1 + offset)
                {
                    flush_raw(&mut raw_start, &mut segments, &lines, text, base, idx);
                    let raw_latex = &text[lines[idx].2..lines[close_idx].1];
                    let latex = raw_latex.strip_suffix('\n').unwrap_or(raw_latex);
                    segments.push(Segment::Math {
                        latex: latex.to_string(),
                        span: Span {
                            start: (base + lines[idx].1) as u32,
                            end: (base + lines[close_idx].2) as u32,
                        },
                    });
                    idx = close_idx + 1;
                    continue;
                }
            }
        }

        // Línea normal: extiende la región cruda.
        if raw_start.is_none() {
            raw_start = Some(lines[idx].1);
        }
        idx += 1;
    }
    flush_raw(
        &mut raw_start,
        &mut segments,
        &lines,
        text,
        base,
        lines.len(),
    );
    segments
}

/// Máscara de líneas dentro de un fence de código (§7.4.1 «código primero»).
///
/// El opener y el closer forman parte del propio code block, así que también
/// quedan marcados. Un fence se abre con tres o más `` ` `` o `~` y se cierra
/// con al menos tantos del mismo carácter (CommonMark).
fn fenced_code_mask(lines: &[(&str, usize, usize)]) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    let mut fence: Option<(char, usize)> = None;
    for (i, (line, _, _)) in lines.iter().enumerate() {
        let trimmed = line.trim();
        match fence {
            Some((marker, len)) => {
                mask[i] = true;
                if closes_fence(trimmed, marker, len) {
                    fence = None;
                }
            }
            None => {
                if let Some(open) = opens_fence(trimmed) {
                    fence = Some(open);
                    mask[i] = true;
                }
            }
        }
    }
    mask
}

fn opens_fence(trimmed: &str) -> Option<(char, usize)> {
    let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = trimmed.chars().take_while(|c| *c == marker).count();
    if len < 3 {
        return None;
    }
    // El info string de un fence de backticks no puede contener backticks.
    if marker == '`' && trimmed[len..].contains('`') {
        return None;
    }
    Some((marker, len))
}

fn closes_fence(trimmed: &str, marker: char, len: usize) -> bool {
    let count = trimmed.chars().take_while(|c| *c == marker).count();
    count == trimmed.chars().count() && count >= len
}

fn flush_raw<'s>(
    raw_start: &mut Option<usize>,
    segments: &mut Vec<Segment<'s>>,
    lines: &[(&str, usize, usize)],
    text: &'s str,
    base: usize,
    boundary_line: usize,
) {
    if let Some(start) = raw_start.take() {
        let end = lines
            .get(boundary_line)
            .map(|(_, start, _)| *start)
            .unwrap_or(text.len());
        segments.push(Segment::Raw {
            text: &text[start..end],
            base: base + start,
        });
    }
}

/// Apertura de callout (§22): `::: tipo` o `::: tipo "Título"`.
fn parse_callout_open(trimmed: &str, options: &ParseOptions) -> Option<(String, Option<String>)> {
    let rest = trimmed.strip_prefix(":::")?.trim();
    if rest.is_empty() {
        return None; // `:::` aislado es la línea de cierre.
    }
    let name_len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
        .unwrap_or(rest.len());
    let name = &rest[..name_len];
    if name.is_empty()
        || !name.starts_with(|c: char| c.is_ascii_alphabetic())
        || !(BUILTIN_CALLOUTS.contains(&name)
            || options.custom_callout_types.iter().any(|t| t == name))
    {
        // §22 + vocabulario cerrado (§7.4.1): tipo sin definir no es sintaxis.
        return None;
    }
    let remainder = rest[name_len..].trim();
    let title = if remainder.is_empty() {
        None
    } else {
        let inner = remainder.strip_prefix('"')?.strip_suffix('"')?;
        if inner.is_empty() {
            return None;
        }
        Some(inner.to_string())
    };
    Some((name.to_string(), title))
}

// --- Post-proceso de atributos (§7.4.3) --------------------------------------

fn slice<'a>(source: &'a str, base: usize, span: Span) -> &'a str {
    let start = (span.start as usize).saturating_sub(base);
    let end = (span.end as usize).saturating_sub(base);
    source.get(start..end).unwrap_or("")
}

fn attach_attributes(blocks: &mut Vec<Block>, source: &str, base: usize) {
    let mut out: Vec<Block> = Vec::new();
    for mut block in std::mem::take(blocks) {
        // Posición 2: línea propia de atributos tras tabla o ecuación (§26.3,
        // §26.4). Se compara sobre el fuente crudo.
        if let BlockKind::Paragraph { content } = &block.kind {
            if matches!(content.as_slice(), [Inline::Text(_)]) {
                let raw = slice(source, base, block.span).trim();
                if let Some(attrs) = parse_attr_line(raw) {
                    let attached = match out.last_mut() {
                        Some(Block {
                            kind: BlockKind::Table { attrs: slot, .. },
                            ..
                        })
                        | Some(Block {
                            kind: BlockKind::MathBlock { attrs: slot, .. },
                            ..
                        }) if slot.is_none() => {
                            *slot = Some(attrs);
                            true
                        }
                        _ => false,
                    };
                    if attached {
                        continue;
                    }
                }
            }
        }
        decorate(&mut block, source, base);
        out.push(block);
    }
    *blocks = out;
}

/// Línea propia `{...}` con gramática de atributos no vacía.
fn parse_attr_line(raw: &str) -> Option<Attributes> {
    let inner = raw.strip_prefix('{')?.strip_suffix('}')?;
    let attrs = parse_attributes(inner)?;
    if attrs.is_empty() {
        return None;
    }
    Some(attrs)
}

/// Aplica atributos dentro de un bloque: heading (§26.1), imagen (§26.2) y
/// recursión en bloques contenedores.
fn decorate(block: &mut Block, source: &str, base: usize) {
    match &mut block.kind {
        BlockKind::Heading { content, attrs, .. } => {
            if attrs.is_none() {
                *attrs = trailing_attrs(content);
            }
            attach_image_attrs(content);
        }
        BlockKind::Paragraph { content } => attach_image_attrs(content),
        BlockKind::Table {
            header,
            rows,
            attrs,
            ..
        } => {
            // CommonMark admite una fila sin barras y consume la línea de
            // atributos normativa como última fila. Recuperarla desde el
            // fuente evita perder el ID de tabla sin reescribir Markdown.
            if attrs.is_none() && !rows.is_empty() {
                let start = block.span.start as usize - base;
                let end = block.span.end as usize - base;
                if let Some(line) = source.get(start..end).and_then(|s| s.lines().last()) {
                    if let Some(value) = parse_attr_line(line.trim()) {
                        rows.pop();
                        *attrs = Some(value);
                    }
                }
            }
            for cell in header.iter_mut() {
                attach_image_attrs(cell);
            }
            for cell in rows.iter_mut().flatten() {
                attach_image_attrs(cell);
            }
        }
        BlockKind::Quote { blocks }
        | BlockKind::Callout { blocks, .. }
        | BlockKind::FootnoteDefinition { blocks, .. } => attach_attributes(blocks, source, base),
        BlockKind::List { items, .. } => {
            for item in items {
                attach_attributes(&mut item.blocks, source, base);
            }
        }
        BlockKind::CodeBlock { .. } | BlockKind::MathBlock { .. } | BlockKind::Rule | BlockKind::Bibliography { .. } => {}
    }
}

/// Posición 3: `## Título {#id}` (§26.1). La regla de extracción vive en
/// [`opm_document::strip_trailing_attrs`] (compartida con el escáner de
/// anclas del índice).
fn trailing_attrs(content: &mut Vec<Inline>) -> Option<Attributes> {
    let last = content.last_mut()?;
    let Inline::Text(text) = last else {
        return None;
    };
    let (rest, attrs) = opm_document::strip_trailing_attrs(text)?;
    *text = rest.to_string();
    if text.is_empty() {
        content.pop();
    }
    Some(attrs)
}

/// Posición 1: `![alt](ruta) {width=50%}` (§18.5, §26.2).
fn attach_image_attrs(inlines: &mut Vec<Inline>) {
    for inline in inlines.iter_mut() {
        match inline {
            Inline::Emphasis(content)
            | Inline::Strong(content)
            | Inline::Strikethrough(content) => attach_image_attrs(content),
            Inline::Link { content, .. } | Inline::Image { alt: content, .. } => {
                attach_image_attrs(content)
            }
            _ => {}
        }
    }

    let mut i = 0;
    while i + 1 < inlines.len() {
        let has_attrs = matches!(inlines[i], Inline::Image { attrs: None, .. });
        let Inline::Text(text) = &inlines[i + 1] else {
            i += 1;
            continue;
        };
        if !has_attrs {
            i += 1;
            continue;
        }
        let Some((attrs, rest)) = leading_attr(text) else {
            i += 1;
            continue;
        };
        if let Inline::Image { attrs: slot, .. } = &mut inlines[i] {
            *slot = Some(attrs);
        }
        if rest.is_empty() {
            inlines.remove(i + 1);
        } else {
            inlines[i + 1] = Inline::Text(rest);
        }
        i += 1;
    }
}

/// Extrae un constructo `{...}` al inicio de un texto literal.
fn leading_attr(text: &str) -> Option<(Attributes, String)> {
    let trimmed = text.trim_start();
    let close = trimmed.find('}')?;
    let attrs = parse_attributes(&trimmed[1..close])?;
    if attrs.is_empty() {
        return None;
    }
    let rest = trimmed[close + 1..].to_string();
    Some((attrs, rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opm_document::Substitution;

    fn parse(source: &str) -> Ast {
        let options = ParseOptions::default();
        parse_document(source, 0, &options, true).0
    }

    #[test]
    fn front_matter_yaml_se_extrae() {
        let ast = parse("---\ntitle: x\n---\n\n# H\n");
        assert_eq!(ast.front_matter.as_deref().map(str::trim), Some("title: x"));
        assert_eq!(ast.blocks.len(), 1);
        assert!(matches!(
            &ast.blocks[0].kind,
            BlockKind::Heading { level: 1, .. }
        ));
    }

    #[test]
    fn sin_cierre_no_hay_front_matter() {
        let ast = parse("---\n\n# H\n");
        assert_eq!(ast.front_matter, None);
        assert!(matches!(&ast.blocks[0].kind, BlockKind::Rule));
    }

    #[test]
    fn callouts_tipicos() {
        let ast = parse("::: warning\nNo elimines.\n:::\n");
        let BlockKind::Callout {
            callout_type,
            title,
            blocks,
        } = &ast.blocks[0].kind
        else {
            panic!("esperaba callout: {:?}", ast.blocks[0]);
        };
        assert_eq!(callout_type, "warning");
        assert_eq!(*title, None);
        assert_eq!(blocks.len(), 1);

        let ast = parse("::: warning \"Atención\"\nContenido.\n:::\n");
        let BlockKind::Callout { title, .. } = &ast.blocks[0].kind else {
            panic!("esperaba callout: {:?}", ast.blocks[0]);
        };
        assert_eq!(title.as_deref(), Some("Atención"));
    }

    #[test]
    fn callout_anidado_y_con_markdown() {
        let ast = parse("::: note\n**fuerte**\n\n::: tip\ninterior\n:::\n:::\n");
        let BlockKind::Callout { blocks, .. } = &ast.blocks[0].kind else {
            panic!("esperaba callout: {:?}", ast.blocks[0]);
        };
        // Párrafo con énfasis + callout anidado.
        assert_eq!(blocks.len(), 2);
        assert!(matches!(
            &blocks[0].kind,
            BlockKind::Paragraph { content }
                if content.contains(&Inline::Strong(vec![Inline::Text("fuerte".into())]))
        ));
        assert!(
            matches!(&blocks[1].kind, BlockKind::Callout { callout_type, .. } if callout_type == "tip")
        );
    }

    #[test]
    fn callout_tipo_desconocido_es_texto_literal() {
        let ast = parse("::: desconocido\ncontenido\n:::\n");
        assert!(matches!(
            &ast.blocks[0].kind,
            BlockKind::Paragraph { content }
                if content.iter().any(|i| matches!(i, Inline::Text(t) if t.contains("desconocido")))
        ));
    }

    #[test]
    fn ecuacion_de_bloque() {
        let ast = parse("$$\nE = mc^2\n$$\n");
        assert_eq!(
            ast.blocks[0].kind,
            BlockKind::MathBlock {
                latex: "E = mc^2".into(),
                attrs: None,
            }
        );
    }

    #[test]
    fn ecuacion_sin_cierre_es_texto_literal() {
        let ast = parse("$$\nsin cierre\n");
        assert!(matches!(
            &ast.blocks[0].kind,
            BlockKind::Paragraph { content }
                if content.iter().any(|i| matches!(i, Inline::Text(t) if t.contains("sin cierre")))
        ));
    }

    #[test]
    fn atributos_de_heading_tabla_ecuacion_e_imagen() {
        // §26.1: heading.
        let ast = parse("## Arquitectura {#architecture}\n");
        let BlockKind::Heading { content, attrs, .. } = &ast.blocks[0].kind else {
            panic!("esperaba heading: {:?}", ast.blocks[0]);
        };
        assert_eq!(
            attrs.as_ref().and_then(|a| a.id.as_deref()),
            Some("architecture")
        );
        assert_eq!(content, &[Inline::Text("Arquitectura".into())]);

        // §26.3: tabla con línea de atributos propia.
        let ast = parse("| A | B |\n|---|---|\n| 1 | 2 |\n\n{#tbl:results \"Resultados\"}\n");
        let BlockKind::Table { attrs, .. } = &ast.blocks[0].kind else {
            panic!("esperaba tabla: {:?}", ast.blocks[0]);
        };
        assert_eq!(
            attrs.as_ref().and_then(|a| a.id.as_deref()),
            Some("tbl:results")
        );
        assert_eq!(ast.blocks.len(), 1);

        // §26.4: ecuación con línea de atributos propia.
        let ast = parse("$$\nE = mc^2\n$$\n{#eq:energy \"Energía\"}\n");
        let BlockKind::MathBlock { attrs, .. } = &ast.blocks[0].kind else {
            panic!("esperaba ecuación: {:?}", ast.blocks[0]);
        };
        assert_eq!(
            attrs.as_ref().and_then(|a| a.id.as_deref()),
            Some("eq:energy")
        );

        // §18.5 / §26.2: imagen con atributos inline.
        let ast = parse("![x](y.png) {#fig:a width=50%}\n");
        let BlockKind::Paragraph { content } = &ast.blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", ast.blocks[0]);
        };
        let Inline::Image { attrs, .. } = &content[0] else {
            panic!("esperaba imagen: {content:?}");
        };
        let attrs = attrs.as_ref().unwrap();
        assert_eq!(attrs.id.as_deref(), Some("fig:a"));
        assert_eq!(attrs.width_percent, Some(50));
    }

    #[test]
    fn atributos_fuera_de_posicion_son_texto_literal() {
        // §7.4.3: en cualquier otra posición, `{...}` es texto literal.
        let ast = parse("texto {#a} más\n");
        let BlockKind::Paragraph { content } = &ast.blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", ast.blocks[0]);
        };
        assert!(content
            .iter()
            .any(|i| matches!(i, Inline::Text(t) if t.contains("{#a}"))));

        // Una línea de atributos sin bloque previo que la reciba también.
        let ast = parse("{#tbl:x}\n");
        let BlockKind::Paragraph { content } = &ast.blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", ast.blocks[0]);
        };
        assert!(content
            .iter()
            .any(|i| matches!(i, Inline::Text(t) if t.contains("{#tbl:x}"))));
    }

    #[test]
    fn sustituciones_y_callouts_en_contexto() {
        let ast = parse("Ver {{ref:fig:arq}} aquí.\n");
        let BlockKind::Paragraph { content } = &ast.blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", ast.blocks[0]);
        };
        assert!(
            content.contains(&Inline::Substitution(Substitution::CrossRef {
                id: "fig:arq".into()
            }))
        );

        let options = ParseOptions {
            custom_callout_types: &["quiz".to_string()],
            ..Default::default()
        };
        let (ast, _) = parse_document("::: quiz\nx\n:::\n", 0, &options, true);
        assert!(
            matches!(&ast.blocks[0].kind, BlockKind::Callout { callout_type, .. } if callout_type == "quiz")
        );
    }

    #[test]
    fn spans_cubren_el_fuente_global() {
        // Base de offsets: los spans son coordenadas del documento completo,
        // incluso dentro de callouts (§9.1 usa estas posiciones).
        let source = "::: info\ncontenido\n:::\n";
        let ast = parse(source);
        let BlockKind::Callout { blocks, .. } = &ast.blocks[0].kind else {
            panic!("esperaba callout: {:?}", ast.blocks[0]);
        };
        let inner_span = blocks[0].span;
        assert_eq!(
            &source[inner_span.start as usize..inner_span.end as usize],
            "contenido\n"
        );
    }
}
