// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Referencias de un documento fuente (ISSUE-023, §16): localización y
//! reescritura de destinos referenciados, sobre el texto tal cual (§6.2: el
//! Markdown original se preserva; solo se sustituyen los destinos).
//!
//! Formas cubiertas (§16, §19, §21):
//! - enlaces e imágenes Markdown `[t](dest)` / `![t](dest)`, con destino
//!   plano o entre `<...>` y título opcional (§19.1/§19.2; el fragmento
//!   `#…` forma parte del destino);
//! - definiciones de enlace `[id]: dest` (enlaces por referencia);
//! - wiki links `[[dest]]` / `[[dest#frag]]` (§19.3; solo son enlaces cuando
//!   están activos — el consumidor decide);
//! - valores tipo ruta del front matter YAML (§21: `clave: valor`, con listas
//!   `- valor`) y de TOML (`.mdproject.toml`, composiciones): cadenas
//!   entrecomilladas.
//!
//! Los bloques de código cercados, los spans de código inline y los
//! comentarios HTML se ignoran: un destino dentro de código no es una
//! referencia. Módulo puro (§4.1): solo `std`.

use crate::Span;

/// Clase de referencia encontrada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// `[texto](dest)` o definición `[id]: dest`.
    MarkdownLink,
    /// `![alt](dest)`.
    MarkdownImage,
    /// `[[dest]]` (§19.3).
    WikiLink,
    /// Valor tipo ruta de front matter YAML o de TOML.
    ConfigPath,
}

/// Forma del destino en el fuente (se preserva al reescribir).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestForm {
    /// Destino plano (sin `<…>`).
    Plain,
    /// Destino entre `<…>`.
    Angle,
    /// Cadena entrecomillada de YAML/TOML (con su comilla inicial registrada
    /// en [`Reference::quote`]).
    Quoted,
}

/// Una referencia con destino reescribible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub kind: RefKind,
    /// Rango del constructo completo para activación y diagnósticos.
    pub full_span: Span,
    /// Rango de bytes del **destino** en el fuente (sin `<`/`>`, sin comillas,
    /// sin título y sin el `#frag`, que viaja dentro de [`Reference::dest`]).
    pub span: Span,
    /// Destino literal tal cual aparece (incluye `#frag` si lo hay).
    pub dest: String,
    pub form: DestForm,
    /// Comilla de apertura para [`DestForm::Quoted`] (`"` o `'`).
    pub quote: char,
    /// `true` si el destino usa escapes (`\x`) o percent-encoding (`%xx`):
    /// reescribirlo sin garantías está prohibido (§16: abortar).
    pub encoded: bool,
}

impl Reference {
    fn new(kind: RefKind, span: Span, dest: String, form: DestForm) -> Self {
        let encoded = dest.contains('%') || dest.contains('\\');
        Self {
            kind,
            full_span: span,
            span,
            dest,
            form,
            quote: '"',
            encoded,
        }
    }
}

/// Referencias de un fuente Markdown (cuerpo + front matter §21).
pub fn find_markdown_references(source: &str) -> Vec<Reference> {
    scan_markdown_links(source).references
}

/// Definiciones de enlace por referencia `[id]: dest` (§16) y usos
/// `[texto][id]` / `[texto][]` de un fuente Markdown, junto con las
/// referencias reescribibles ([`Reference`]).
pub fn scan_markdown_links(source: &str) -> LinkScan {
    let mut scan = LinkScan::default();
    let mut body_start = 0;
    if let Some((fm, body)) = split_front_matter(source) {
        let fm_source = &source[fm.start as usize..fm.end as usize];
        find_config_lines(fm_source, fm.start as usize, &mut scan.references);
        body_start = body;
    }
    find_markdown_body(&source[body_start..], body_start, &mut scan);
    scan
}

/// Definiciones de enlace por referencia `[id]: dest` (§16) de un fuente
/// Markdown (el primero por etiqueta es el que gana al resolver).
pub fn find_link_definitions(source: &str) -> Vec<LinkDefinition> {
    scan_markdown_links(source).definitions
}

/// Usos por referencia de un fuente Markdown (§16).
pub fn find_reference_usages(source: &str) -> Vec<ReferenceUsage> {
    scan_markdown_links(source).usages
}

