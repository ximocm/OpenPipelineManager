// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Anclas y IDs de documento: la política única de destinos `#fragmento`
//! (§19.2, §26.1, §27.1).
//!
//! Un heading es alcanzable por enlace cuando el escáner [`find_heading_nodes`]
//! lo encuentra. La Preview (vía `minmark-renderer`) y el índice del proyecto
//! (vía `minmark-fs`) derivan sus IDs de esta única función, de modo que
//! **producen siempre los mismos destinos**: el HTML del Preview emite
//! `id="<ancla>"` exactamente donde el índice resuelve el fragmento.
//!
//! ## Política de IDs (única)
//!
//! 1. Un ID explícito `{#id}` (§26.1) se usa tal cual (charset
//!    `[A-Za-z0-9:._-]+`, sensible a mayúsculas).
//! 2. Sin ID explícito, el ancla automático es [`slugify`] del texto del
//!    heading: minúsculas Unicode, letras/dígitos/`-` conservados, marcas
//!    inline (énfasis, código, enlaces…) eliminadas, espacios → `-`; un texto
//!    sin ningún carácter utilizable produce `seccion`.
//! 3. Los duplicados se resuelven en orden de documento con sufijo numérico
//!    sobre su propia base: el primer `Introducción` es `introduccion`, los
//!    siguientes `introduccion-1`, `introduccion-2`, … Esto vale también para
//!    IDs explícitos repetidos (el primero conserva el ID declarado).
//! 4. El fragmento de un enlace debe coincidir **exactamente** con el ancla
//!    final (sin plegado de mayúsculas).
//!
//! ## Cobertura
//!
//! Headings ATX (§27.1) y setext, incluidos los anidados en citas/listas y los
//! anidados en listas, con el texto plano del heading como base del slug. Los
//! headings dentro de bloques de código cercados y del front matter (§21) no
//! generan ancla. Un heading setext multilínea usa todo su párrafo como texto.
//!
//! Módulo puro (§4.1): solo `std`.

use crate::attrs::strip_trailing_attrs;
use crate::references::split_front_matter;
use crate::Span;
use std::collections::BTreeMap;

/// Heading ATX de un fuente Markdown (§27.1), para el índice §45 y el
/// autocompletado §41.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// Nivel 1–6.
    pub level: u8,
    /// Texto del heading (sin los `#`, sin el cierre opcional y sin los
    /// atributos `{...}` finales, como en el AST).
    pub text: String,
}

/// Heading localizado en el fuente: base de las anclas y de la navegación.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingNode {
    pub level: u8,
    /// Texto del heading (misma forma que [`Heading::text`]).
    pub text: String,
    /// ID explícito `{#id}` (§26.1), si lo declara.
    pub id: Option<String>,
    /// Rango del heading en el fuente (desde el primer carácter del heading
    /// hasta el final de su última línea, salto incluido).
    pub span: Span,
    /// Línea (1-based) donde empieza el heading.
    pub line: u32,
}

/// Ancla final de un documento: identificador único y heading alcanzado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// ID final (§26): explícito o automático con sufijo de unicidad.
    pub id: String,
    pub level: u8,
    pub text: String,
    /// Línea (1-based) del heading en el fuente resuelto.
    pub line: u32,
}

/// Headings de un fuente (§27.1): proyección de [`find_heading_nodes`].
pub fn find_headings(source: &str) -> Vec<Heading> {
    find_heading_nodes(source)
        .into_iter()
        .map(|node| Heading {
            level: node.level,
            text: node.text,
        })
        .collect()
}

