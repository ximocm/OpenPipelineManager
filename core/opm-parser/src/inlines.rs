// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Escáner de extensiones inline sobre texto crudo (§7.4).
//!
//! El escáner opera sobre el **fuente crudo** del run de texto, no sobre el
//! texto ya procesado por pulldown-cmark: la regla de escaping §7.4.6 («una
//! `{` precedida de `\` nunca inicia sintaxis extendida») es irrecoverable
//! sobre el texto procesado. A cambio el escáner implementa la capa de texto
//! CommonMark sobre ese run: escapes, character references y saltos de línea.
//!
//! Precedencia (§7.4.7): code spans y fences (fuera de este escáner) →
//! escapes `\` → sustituciones `{{...}}` → colores `{...}` → resto literal.

use super::highlight::{Highlight, HighlightKind};
use super::ParseOptions;
use opm_document::{CitationItem, Inline, Span, Substitution};

/// Paleta física integrada (§23).
pub(crate) const PHYSICAL_COLORS: &[&str] = &[
    "red", "orange", "yellow", "green", "blue", "purple", "gray", "black", "white",
];

/// Paleta semántica integrada (§23).
pub(crate) const SEMANTIC_COLORS: &[&str] = &[
    "danger", "warning", "success", "info", "note", "muted", "highlight", "accent",
];

/// Tipos de bloque destacado integrados (§22).
pub(crate) const BUILTIN_CALLOUTS: &[&str] = &["note", "info", "tip", "warning", "danger", "success"];

pub(crate) fn color_exists(name: &str, options: &ParseOptions) -> bool {
    PHYSICAL_COLORS.contains(&name)
        || SEMANTIC_COLORS.contains(&name)
        || options.custom_colors.iter().any(|c| c == name)
}

pub(crate) fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-')
}

fn is_ascii_punct(c: char) -> bool {
    c.is_ascii_punctuation()
}