/// Referencias de un fuente Markdown en todas sus formas (§16, §19): lo que
/// reescribe [`apply_replacements`] y lo que se resuelve al navegar.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LinkScan {
    /// Destinos referenciados reescribibles ([`Reference`]).
    pub references: Vec<Reference>,
    /// Definiciones `[id]: dest`.
    pub definitions: Vec<LinkDefinition>,
    /// Usos `[texto][id]` / `[texto][]`.
    pub usages: Vec<ReferenceUsage>,
}

/// Definición de enlace por referencia `[id]: dest` (§16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkDefinition {
    /// Etiqueta normalizada ([`normalize_label`]).
    pub label: String,
    /// Destino literal tal cual aparece (incluye `#frag` si lo hay).
    pub dest: String,
    /// Rango de bytes del destino en el fuente (como [`Reference::span`]).
    pub span: Span,
}

/// Uso por referencia `[texto][id]` / `[texto][]` (§16). El atajo `[texto]`
/// no se considera uso: sin corchetes de etiqueta no hay forma de distinguirlo
/// de un texto entre corchetes (frontera documentada).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceUsage {
    /// [`RefKind::MarkdownLink`] o [`RefKind::MarkdownImage`] (`![alt][id]`).
    pub kind: RefKind,
    /// Etiqueta normalizada ([`normalize_label`]); en `[texto][]` es el texto.
    pub label: String,
    /// Rango de bytes del uso completo en el fuente.
    pub span: Span,
}

/// Etiqueta de enlace por referencia normalizada (CommonMark: sin espacios
/// redundantes y sin distinguir mayúsculas).
pub fn normalize_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Referencias de un fuente TOML (`.mdproject.toml`, composiciones).
pub fn find_toml_references(source: &str) -> Vec<Reference> {
    let mut refs = Vec::new();
    find_toml_strings(source, 0, &mut refs);
    refs
}

/// Headings: [`crate::anchors::find_headings`] y
/// [`crate::anchors::find_heading_nodes`] (§27.1, §26.1).

/// Aplica sustituciones de destino por rango de bytes, de mayor a menor
/// offset (los rangos no se solapan por construcción).
pub fn apply_replacements(source: &str, edits: &[(Span, String)]) -> String {
    let mut out = source.to_owned();
    let mut sorted = edits.to_vec();
    sorted.sort_by_key(|(span, _)| span.start);
    for (span, replacement) in sorted.into_iter().rev() {
        out.replace_range(span.start as usize..span.end as usize, &replacement);
    }
    out
}

// --- Front matter (§21) ------------------------------------------------------

/// Devuelve el front matter inicial `(contenido, offset)` y el offset del
/// cuerpo. El front matter es un bloque `---` al principio del fichero.
pub(crate) fn split_front_matter(source: &str) -> Option<(Span, usize)> {
    let rest = source
        .strip_prefix("---\n")
        .or_else(|| source.strip_prefix("---\r\n"))?;
    let offset = source.len() - rest.len();
    for end in ["\n---\n", "\n---\r\n", "\n---"] {
        if let Some(pos) = rest.find(end) {
            return Some((
                Span {
                    start: offset as u32,
                    end: (offset + pos) as u32,
                },
                offset + pos,
            ));
        }
    }
    None
}

/// Escanea líneas `clave: valor` / `- valor` tipo YAML de front matter.
fn find_config_lines(source: &str, base: usize, refs: &mut Vec<Reference>) {
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let without_comment = strip_yaml_comment(trimmed);
        let content = without_comment.trim_start();
        if content.is_empty() {
            continue;
        }
        // `clave: valor` (con valor vacío abre una lista) o ítem `- valor`.
        let value_part = if let Some(rest) = content.strip_prefix("- ") {
            rest
        } else if let Some((key, rest)) = content.split_once(':') {
            if key.is_empty()
                || !key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            {
                continue;
            }
            rest.trim_start()
        } else {
            continue;
        };
        let value_part = value_part.trim_start();
        if value_part.is_empty() {
            continue;
        }
        let value_start = base + (value_part.as_ptr() as usize - line.as_ptr() as usize);
        push_config_value(value_part, value_start, refs);
    }
}