/// Headings ATX y setext del cuerpo de un fuente Markdown, en orden de
/// documento, con posición e ID explícito (§26.1).
///
/// Saltos de código cercado (` ``` ` / `~~~`) y del front matter (§21: un `#`
/// allí es comentario YAML). Los headings ATX tras marcas de cita (`> # x`)
/// también se localizan: la AST los parsea como headings y deben recibir la
/// misma ancla que el resto.
pub fn find_heading_nodes(source: &str) -> Vec<HeadingNode> {
    let mut nodes = Vec::new();
    let mut body_start = 0;
    if let Some((_, body)) = split_front_matter(source) {
        body_start = body;
    }
    let body = &source[body_start..];
    let mut line_starts: Vec<usize> = vec![body_start];
    for (offset, _) in body.match_indices('\n') {
        line_starts.push(body_start + offset + 1);
    }
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let contents: Vec<&str> = lines
        .iter()
        .map(|l| l.trim_end_matches(['\n', '\r']))
        .collect();

    let line_content = |i: usize| -> &str { contents.get(i).copied().unwrap_or("") };
    let line_start = |i: usize| -> usize { line_starts[i] };
    let line_end = |i: usize| -> usize {
        line_start(i) + lines[i].len() // incluye el salto, como la AST
    };
    // Línea absoluta (1-based) de un offset del fuente completo.
    let line_no = |offset: usize| -> u32 { (source[..offset].matches('\n').count() + 1) as u32 };

    let mut fence: Option<(u8, usize)> = None;
    let mut comment = false;
    let mut i = 0;
    while i < lines.len() {
        let content = line_content(i);
        let trimmed = content.trim_start();
        let indent = content.len() - trimmed.len();
        if fence.is_none()
            && (comment
                || strip_quote_prefix(content)
                    .1
                    .trim_start()
                    .starts_with("<!--"))
        {
            comment = !content.contains("-->");
            i += 1;
            continue;
        }
        let marker = trimmed
            .as_bytes()
            .first()
            .copied()
            .filter(|b| *b == b'`' || *b == b'~');
        if indent <= 3 {
            if let Some(marker) = marker {
                if run_len(trimmed, marker) >= 3 {
                    fence = match fence {
                        Some((open, length))
                            if open == marker
                                && run_len(trimmed, marker) >= length
                                && trimmed[run_len(trimmed, marker)..].trim().is_empty() =>
                        {
                            None
                        }
                        Some(open) => Some(open),
                        None => Some((marker, run_len(trimmed, marker))),
                    };
                    i += 1;
                    continue;
                }
            }
        }
        if fence.is_some() {
            i += 1;
            continue;
        }

        // Heading ATX (§27.1), también tras marcas de cita.
        if indent <= 3 {
            let (col, text) = strip_quote_prefix(content);
            if let Some(node) = atx_heading(
                text,
                line_start(i) + col,
                line_end(i),
                line_no(line_start(i)),
            ) {
                nodes.push(node);
                i += 1;
                continue;
            }
            // Heading setext: línea de texto + subrayado `===` / `---`.
            if !trimmed.starts_with('>') && setext_level(line_content(i + 1)).is_some() {
                if let Some((start, level)) = paragraph_start(&contents, i) {
                    let raw: Vec<&str> = (start..=i).map(line_content).collect();
                    let joined = raw.join(" ");
                    let (text, id) = heading_text_id(&joined);
                    nodes.push(HeadingNode {
                        level,
                        text,
                        id,
                        span: Span {
                            start: line_start(start) as u32,
                            end: line_end(i + 1) as u32,
                        },
                        line: line_no(line_start(start)),
                    });
                    i += 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    nodes
}

/// Anclas de un documento, en orden: una por heading con la política única de
/// IDs (ver módulo).
pub fn document_anchors(headings: &[HeadingNode]) -> Vec<Anchor> {
    let mut used: BTreeMap<String, ()> = BTreeMap::new();
    let mut anchors = Vec::with_capacity(headings.len());
    for node in headings {
        let base = node.id.clone().unwrap_or_else(|| slugify(&node.text));
        let id = if used.insert(base.clone(), ()).is_none() {
            base
        } else {
            let mut n = 1u32;
            loop {
                let candidate = format!("{base}-{n}");
                if used.insert(candidate.clone(), ()).is_none() {
                    break candidate;
                }
                n += 1;
            }
        };
        anchors.push(Anchor {
            id,
            level: node.level,
            text: node.text.clone(),
            line: node.line,
        });
    }
    anchors
}

/// Busca el ancla `fragment` (coincidencia exacta del ID, sin la `#`).
pub fn find_anchor<'a>(anchors: &'a [Anchor], fragment: &str) -> Option<&'a Anchor> {
    anchors.iter().find(|anchor| anchor.id == fragment)
}

/// Base automática de un ancla (política del módulo): texto plano del heading
/// (marcas inline eliminadas, como el contenido renderizado) en minúsculas
/// Unicode, con letras/dígitos/`-` conservados y espacios → `-`.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for c in plain_heading_text(text).chars() {
        if c.is_alphanumeric() || c == '-' {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            out.extend(c.to_lowercase());
        } else if c.is_whitespace() {
            pending_sep = true;
        }
        // El resto (puntuación) se elimina sin separar: «1.1» → «11».
    }
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() {
        "seccion".to_owned()
    } else {
        out
    }
}