/// Escanea un run de texto crudo y produce sus inlines (§7.4).
///
/// Los constructos de extensión reconocidos se registran además en `tokens`
/// (syntax highlighting) con offsets globales `base + offset`.
pub(crate) fn scan_run(
    raw: &str,
    base: usize,
    options: &ParseOptions,
    tokens: &mut Vec<Highlight>,
) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut cur = Cursor::new(raw);

    while let Some(c) = cur.peek() {
        match c {
            '\\' => match cur.peek2() {
                // Backslash + salto de línea = hard break (CommonMark).
                Some('\n') => {
                    cur.eat(2);
                    flush(&mut buf, &mut out);
                    out.push(Inline::HardBreak);
                }
                // §7.4.6: `\{ \} \: \| \\` (y el resto de escapes CommonMark)
                // producen el carácter literal, que nunca inicia sintaxis.
                Some(p) if is_ascii_punct(p) => {
                    buf.push(p);
                    cur.eat(2);
                }
                _ => {
                    buf.push('\\');
                    cur.bump();
                }
            },
            '&' => match decode_char_ref(cur.rest()) {
                // Los character references producen datos literales: un `&#123;`
                // no inicia sintaxis extendida.
                Some((decoded, len)) => {
                    buf.push_str(&decoded);
                    cur.eat(len);
                }
                None => {
                    buf.push('&');
                    cur.bump();
                }
            },
            '\n' => {
                cur.bump();
                // CommonMark: los espacios finales se consumen; 2+ = hard break.
                let mut spaces = 0;
                while buf.ends_with(' ') {
                    buf.pop();
                    spaces += 1;
                }
                flush(&mut buf, &mut out);
                out.push(if spaces >= 2 {
                    Inline::HardBreak
                } else {
                    Inline::SoftBreak
                });
            }
            '{' => {
                if cur.peek2() == Some('{') {
                    match try_substitution(cur.rest()) {
                        Some((sub, len)) => {
                            flush(&mut buf, &mut out);
                            out.push(Inline::Substitution(sub));
                            push_token(tokens, base, cur.pos, len, HighlightKind::Substitution);
                            cur.eat(len);
                        }
                        // `{{foo}}` y similares son texto literal (§7.4.2).
                        None => {
                            buf.push('{');
                            cur.bump();
                        }
                    }
                } else {
                    match try_color(cur.rest(), options) {
                        ColorResult::Valid {
                            foreground,
                            background,
                            text,
                            len,
                        } => {
                            flush(&mut buf, &mut out);
                            out.push(Inline::Color {
                                foreground,
                                background,
                                text,
                            });
                            push_token(tokens, base, cur.pos, len, HighlightKind::Color);
                            cur.eat(len);
                        }
                        // §23: nombre inexistente → toda la construcción es
                        // texto literal, preservada tal cual (§7.4.1).
                        ColorResult::UnknownName { len } => {
                            buf.push_str(&cur.rest()[..len]);
                            cur.eat(len);
                        }
                        ColorResult::Malformed => {
                            buf.push('{');
                            cur.bump();
                        }
                    }
                }
            }
            '[' => {
                let second = cur.peek2();
                if second == Some('^') {
                    // GFM deja literales las notas ausentes. Mantener el nodo
                    // permite diagnosticarlas, también entre segmentos math.
                    if let Some(end) = cur.rest().find(']') {
                        let label = &cur.rest()[2..end];
                        if !label.is_empty() && !label.contains(['\n', '\r', '[']) {
                            flush(&mut buf, &mut out);
                            out.push(Inline::FootnoteReference(opm_document::normalize_label(label)));
                            cur.eat(end + 1);
                            continue;
                        }
                    }
                    buf.push('[');
                    cur.bump();
                } else if second == Some('@') {
                    match try_citation(cur.rest()) {
                        Some((items, len)) => {
                            flush(&mut buf, &mut out);
                            out.push(Inline::Citation { items });
                            push_token(tokens, base, cur.pos, len, HighlightKind::Citation);
                            cur.eat(len);
                        }
                        None => {
                            buf.push('[');
                            cur.bump();
                        }
                    }
                } else if second == Some('[') && options.wiki_links {
                    match try_wiki_link(cur.rest()) {
                        Some((target, len)) => {
                            flush(&mut buf, &mut out);
                            out.push(Inline::WikiLink { target });
                            push_token(tokens, base, cur.pos, len, HighlightKind::WikiLink);
                            cur.eat(len);
                        }
                        None => {
                            buf.push('[');
                            cur.bump();
                        }
                    }
                } else {
                    buf.push('[');
                    cur.bump();
                }
            }
            '$' => match try_math(cur.rest()) {
                Some((latex, len)) => {
                    flush(&mut buf, &mut out);
                    out.push(Inline::Math(latex));
                    push_token(tokens, base, cur.pos, len, HighlightKind::Math);
                    cur.eat(len);
                }
                None => {
                    buf.push('$');
                    cur.bump();
                }
            },
            _ => {
                buf.push(c);
                cur.bump();
            }
        }
    }
    flush(&mut buf, &mut out);
    out
}

fn flush(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        out.push(Inline::Text(std::mem::take(buf)));
    }
}

/// Registra un token de highlighting con offsets globales.
pub(crate) fn push_token(
    tokens: &mut Vec<Highlight>,
    base: usize,
    start: usize,
    len: usize,
    kind: HighlightKind,
) {
    tokens.push(Highlight {
        span: Span {
            start: (base + start) as u32,
            end: (base + start + len) as u32,
        },
        kind,
    });
}