/// Un valor YAML/TOML candidato: entrecomillado o bare token.
fn push_config_value(value: &str, start: usize, refs: &mut Vec<Reference>) {
    let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''));
    if let Some(q) = quote {
        let inner = &value[1..];
        if let Some(end) = inner.find(q) {
            let dest = inner[..end].to_owned();
            let span = Span {
                start: (start + 1) as u32,
                end: (start + 1 + end) as u32,
            };
            let mut reference = Reference::new(RefKind::ConfigPath, span, dest, DestForm::Quoted);
            reference.quote = q;
            refs.push(reference);
        }
        return;
    }
    // Token bare YAML: hasta espacio + comentario o fin de línea.
    let end = value.find(" #").unwrap_or_else(|| value.trim_end().len());
    let dest = value[..end].trim_end().to_owned();
    if !dest.is_empty() {
        let span = Span {
            start: start as u32,
            end: (start + dest.len()) as u32,
        };
        refs.push(Reference::new(
            RefKind::ConfigPath,
            span,
            dest,
            DestForm::Plain,
        ));
    }
}

fn strip_yaml_comment(line: &str) -> &str {
    match line.find(" #") {
        Some(pos) => &line[..pos],
        None => line,
    }
}

// --- TOML --------------------------------------------------------------------

/// Cadenas dobles/simples de TOML como candidatas de ruta. `"""` / `'''`
/// (multilínea) se saltan por completo: sus contenidos no son «referencias
/// conocidas» (frontera documentada en ISSUE-023).
fn find_toml_strings(source: &str, base: usize, refs: &mut Vec<Reference>) {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' | b'\'' => {
                let quote = bytes[i] as char;
                if source[i..].starts_with(&format!("{quote}{quote}{quote}")) {
                    // Cadena multilínea: se ignora hasta su cierre.
                    let close = format!("{quote}{quote}{quote}");
                    let skip = source[i + 3..]
                        .find(&close)
                        .map(|p| i + 3 + p + 3)
                        .unwrap_or(bytes.len());
                    i = skip;
                    continue;
                }
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == b'\\' && quote == '"' {
                        j += 2;
                        continue;
                    }
                    if bytes[j] == bytes[i] {
                        break;
                    }
                    j += 1;
                }
                let dest = source[i + 1..j.min(bytes.len())].to_owned();
                let span = Span {
                    start: (base + i + 1) as u32,
                    end: (base + j.min(bytes.len())) as u32,
                };
                let mut reference =
                    Reference::new(RefKind::ConfigPath, span, dest, DestForm::Quoted);
                reference.quote = quote;
                refs.push(reference);
                i = j + 1;
            }
            _ => i += 1,
        }
    }
}

// --- Cuerpo Markdown ---------------------------------------------------------