/// Texto plano del heading: marcado inline eliminado como en el contenido
/// renderizado — enlaces e imágenes dan su texto, los wiki links su destino,
/// los code spans su contenido y el color su texto; las marcas de énfasis y
/// los escapes se resuelven.
fn plain_heading_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => {
                let rest = &text[i + 1..];
                let c = rest.chars().next().unwrap();
                out.push(c);
                i += 1 + c.len_utf8();
            }
            b'`' => {
                let run = run_len(&text[i..], b'`');
                let closing = "`".repeat(run);
                let search_from = i + run;
                let end = text[search_from..].find(&closing).map(|p| search_from + p);
                let content_end = end.unwrap_or(bytes.len());
                out.push_str(&text[search_from..content_end]);
                i = end.map(|p| p + run).unwrap_or(bytes.len());
            }
            b'!' if text[i..].starts_with("![") => {
                i += 1; // el alt del enlace `[alt]` se procesa como texto.
            }
            b'[' if text[i..].starts_with("[[") => {
                // Wiki link `[[destino#frag]]` (§19.3): da su destino.
                match text[i + 2..].find("]]") {
                    Some(pos) => {
                        let target = &text[i + 2..i + 2 + pos];
                        let target = target.split('#').next().unwrap_or(target);
                        out.push_str(target);
                        i = i + 2 + pos + 2;
                    }
                    None => {
                        out.push('[');
                        i += 1;
                    }
                }
            }
            b'[' => {
                // Enlace `[texto](dest)` / `[texto][id]`: da su texto.
                match text[i + 1..].find(']') {
                    Some(pos) => {
                        out.push_str(&text[i + 1..i + 1 + pos]);
                        i += 1 + pos + 1;
                        if bytes.get(i) == Some(&b'(') {
                            i = match text[i..].find(')') {
                                Some(p) => i + p + 1,
                                None => bytes.len(),
                            };
                        } else if bytes.get(i) == Some(&b'[') {
                            i = match text[i..].find(']') {
                                Some(p) => i + p + 1,
                                None => bytes.len(),
                            };
                        }
                    }
                    None => {
                        out.push('[');
                        i += 1;
                    }
                }
            }
            b'{' => {
                // Color `{fg: t}` (§7.3) da su texto; el resto de llaves, el
                // interior tal cual.
                match text[i..].find('}') {
                    Some(pos) => {
                        let inner = &text[i + 1..i + pos];
                        match inner.split_once(':') {
                            Some((_, value)) => out.push_str(value.trim()),
                            None => out.push_str(inner),
                        }
                        i += pos + 1;
                    }
                    None => {
                        out.push('{');
                        i += 1;
                    }
                }
            }
            b'*' | b'_' | b'~' => i += 1, // marcas de énfasis (§7.1)
            _ => {
                let c = text[i..].chars().next().unwrap();
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

/// Texto e ID explícito de un heading (§26.1): cierre opcional ` #…` y
/// atributos `{...}` finales eliminados, como en el AST.
fn heading_text_id(raw: &str) -> (String, Option<String>) {
    let mut text = raw.trim().to_owned();
    // Cierre opcional ` #…` (CommonMark).
    if let Some(pos) = text.rfind(' ') {
        if !text[pos + 1..].is_empty() && text[pos + 1..].chars().all(|c| c == '#') {
            text.truncate(pos);
        }
    }
    match strip_trailing_attrs(&text) {
        Some((rest, attrs)) => (rest.to_owned(), attrs.id),
        None => (text.trim_end().to_owned(), None),
    }
}

/// Heading ATX de la línea `text` (sin marcas de cita), si lo es: 1–6 `#`
/// seguidos de espacio, tabulador o fin de línea (§27.1; `#hashtag` no vale).
fn atx_heading(text: &str, marker_start: usize, end: usize, line: u32) -> Option<HeadingNode> {
    let hashes = run_len(text, b'#');
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &text[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let (text, id) = heading_text_id(rest);
    Some(HeadingNode {
        level: hashes as u8,
        text,
        id,
        span: Span {
            start: marker_start as u32,
            end: end as u32,
        },
        line,
    })
}

/// Subrayado setext de la línea: `=`+ (nivel 1) o `-`+ (nivel 2), con
/// indentación ≤ 3 como el resto del dialecto.
fn setext_level(line: &str) -> Option<u8> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 || trimmed.is_empty() {
        return None;
    }
    let trimmed = trimmed.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let first = trimmed.as_bytes()[0];
    if first != b'=' && first != b'-' {
        return None;
    }
    if !trimmed.bytes().all(|b| b == first) {
        return None;
    }
    Some(if first == b'=' { 1 } else { 2 })
}

/// Inicio del párrafo setext que termina en `i` y su nivel: se retrocede sobre
/// las líneas de continuación (un heading setext abarca todo el párrafo).
fn paragraph_start(contents: &[&str], i: usize) -> Option<(usize, u8)> {
    let at = |j: usize| -> &str { contents.get(j).copied().unwrap_or("") };
    if !is_paragraph_line(at(i)) {
        return None;
    }
    let mut start = i;
    while start > 0 && is_paragraph_line(at(start - 1)) {
        start -= 1;
    }
    Some((start, setext_level(at(i + 1))?))
}

/// ¿La línea continúa un párrafo (y por tanto puede formar un setext)?
fn is_paragraph_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let indent_ok = line.len() - line.trim_start().len() <= 3;
    if !indent_ok {
        return false;
    }
    let first = trimmed.as_bytes()[0];
    // Bloques propios: cita, ATX, valla, lista y subrayado setext.
    if first == b'>' || first == b'#' || first == b'`' || first == b'~' {
        return false;
    }
    if setext_level(line).is_some() {
        return false;
    }
    if matches!(first, b'-' | b'*' | b'+') {
        let rest = &trimmed[1..];
        if rest.is_empty() || rest.starts_with([' ', '\t']) {
            return false;
        }
    }
    let digits = trimmed.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 && digits <= 9 {
        let rest = &trimmed[digits..];
        // Marcador de lista ordenada: `1.` + espacio (no `1.1 título`).
        if rest.starts_with(". ") || rest == "." {
            return false;
        }
    }
    true
}