/// Cursor sobre el texto crudo.
struct Cursor<'a> {
    raw: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(raw: &'a str) -> Self {
        Self { raw, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.raw[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek2(&self) -> Option<char> {
        let mut chars = self.rest().chars();
        chars.next();
        chars.next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, bytes: usize) {
        self.pos += bytes;
    }
}

/// Sustituciones `{{...}}` (§7.4.2): vocabulario cerrado.
fn try_substitution(s: &str) -> Option<(Substitution, usize)> {
    let end = s.find("}}")?;
    let inner = &s[2..end];
    let sub = match inner {
        "toc" => Substitution::Toc,
        "pagebreak" => Substitution::PageBreak,
        _ => {
            let id = inner.strip_prefix("ref:")?;
            if id.is_empty() || !id.chars().all(is_id_char) {
                return None;
            }
            Substitution::CrossRef { id: id.to_string() }
        }
    };
    Some((sub, end + 2))
}

enum ColorResult {
    Valid {
        foreground: Option<String>,
        background: Option<String>,
        text: String,
        len: usize,
    },
    UnknownName {
        len: usize,
    },
    Malformed,
}

/// Colores inline (§7.4.4): `{'+'? nombre ('+' nombre)?: texto}`.
fn try_color(s: &str, options: &ParseOptions) -> ColorResult {
    let bytes = s.as_bytes();
    let mut i = 1; // tras '{'

    let parse_name = |i: &mut usize| -> Option<String> {
        let start = *i;
        if !matches!(bytes.get(*i), Some(b'a'..=b'z' | b'A'..=b'Z')) {
            return None;
        }
        while matches!(bytes.get(*i), Some(c) if is_name_byte(*c)) {
            *i += 1;
        }
        Some(s[start..*i].to_string())
    };

    let (foreground, background) = if bytes.get(i) == Some(&b'+') {
        i += 1;
        let name = match parse_name(&mut i) {
            Some(name) => name,
            None => return ColorResult::Malformed,
        };
        (None, Some(name))
    } else {
        let first = match parse_name(&mut i) {
            Some(name) => name,
            None => return ColorResult::Malformed,
        };
        if bytes.get(i) == Some(&b'+') {
            i += 1;
            let second = match parse_name(&mut i) {
                Some(name) => name,
                None => return ColorResult::Malformed,
            };
            (Some(first), Some(second))
        } else {
            (Some(first), None)
        }
    };

    if bytes.get(i) != Some(&b':') {
        return ColorResult::Malformed;
    }
    i += 1;
    // Separador `:` con un espacio opcional tras él (§23).
    if bytes.get(i) == Some(&b' ') {
        i += 1;
    }

    // Texto hasta el primer `}` sin escapar; `\}` y `\:` escriben literales.
    let mut text = String::new();
    loop {
        let rest = &s[i..];
        let c = match rest.chars().next() {
            Some(c) => c,
            // Sin `}` que cierre: no es una construcción válida.
            None => return ColorResult::Malformed,
        };
        match c {
            '}' => {
                i += 1;
                break;
            }
            '\\' => {
                let next = rest[1..].chars().next();
                match next {
                    Some(p) if is_ascii_punct(p) => {
                        text.push(p);
                        i += 1 + p.len_utf8();
                    }
                    _ => {
                        text.push('\\');
                        i += 1;
                    }
                }
            }
            _ => {
                text.push(c);
                i += c.len_utf8();
            }
        }
    }

    let names_ok = [&foreground, &background].iter().all(|name| {
        name.as_ref()
            .map_or(true, |n| color_exists(n, options))
    });
    if !names_ok {
        return ColorResult::UnknownName { len: i };
    }

    ColorResult::Valid {
        foreground,
        background,
        text,
        len: i,
    }
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')
}

/// Citas estilo Pandoc (§25.2): `[@a]`, `[@a; @b]`, `[@a, p. 42]`.
fn try_citation(s: &str) -> Option<(Vec<CitationItem>, usize)> {
    let end = s.find(']')?;
    let inner = &s[1..end];
    let mut items = Vec::new();
    for part in inner.split(';') {
        let part = part.trim();
        let body = part.strip_prefix('@')?;
        if body.is_empty() {
            return None;
        }
        let (key, locator) = match body.split_once(',') {
            Some((key, locator)) => {
                let locator = locator.trim();
                (key.trim(), if locator.is_empty() { None } else { Some(locator) })
            }
            None => (body.trim(), None),
        };
        if key.is_empty()
            || !key
                .chars()
                .all(|c| is_id_char(c) || matches!(c, '*' | '+' | '/' | '!'))
        {
            return None;
        }
        items.push(CitationItem {
            key: key.to_string(),
            locator: locator.map(str::to_string),
        });
    }
    if items.is_empty() {
        return None;
    }
    Some((items, end + 1))
}

/// Wiki links (§19.3): `[[architecture]]`, `[[architecture#database]]`.
fn try_wiki_link(s: &str) -> Option<(String, usize)> {
    let end = s[2..].find("]]")? + 2;
    let target = &s[2..end];
    if target.is_empty() || target.contains(['[', ']', '\n']) {
        return None;
    }
    Some((target.to_string(), end + 2))
}

/// Math inline (§24): `$E = mc^2$`.
fn try_math(s: &str) -> Option<(String, usize)> {
    let mut chars = s.char_indices();
    let (first_pos, _) = chars.next()?;
    debug_assert_eq!(first_pos, 0);
    let (_, second) = chars.next()?;
    // `$$` no abre math inline (el bloque es `$$ ... $$`, §24).
    if second == '$' || second.is_whitespace() {
        return None;
    }
    let body_start = second.len_utf8();
    let rest = &s[body_start..];
    let close = rest.find('$')?;
    let body = &rest[..close];
    // Cuerpo no vacío, sin saltos de línea ni `$`, y sin espacio antes del `$`
    // de cierre (reglas habituales para no capturar divisas).
    if body.is_empty() || body.contains('\n') || body.ends_with(char::is_whitespace) {
        return None;
    }
    Some((body.to_string(), body_start + close + 1))
}

/// Character references (`&amp;`, `&#123;`, `&#x7b;`): decodificación CommonMark
/// de los entities XML y de tipografía más comunes; numéricos completos. Un
/// `&name;` desconocido se preserva literal (igual que CommonMark).
fn decode_char_ref(s: &str) -> Option<(String, usize)> {
    let end = s.find(';')?;
    if end > 12 {
        return None;
    }
    let body = &s[1..end];
    let decoded = if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
        let value = u32::from_str_radix(hex, 16).ok()?;
        char::from_u32(value)?.to_string()
    } else if let Some(dec) = body.strip_prefix('#') {
        let value: u32 = dec.parse().ok()?;
        char::from_u32(value)?.to_string()
    } else {
        named_entity(body)?.to_string()
    };
    Some((decoded, end + 1))
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "mdash" => "—",
        "ndash" => "–",
        "hellip" => "…",
        "ldquo" => "“",
        "rdquo" => "”",
        "lsquo" => "‘",
        "rsquo" => "’",
        "bull" => "•",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "deg" => "°",
        "times" => "×",
        "middot" => "·",
        "dagger" => "†",
        "permil" => "‰",
        "laquo" => "«",
        "raquo" => "»",
        "sect" => "§",
        "para" => "¶",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> ParseOptions<'static> {
        ParseOptions::default()
    }

    fn text(raw: &str) -> Vec<Inline> {
        scan_run(raw, 0, &options(), &mut Vec::new())
    }

    #[test]
    fn escapes_7_4_6() {
        // `\{danger: x}` → `{danger: x}` literal.
        assert_eq!(text(r"\{danger: x}"), vec![Inline::Text("{danger: x}".into())]);
        assert_eq!(text(r"\{{toc}}"), vec![Inline::Text("{{toc}}".into())]);
        assert_eq!(text(r"a \} \: \| \\ b"), vec![Inline::Text("a } : | \\ b".into())]);
    }

    #[test]
    fn sustituciones_vocabulario_cerrado() {
        assert_eq!(text("{{toc}}"), vec![Inline::Substitution(Substitution::Toc)]);
        assert_eq!(
            text("{{pagebreak}}"),
            vec![Inline::Substitution(Substitution::PageBreak)]
        );
        assert_eq!(
            text("{{ref:fig:arq}}"),
            vec![Inline::Substitution(Substitution::CrossRef { id: "fig:arq".into() })]
        );
        // Fuera del vocabulario: literal (§7.4.2).
        assert_eq!(text("{{foo}}"), vec![Inline::Text("{{foo}}".into())]);
        assert_eq!(text("{{ref:}}"), vec![Inline::Text("{{ref:}}".into())]);
    }

    #[test]
    fn colores_validos() {
        assert_eq!(
            text("{danger: x}"),
            vec![Inline::Color {
                foreground: Some("danger".into()),
                background: None,
                text: "x".into(),
            }]
        );
        assert_eq!(
            text("{+highlight: x}"),
            vec![Inline::Color {
                foreground: None,
                background: Some("highlight".into()),
                text: "x".into(),
            }]
        );
        assert_eq!(
            text("{accent+highlight: x}"),
            vec![Inline::Color {
                foreground: Some("accent".into()),
                background: Some("highlight".into()),
                text: "x".into(),
            }]
        );
        // Texto hasta el primer `}` sin escapar; `\}` es literal (§23).
        assert_eq!(
            text(r"{danger: a\}b}"),
            vec![Inline::Color {
                foreground: Some("danger".into()),
                background: None,
                text: "a}b".into(),
            }]
        );
    }

    #[test]
    fn colores_invalidos_o_con_nombre_desconocido() {
        // Nombre inexistente: toda la construcción, texto literal (§23).
        assert_eq!(text("{peligro: x}"), vec![Inline::Text("{peligro: x}".into())]);
        // Sin `}` de cierre: no es construcción.
        assert_eq!(text("{danger: x"), vec![Inline::Text("{danger: x".into())]);
        // Variables de plantilla en cuerpo son texto literal (§7.4.5).
        assert_eq!(text("{title}"), vec![Inline::Text("{title}".into())]);
    }

    #[test]
    fn citas_pandoc() {
        assert_eq!(
            text("[@smith2025]"),
            vec![Inline::Citation {
                items: vec![CitationItem { key: "smith2025".into(), locator: None }],
            }]
        );
        assert_eq!(
            text("[@a; @b]"),
            vec![Inline::Citation {
                items: vec![
                    CitationItem { key: "a".into(), locator: None },
                    CitationItem { key: "b".into(), locator: None },
                ],
            }]
        );
        assert_eq!(
            text("[@a, p. 42]"),
            vec![Inline::Citation {
                items: vec![CitationItem { key: "a".into(), locator: Some("p. 42".into()) }],
            }]
        );
        // Mal formada: literal.
        assert_eq!(text("[@]"), vec![Inline::Text("[@]".into())]);
    }

    #[test]
    fn wiki_links_solo_si_estan_activados() {
        assert_eq!(text("[[architecture]]"), vec![Inline::Text("[[architecture]]".into())]);

        let options = ParseOptions {
            wiki_links: true,
            ..Default::default()
        };
        assert_eq!(
            scan_run("[[architecture#database]]", 0, &options, &mut Vec::new()),
            vec![Inline::WikiLink { target: "architecture#database".into() }]
        );
    }

    #[test]
    fn math_inline() {
        assert_eq!(text("La energía es $E = mc^2$."), vec![
            Inline::Text("La energía es ".into()),
            Inline::Math("E = mc^2".into()),
            Inline::Text(".".into()),
        ]);
        // Divisas y `$` sueltos no abren math.
        assert_eq!(text("$20 y $30"), vec![Inline::Text("$20 y $30".into())]);
        assert_eq!(text("$ a$"), vec![Inline::Text("$ a$".into())]);
    }

    #[test]
    fn character_references_producen_texto_literal() {
        assert_eq!(text("&amp;"), vec![Inline::Text("&".into())]);
        // `&#123;` es un `{` de dato: no inicia sintaxis extendida.
        assert_eq!(text("&#123;danger: x}"), vec![Inline::Text("{danger: x}".into())]);
        // Nombre desconocido: se preserva literal.
        assert_eq!(text("&foo;"), vec![Inline::Text("&foo;".into())]);
    }

    #[test]
    fn saltos_de_linea() {
        assert_eq!(
            text("a\nb"),
            vec![
                Inline::Text("a".into()),
                Inline::SoftBreak,
                Inline::Text("b".into()),
            ]
        );
        assert_eq!(
            text("a  \nb"),
            vec![
                Inline::Text("a".into()),
                Inline::HardBreak,
                Inline::Text("b".into()),
            ]
        );
    }
}