/// Escanea el cuerpo Markdown saltando código y comentarios HTML.
fn find_markdown_body(source: &str, base: usize, scan: &mut LinkScan) {
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut bracket_stack: Vec<usize> = Vec::new();
    while i < bytes.len() {
        // Valla de código al inicio de línea.
        if i == 0 || bytes[i - 1] == b'\n' {
            let line = &source[i..];
            if line.starts_with("    ") || line.starts_with('\t') {
                i += line.find('\n').map(|p| p + 1).unwrap_or(line.len());
                continue;
            }
            if let Some((marker, length)) = fence_len(source[i..].trim_start_matches(' ')) {
                // Se salta hasta el final de la línea de cierre (o EOF).
                let mut j = i + 1;
                let mut resume = bytes.len();
                while j < bytes.len() {
                    if bytes[j - 1] == b'\n' {
                        let line = &source[j..];
                        let trimmed = line.trim_start();
                        if trimmed.as_bytes().first() == Some(&marker)
                            && run_len(trimmed, marker) >= length
                            && trimmed.lines().next().unwrap_or("").trim_end_matches('\r')
                                [run_len(trimmed, marker)..]
                                .trim()
                                .is_empty()
                        {
                            resume = line.find('\n').map(|p| j + p + 1).unwrap_or(bytes.len());
                            break;
                        }
                    }
                    j += 1;
                }
                i = resume;
                continue;
            }
        }
        match bytes[i] {
            b'\\' => {
                i += 1;
                if i < bytes.len() {
                    i += source[i..].chars().next().unwrap().len_utf8();
                }
            }
            // Código inline: salto de la secuencia de backticks equivalente.
            b'`' => {
                let run = run_len(&source[i..], b'`');
                let closing = "`".repeat(run);
                let search_from = i + run;
                i = match source[search_from..].find(&closing) {
                    Some(pos) => search_from + pos + run,
                    None => bytes.len(),
                };
            }
            // Comentario HTML: no es referencia.
            b'<' if source[i..].starts_with("<!--") => {
                i = match source[i + 4..].find("-->") {
                    Some(pos) => i + 4 + pos + 3,
                    None => bytes.len(),
                };
            }
            // Wiki link [[dest]] (§19.3).
            b'[' if source[i..].starts_with("[[") => {
                if let Some(pos) = source[i + 2..].find("]]") {
                    let dest = source[i + 2..i + 2 + pos].to_owned();
                    let span = Span {
                        start: (base + i + 2) as u32,
                        end: (base + i + 2 + pos) as u32,
                    };
                    let mut reference =
                        Reference::new(RefKind::WikiLink, span, dest, DestForm::Plain);
                    reference.full_span = Span {
                        start: (base + i) as u32,
                        end: (base + i + pos + 4) as u32,
                    };
                    scan.references.push(reference);
                    i = i + 2 + pos + 2;
                    continue;
                }
                bracket_stack.push(i);
                i += 1;
            }
            b'[' => {
                bracket_stack.push(i);
                i += 1;
            }
            b']' => {
                let open = bracket_stack.pop();
                i += 1;
                let Some(open) = open else { continue };
                let image = open > 0 && bytes[open - 1] == b'!';
                let label = normalize_label(&source[open + 1..i - 1]);
                if bytes.get(i) == Some(&b'(') {
                    // `[t](dest "title")` / `![a](dest)`.
                    if let Some((dest, form, span, next)) = parse_inline_dest(source, i + 1, base) {
                        let kind = if image {
                            RefKind::MarkdownImage
                        } else {
                            RefKind::MarkdownLink
                        };
                        let mut reference = Reference::new(kind, span, dest, form);
                        reference.full_span = Span {
                            start: (base + if image { open - 1 } else { open }) as u32,
                            end: (base + next) as u32,
                        };
                        scan.references.push(reference);
                        i = next;
                    }
                } else if bytes.get(i) == Some(&b':') && is_line_start(source, open) && !label.starts_with('^') {
                    // Definición `[id]: dest`.
                    if let Some((dest, form, span, next)) = parse_inline_dest(source, i + 1, base) {
                        scan.definitions.push(LinkDefinition {
                            label,
                            dest: dest.clone(),
                            span,
                        });
                        scan.references.push(Reference::new(
                            RefKind::MarkdownLink,
                            span,
                            dest,
                            form,
                        ));
                        i = next;
                    }
                } else if bytes.get(i) == Some(&b'[') && !source[i..].starts_with("[[") {
                    // Uso por referencia `[texto][id]` / `[texto][]` (§16).
                    if let Some(pos) = source[i + 1..].find(']') {
                        let close = i + 1 + pos;
                        let inner = &source[i + 1..close];
                        let label = if inner.is_empty() {
                            label
                        } else {
                            normalize_label(inner)
                        };
                        if label.starts_with('^') { i = close + 1; continue; }
                        scan.usages.push(ReferenceUsage {
                            kind: if image {
                                RefKind::MarkdownImage
                            } else {
                                RefKind::MarkdownLink
                            },
                            label,
                            span: Span {
                                start: (base + if image { open - 1 } else { open }) as u32,
                                end: (base + close + 1) as u32,
                            },
                        });
                        i = close + 1;
                    }
                }
            }
            _ => i += 1,
        }
    }
}

/// ¿`pos` está al inicio de una línea con solo espacios antes?
fn is_line_start(source: &str, pos: usize) -> bool {
    source[..pos]
        .rsplit('\n')
        .next()
        .is_some_and(|prefix| prefix.trim().is_empty())
}

/// Longitud de la valla ``` / ~~~ al inicio de `source` (si la hay).
fn fence_len(source: &str) -> Option<(u8, usize)> {
    let marker = *source.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let len = run_len(source, marker);
    (len >= 3).then_some((marker, len))
}

fn run_len(source: &str, byte: u8) -> usize {
    source.as_bytes().iter().take_while(|b| **b == byte).count()
}