/// Quita las marcas de cita (`>`, `> >`…) y devuelve `(columna, resto)`: la
/// columna es donde empieza el contenido real del heading.
fn strip_quote_prefix(content: &str) -> (usize, &str) {
    let bytes = content.as_bytes();
    let mut pos = content.len() - content.trim_start().len();
    loop {
        if bytes.get(pos) != Some(&b'>') {
            break;
        }
        pos += 1;
        if bytes.get(pos) == Some(&b' ') {
            pos += 1;
        }
    }
    let rest = &content[pos..];
    let extra = rest.len() - rest.trim_start().len();
    (pos + extra.min(3), &rest[extra.min(3)..])
}

fn run_len(source: &str, byte: u8) -> usize {
    source.as_bytes().iter().take_while(|b| **b == byte).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchors(source: &str) -> Vec<String> {
        document_anchors(&find_heading_nodes(source))
            .into_iter()
            .map(|anchor| anchor.id)
            .collect()
    }

    fn lines(source: &str) -> Vec<u32> {
        find_heading_nodes(source)
            .into_iter()
            .map(|node| node.line)
            .collect()
    }

    #[test]
    fn headings_atx_27_1() {
        let source = "---\ntitle: # no heading\n---\n# Uno\n\n## Dos con `code` ###\n\n\
                      ###hashtag no\n\n```\n# en codigo\n```\n\n####\n\nCuerpo\n";
        assert_eq!(
            find_headings(source),
            [
                Heading {
                    level: 1,
                    text: "Uno".to_owned()
                },
                Heading {
                    level: 2,
                    text: "Dos con `code`".to_owned()
                },
                Heading {
                    level: 4,
                    text: String::new().to_owned()
                },
            ]
        );
    }

    #[test]
    fn ids_automaticos_slug_y_duplicados() {
        assert_eq!(
            anchors("# Introducción\n\n# Introducción\n\n# Introducción\n"),
            ["introducción", "introducción-1", "introducción-2"]
        );
        assert_eq!(anchors("# Dos con `code`\n"), ["dos-con-code"]);
        assert_eq!(
            anchors("# **Importante** y [enlace](x.md)\n"),
            ["importante-y-enlace"]
        );
        assert_eq!(anchors("### !!!\n"), ["seccion"]);
        assert_eq!(anchors("# Ñandú 見出し\n"), ["ñandú-見出し"]);
    }

    #[test]
    fn ids_explicitos_26_1_y_duplicados() {
        // El ID explícito se usa tal cual (§26.1) y también se desambigua.
        assert_eq!(
            anchors("# Título {#architecture}\n\n## Otra {#architecture}\n"),
            ["architecture", "architecture-1"]
        );
        // Los automáticos no pisan a un explícito ya usado.
        assert_eq!(
            anchors("# Architecture\n\n# Otra {#architecture}\n"),
            ["architecture", "architecture-1"]
        );
    }

    #[test]
    fn headings_setext_con_parrafo_completo() {
        let source = "Título\n=====\n\npárrafo\ncontinuación\n---\n";
        let nodes = find_heading_nodes(source);
        assert_eq!(
            nodes
                .iter()
                .map(|n| (n.level, n.text.as_str()))
                .collect::<Vec<_>>(),
            [(1, "Título"), (2, "párrafo continuación")]
        );
        assert_eq!(lines(source), [1, 4]);
        assert_eq!(anchors(source), ["título", "párrafo-continuación"]);
    }

    #[test]
    fn posiciones_y_spans_con_crlf() {
        let source = "# Uno\r\n\r\n## Dos {#d}\r\n";
        let nodes = find_heading_nodes(source);
        assert_eq!(lines(source), [1, 3]);
        assert_eq!(nodes[0].text, "Uno");
        assert_eq!(nodes[1].id.as_deref(), Some("d"));
        assert_eq!(
            &source[nodes[0].span.start as usize..nodes[0].span.end as usize],
            "# Uno\r\n"
        );
    }

    #[test]
    fn citas_y_listas_reciben_ancla() {
        let source = "> # Citado\n\n- item\n\n  # En lista\n";
        assert_eq!(anchors(source), ["citado", "en-lista"]);
    }

    #[test]
    fn busqueda_exacta_de_ancla() {
        let source = "# Introducción\n\n# Introducción\n";
        let anchors = document_anchors(&find_heading_nodes(source));
        assert_eq!(
            find_anchor(&anchors, "introducción").map(|a| a.line),
            Some(1)
        );
        assert_eq!(
            find_anchor(&anchors, "introducción-1").map(|a| a.line),
            Some(3)
        );
        assert_eq!(
            find_anchor(&anchors, "Introducción"),
            None,
            "coincidencia exacta"
        );
        assert_eq!(find_anchor(&anchors, "no-esta"), None);
    }
}