/// Parsea el destino tras `(` o `:`: `(dest, form, span, offset_siguiente)`.
///
/// El offset siguiente queda justo tras el destino (antes de título y del
/// `)` de cierre); la reescritura solo toca [`Reference::span`].
fn parse_inline_dest(
    source: &str,
    after: usize,
    base: usize,
) -> Option<(String, DestForm, Span, usize)> {
    let bytes = source.as_bytes();
    let mut i = after;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    if bytes[i] == b'<' {
        let end = source[i + 1..].find('>')? + i + 1;
        let dest = source[i + 1..end].to_owned();
        let span = Span {
            start: (base + i + 1) as u32,
            end: (base + end) as u32,
        };
        Some((dest, DestForm::Angle, span, end + 1))
    } else {
        let mut j = i;
        let mut depth = 0usize;
        while j < bytes.len() {
            match bytes[j] {
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                b if (b as char).is_whitespace() && depth == 0 => break,
                _ => {}
            }
            j += 1;
        }
        if j == i {
            return None;
        }
        let dest = source[i..j].to_owned();
        let span = Span {
            start: (base + i) as u32,
            end: (base + j) as u32,
        };
        Some((dest, DestForm::Plain, span, j))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dests(source: &str) -> Vec<(RefKind, String)> {
        find_markdown_references(source)
            .iter()
            .map(|r| (r.kind, r.dest.clone()))
            .collect()
    }

    #[test]
    fn definiciones_y_usos_por_referencia_16() {
        let source = "Ver [texto][id] y ![fig][fig] y [colapsado][].\n\n\
                      [id]: cap1/doc.md#frag\n[COLAPSADO]: otro.md\n[id]: duplicado.md\n";
        let scan = scan_markdown_links(source);
        assert_eq!(
            scan.definitions,
            [
                LinkDefinition {
                    label: "id".to_owned(),
                    dest: "cap1/doc.md#frag".to_owned(),
                    span: Span {
                        start: source.find("cap1").unwrap() as u32,
                        end: source.find("cap1").unwrap() as u32 + "cap1/doc.md#frag".len() as u32
                    },
                },
                LinkDefinition {
                    label: "colapsado".to_owned(),
                    dest: "otro.md".to_owned(),
                    span: Span {
                        start: source.find("otro.md").unwrap() as u32,
                        end: source.find("otro.md").unwrap() as u32 + "otro.md".len() as u32
                    },
                },
                LinkDefinition {
                    label: "id".to_owned(),
                    dest: "duplicado.md".to_owned(),
                    span: Span {
                        start: source.find("duplicado.md").unwrap() as u32,
                        end: source.find("duplicado.md").unwrap() as u32
                            + "duplicado.md".len() as u32
                    },
                },
            ],
            "las definiciones se conservan todas, en orden"
        );
        let usage_span = |text: &str| -> Span {
            let start = source.find(text).unwrap() as u32;
            Span {
                start,
                end: start + text.len() as u32,
            }
        };
        assert_eq!(
            scan.usages,
            [
                ReferenceUsage {
                    kind: RefKind::MarkdownLink,
                    label: "id".to_owned(),
                    span: usage_span("[texto][id]"),
                },
                ReferenceUsage {
                    kind: RefKind::MarkdownImage,
                    label: "fig".to_owned(),
                    span: usage_span("![fig][fig]"),
                },
                ReferenceUsage {
                    kind: RefKind::MarkdownLink,
                    label: "colapsado".to_owned(),
                    span: usage_span("[colapsado][]"),
                },
            ]
        );
    }

    #[test]
    fn etiquetas_normalizadas_y_atajo_fuera() {
        assert_eq!(normalize_label("  Mi   Etiqueta "), "mi etiqueta");
        let scan = scan_markdown_links("Un [texto] suelto y `[no](x.md)`.\n\n[id]: a.md\n");
        assert!(scan.usages.is_empty(), "el atajo `[texto]` no es un uso");
    }

    #[test]
    fn definiciones_y_usos_respetan_codigo_y_comentarios() {
        let scan =
            scan_markdown_links("```\n[a][b]\n[b]: c.md\n```\n`[x][y]` <!-- [m][n] -->\n[o][p]\n");
        assert!(scan.definitions.is_empty());
        assert_eq!(scan.usages.len(), 1);
        assert_eq!(scan.usages[0].label, "p");
    }

    #[test]
    fn enlaces_e_imagenes_con_titulo_y_fragmento() {
        let source = "Ve a [Arquitectura](architecture.md#database) y ![Diagrama](<img/diagram.png> \"D\").\n";
        assert_eq!(
            dests(source),
            [
                (RefKind::MarkdownLink, "architecture.md#database".to_owned()),
                (RefKind::MarkdownImage, "img/diagram.png".to_owned()),
            ]
        );
    }

    #[test]
    fn definiciones_de_enlace_por_referencia() {
        let source = "[texto][id]\n\n[id]: cap1/doc.md \"Título\"\n";
        assert_eq!(
            dests(source),
            [(RefKind::MarkdownLink, "cap1/doc.md".to_owned())]
        );
    }

    #[test]
    fn wiki_links_19_3() {
        let source = "Ver [[architecture]] y [[architecture#database]].\n";
        assert_eq!(
            dests(source),
            [
                (RefKind::WikiLink, "architecture".to_owned()),
                (RefKind::WikiLink, "architecture#database".to_owned()),
            ]
        );
    }

    #[test]
    fn codigo_y_comentarios_no_son_referencias() {
        let source = "\
```
[n](code.md)
```
`[inline](code.md)` y <!-- [c](comment.md) -->
[f](real.md)
~~~
[t](tachon.md)
~~~
[ni](nada.md)
";
        assert_eq!(
            dests(source),
            [
                (RefKind::MarkdownLink, "real.md".to_owned()),
                (RefKind::MarkdownLink, "nada.md".to_owned()),
            ]
        );
    }

    #[test]
    fn front_matter_yaml_21() {
        let source = "---\nbibliography: refs.bib\nbibliography2: \"otros.bib\"\nassets:\n  - img/foto.png\ntitle: Mi documento\n---\n# Hola\n[n](x.md)\n";
        let refs = find_markdown_references(source);
        let config: Vec<&str> = refs
            .iter()
            .filter(|r| r.kind == RefKind::ConfigPath)
            .map(|r| r.dest.as_str())
            .collect();
        assert_eq!(
            config,
            ["refs.bib", "otros.bib", "img/foto.png", "Mi documento"]
        );
    }

    #[test]
    fn toml_strings_incluidas_en_listas() {
        let source = "# comentario \"falso.md\"\nwiki_links = true\n[[composition]]\nfiles = [\n  \"a.md\", # nota\n  'b.md',\n]\npath = \"c.md\"\n";
        let refs = find_toml_references(source);
        let dests: Vec<&str> = refs.iter().map(|r| r.dest.as_str()).collect();
        assert_eq!(dests, ["a.md", "b.md", "c.md"]);
    }

    #[test]
    fn reescritura_preserva_forma_titulo_y_fragmento() {
        let source = "[a](x.md#frag \"T\") y ![b](<y.png>) y [[z]]\n";
        let mut edits = Vec::new();
        for r in find_markdown_references(source) {
            let replacement = match r.kind {
                RefKind::WikiLink => "nuevo-z".to_owned(),
                _ => format!("sub/{}", r.dest),
            };
            edits.push((r.span, replacement));
        }
        let out = apply_replacements(source, &edits);
        assert_eq!(
            out,
            "[a](sub/x.md#frag \"T\") y ![b](<sub/y.png>) y [[nuevo-z]]\n"
        );
    }

    #[test]
    fn destinos_codificados_se_marcan() {
        let refs = find_markdown_references("[a](mi%20archivo.md) y [b](a\\ b.md)\n");
        assert!(refs.iter().all(|r| r.encoded));
        let refs = find_markdown_references("[a](normal.md)\n");
        assert!(!refs[0].encoded);
    }

    #[test]
    fn enlaces_anidados_y_con_paréntesis() {
        let source = "[![img](assets/i.png)](pagina.md) y [p](dir/con_(paréntesis).md)\n";
        assert_eq!(
            dests(source),
            [
                (RefKind::MarkdownImage, "assets/i.png".to_owned()),
                (RefKind::MarkdownLink, "pagina.md".to_owned()),
                (RefKind::MarkdownLink, "dir/con_(paréntesis).md".to_owned()),
            ]
        );
    }
}

#[cfg(test)]
mod footnote_link_tests {
    use super::*;
    #[test]
    fn footnotes_are_not_link_definitions_but_links_inside_notes_remain() {
        let scan = scan_markdown_links("Texto[^1].\n\n[^1]: Nota.\n");
        assert!(scan.references.is_empty());
        assert!(scan.definitions.is_empty());
        let scan = scan_markdown_links("Texto[^n] y [texto][^n].\n\n[^n]: [fuente](actual.md)\n");
        assert_eq!(scan.references.len(), 1);
        assert_eq!(scan.references[0].dest, "actual.md");
        assert!(scan.usages.is_empty());
    }
}
