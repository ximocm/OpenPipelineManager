//! Lexer YAML: claves, cadenas, comentarios, números, anclas, marcadores de
//! lista e indicadores de escalar de bloque.
//!
//! Escáner por líneas con tres estados compartidos entre líneas (YAML real):
//!
//! - **comilla abierta** (`'`/`"`) que continúa en la línea siguiente — con
//!   `''` literal en simples y `\` escapado en dobles;
//! - **escalar de bloque** (`|`, `>`): las líneas más indentadas que la línea
//!   del indicador son contenido ([`SyntaxRole::String`]), comentarios y
//!   todo incluidos;
//! - **profundidad de flujos** `[`/`{` sin cerrar, que también puede cruzar
//!   líneas.
//!
//! Por línea: `#` tras espacio → [`SyntaxRole::Comment`]; `- ` →
//! [`SyntaxRole::Marker`]; `&ancla`/`*alias`/`!etiqueta` →
//! [`SyntaxRole::Anchor`]; `key:` (o `"key":`) → [`SyntaxRole::Key`] +
//! [`SyntaxRole::Punct`] del `:`; escalares entrecomillados →
//! [`SyntaxRole::String`]; escalares planos numéricos →
//! [`SyntaxRole::Number`] y el resto → [`SyntaxRole::String`]; `,` `[{ }]`
//! → [`SyntaxRole::Punct`]; `---`/`...` → [`SyntaxRole::Punct`].

use super::SyntaxRole;

/// Contexto de parseo dentro de una línea: `Node` detecta claves (`key:`),
/// `Value` (tras un `:`) escanea el valor sin más claves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Node,
    Value,
}

/// Estado que cruza líneas.
struct State {
    /// Comilla abierta que continúa en la siguiente línea.
    quote: Option<u8>,
    /// Indentación (bytes) de la línea con `|`/`>`: por debajo hay que
    /// cerrar el escalar de bloque.
    block: Option<usize>,
    /// Profundidad de `[`/`{` abiertos sin cerrar.
    flow: usize,
}

/// Fin de un escalar plano.
enum Stop {
    /// `:` separador de clave (solo en [`Mode::Node`]).
    Colon(usize),
    /// Inicio de un comentario.
    Comment(usize),
    /// `,`/`]`/`}` en contexto flujo.
    Flow(usize),
    /// Fin de línea.
    End,
}

pub(crate) fn highlight(text: &str) -> Vec<(usize, usize, SyntaxRole)> {
    let mut spans: Vec<(usize, usize, SyntaxRole)> = Vec::new();
    let mut state = State {
        quote: None,
        block: None,
        flow: 0,
    };
    for (offset, line) in super::lines_with_offsets(text) {
        let bytes = line.as_bytes();
        let len = bytes.len();
        let mut i;
        let mut mode;

        if let Some(quote) = state.quote {
            // Continuación de comilla: la línea es contenido hasta cerrar.
            let (end, closed) = scan_from(bytes, 0, quote);
            push(&mut spans, offset, 0, end, SyntaxRole::String);
            if !closed {
                continue;
            }
            state.quote = None;
            i = end;
            mode = Mode::Value;
        } else {
            let indent = leading_ws(bytes);
            if indent == len {
                // Línea en blanco (dentro de comillas ya se gestionó arriba).
                continue;
            }
            if let Some(min) = state.block {
                if indent > min {
                    // Contenido del escalar de bloque: todo es String.
                    let end = trim_end(bytes);
                    push(&mut spans, offset, indent, end, SyntaxRole::String);
                    continue;
                }
                state.block = None;
            }
            i = indent;
            mode = Mode::Node;
        }

        loop {
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= len {
                break;
            }
            // Comentario: `#` al inicio de línea o tras espacio.
            if bytes[i] == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
                push(&mut spans, offset, i, len, SyntaxRole::Comment);
                break;
            }
            // Marcas de documento `---` / `...` al inicio de línea.
            if mode == Mode::Node && i == 0 && matches!(bytes, [b'-', b'-', b'-', ..] | [b'.', b'.', b'.', ..])
                && (len == 3 || bytes[3].is_ascii_whitespace())
            {
                push(&mut spans, offset, 0, 3, SyntaxRole::Punct);
                i = 3;
                continue;
            }
            let byte = bytes[i];
            match byte {
                b'-' if matches!(bytes.get(i + 1), None | Some(b' ' | b'\t')) => {
                    push(&mut spans, offset, i, i + 1, SyntaxRole::Marker);
                    i += 1;
                    mode = Mode::Node;
                }
                b'[' | b'{' => {
                    push(&mut spans, offset, i, i + 1, SyntaxRole::Punct);
                    state.flow += 1;
                    mode = Mode::Node;
                    i += 1;
                }
                b']' | b'}' => {
                    push(&mut spans, offset, i, i + 1, SyntaxRole::Punct);
                    state.flow = state.flow.saturating_sub(1);
                    mode = Mode::Node;
                    i += 1;
                }
                b',' => {
                    push(&mut spans, offset, i, i + 1, SyntaxRole::Punct);
                    mode = Mode::Node;
                    i += 1;
                }
                b'\'' | b'"' => {
                    let (end, closed) = scan_from(bytes, i + 1, byte);
                    if !closed {
                        push(&mut spans, offset, i, end, SyntaxRole::String);
                        state.quote = Some(byte);
                        break;
                    }
                    if mode == Mode::Node && colon_after(bytes, end) {
                        push(&mut spans, offset, i, end, SyntaxRole::Key);
                        let colon = skip_ws(bytes, end);
                        push(&mut spans, offset, colon, colon + 1, SyntaxRole::Punct);
                        i = colon + 1;
                        mode = Mode::Value;
                    } else {
                        push(&mut spans, offset, i, end, SyntaxRole::String);
                        i = end;
                        mode = Mode::Value;
                    }
                }
                b'&' | b'*' | b'!' => {
                    let start = i;
                    i += 1;
                    while i < len && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b',' | b']' | b'}') {
                        i += 1;
                    }
                    push(&mut spans, offset, start, i, SyntaxRole::Anchor);
                }
                b'|' | b'>'
                    if matches!(bytes.get(i + 1), None | Some(b' ' | b'\t' | b'+' | b'-' | b'0'..=b'9')) =>
                {
                    push(&mut spans, offset, i, i + 1, SyntaxRole::Marker);
                    state.block = Some(leading_ws(bytes).min(len));
                    // Tras el indicador solo caben modificadores y comentario.
                    let mut rest = i + 1;
                    while rest < len && !bytes[rest].is_ascii_whitespace() {
                        rest += 1;
                    }
                    rest = skip_ws(bytes, rest);
                    if rest < len && bytes[rest] == b'#' {
                        push(&mut spans, offset, rest, len, SyntaxRole::Comment);
                    }
                    break;
                }
                _ => {
                    let allow_key = mode == Mode::Node;
                    match scan_plain(bytes, i, state.flow, allow_key) {
                        Stop::Colon(colon) => {
                            let mut end = colon;
                            while end > i && bytes[end - 1].is_ascii_whitespace() {
                                end -= 1;
                            }
                            push(&mut spans, offset, i, end, SyntaxRole::Key);
                            push(&mut spans, offset, colon, colon + 1, SyntaxRole::Punct);
                            i = colon + 1;
                            mode = Mode::Value;
                        }
                        Stop::Comment(at) => {
                            push_scalar(&mut spans, offset, bytes, i, at);
                            i = at;
                        }
                        Stop::Flow(at) => {
                            push_scalar(&mut spans, offset, bytes, i, at);
                            i = at;
                        }
                        Stop::End => {
                            push_scalar(&mut spans, offset, bytes, i, len);
                            i = len;
                        }
                    }
                }
            }
        }
    }
    spans
}

fn push(spans: &mut Vec<(usize, usize, SyntaxRole)>, base: usize, start: usize, end: usize, role: SyntaxRole) {
    if start < end {
        spans.push((base + start, base + end, role));
    }
}

/// Empuja el escalar plano `[start, end)` (recortado de espacios finales):
/// [`SyntaxRole::Number`] si TODO el escalar es numérico, si no String.
fn push_scalar(
    spans: &mut Vec<(usize, usize, SyntaxRole)>,
    base: usize,
    bytes: &[u8],
    start: usize,
    end: usize,
) {
    let mut end = end;
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if start >= end {
        return;
    }
    let role = if is_yaml_number(&bytes[start..end]) {
        SyntaxRole::Number
    } else {
        SyntaxRole::String
    };
    push(spans, base, start, end, role);
}

/// Escanea un escalar plano desde `i`: para en separador de clave (si
/// `allow_key`), comentario, punctuation de flujo o fin de línea.
fn scan_plain(bytes: &[u8], i: usize, flow: usize, allow_key: bool) -> Stop {
    let len = bytes.len();
    let mut i = i;
    while i < len {
        let byte = bytes[i];
        if flow > 0 && matches!(byte, b',' | b']' | b'}') {
            return Stop::Flow(i);
        }
        if byte == b'#' && i > 0 && bytes[i - 1].is_ascii_whitespace() {
            return Stop::Comment(i);
        }
        if allow_key && byte == b':' && (i + 1 >= len || bytes[i + 1].is_ascii_whitespace()) {
            return Stop::Colon(i);
        }
        i += 1;
    }
    Stop::End
}

/// ¿En `i` (o tras espacios) hay un `:` separador?
fn colon_after(bytes: &[u8], i: usize) -> bool {
    let i = skip_ws(bytes, i);
    matches!(bytes.get(i), Some(b':'))
        && matches!(bytes.get(i + 1), None | Some(b' ' | b'\t'))
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn leading_ws(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|&b| !matches!(b, b' ' | b'\t'))
        .unwrap_or(bytes.len())
}

fn trim_end(bytes: &[u8]) -> usize {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

/// Escaneo de comilla desde `start` (justo tras la comilla de apertura en
/// línea propia, o desde 0 en continuación). Simples: `''` es literal; dobles:
/// `\` escapa el siguiente byte. Devuelve fin exclusivo y si cerró.
fn scan_from(bytes: &[u8], start: usize, quote: u8) -> (usize, bool) {
    let len = bytes.len();
    let mut i = start;
    if quote == b'\'' {
        while i < len {
            if bytes[i] == b'\'' {
                if bytes.get(i + 1) == Some(&b'\'') {
                    i += 2;
                } else {
                    return (i + 1, true);
                }
            } else {
                i += 1;
            }
        }
        return (len, false);
    }
    while i < len {
        match bytes[i] {
            b'\\' => i = (i + 2).min(len),
            b'"' => return (i + 1, true),
            _ => i += 1,
        }
    }
    (len, false)
}

/// ¿Todo el texto es un número YAML (entero o float con signo/exponente)?
fn is_yaml_number(bytes: &[u8]) -> bool {
    let len = bytes.len();
    if len == 0 {
        return false;
    }
    let mut i = 0;
    if bytes[i] == b'+' || bytes[i] == b'-' {
        i += 1;
    }
    let mut digits = 0usize;
    while i < len && bytes[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < len && bytes[i] == b'.' {
        i += 1;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if i < len && matches!(bytes[i], b'e' | b'E') {
        i += 1;
        if i < len && matches!(bytes[i], b'+' | b'-') {
            i += 1;
        }
        let mut exponent = 0usize;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
            exponent += 1;
        }
        if exponent == 0 {
            return false;
        }
    }
    i == len
}

#[cfg(test)]
mod tests {
    use super::{highlight, is_yaml_number};
    use crate::syntax::SyntaxRole;

    fn roles(text: &str) -> Vec<SyntaxRole> {
        highlight(text).iter().map(|(_, _, r)| *r).collect()
    }

    fn span_text<'a>(text: &'a str, index: usize) -> &'a str {
        let (start, end, _) = highlight(text)[index];
        &text[start..end]
    }

    #[test]
    fn claves_valores_comentarios_y_numeros() {
        let text = "# notas\nname: Ana\ncount: 42\nratio: -3.14e2\nenabled: true\n";
        assert_eq!(
            roles(text),
            vec![
                SyntaxRole::Comment,
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::String,
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::String, // `true` es escalar plano en YAML
            ]
        );
        assert_eq!(span_text(text, 3), "Ana");
        assert_eq!(span_text(text, 6), "42");
        assert_eq!(span_text(text, 9), "-3.14e2");
    }

    #[test]
    fn marcadores_de_lista_y_anidacion() {
        let text = "items:\n  - one\n  - two: 2\n";
        assert_eq!(
            roles(text),
            vec![
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::Marker,
                SyntaxRole::String,
                SyntaxRole::Marker,
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::Number,
            ]
        );
        assert_eq!(span_text(text, 3), "one");
    }

    #[test]
    fn anclas_aliases_y_etiquetas() {
        let text = "base: &defaults\nnext: *defaults\nkind: !!str\n";
        let anchors: Vec<&str> = highlight(text)
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::Anchor)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(anchors, vec!["&defaults", "*defaults", "!!str"]);
    }

    #[test]
    fn escalares_de_bloque_con_conmutadores() {
        let text = "text: |-\n  línea uno\n  línea dos\nnext: 1\n";
        assert_eq!(
            roles(text),
            vec![
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::Marker, // |
                SyntaxRole::String, // contenido
                SyntaxRole::String, // contenido
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::Number,
            ]
        );
        assert_eq!(span_text(text, 3), "línea uno");
        assert_eq!(span_text(text, 4), "línea dos");
    }

    #[test]
    fn contenido_de_bloque_con_hash_no_es_comentario() {
        let text = "sh: |\n  echo hi # no comentario\n";
        assert_eq!(
            roles(text),
            vec![SyntaxRole::Key, SyntaxRole::Punct, SyntaxRole::Marker, SyntaxRole::String]
        );
    }

    #[test]
    fn cadena_multilinea_doble_con_escape() {
        let text = "msg: \"started\n  fin\\\"ished\" # tras\ntab: 1\n";
        let strings: Vec<&str> = highlight(text)
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::String)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(strings.len(), 2);
        assert_eq!(strings[0], "\"started");
        assert_eq!(strings[1], "  fin\\\"ished\"");
        // El comentario tras el cierre y la línea siguiente se resuelven.
        assert_eq!(
            roles(text),
            vec![
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::String,
                SyntaxRole::String,
                SyntaxRole::Comment,
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::Number,
            ]
        );
    }

    #[test]
    fn comillas_simples_con_escape_de_doble_comilla() {
        // `''` es una comilla literal DENTRO de la simple.
        let text = "msg: 'it''s fine'\n";
        assert_eq!(
            roles(text),
            vec![SyntaxRole::Key, SyntaxRole::Punct, SyntaxRole::String]
        );
        assert_eq!(span_text(text, 2), "'it''s fine'");
    }

    #[test]
    fn flujo_con_anidamiento_y_clave_json() {
        let text = "{\"a\": 1, \"b\": [2, 3]}\n";
        let spans = highlight(text);
        let roles: Vec<SyntaxRole> = spans.iter().map(|(_, _, r)| *r).collect();
        assert_eq!(
            roles,
            vec![
                SyntaxRole::Punct, // {
                SyntaxRole::Key,   // "a"
                SyntaxRole::Punct, // :
                SyntaxRole::Number,
                SyntaxRole::Punct, // ,
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::Punct, // [
                SyntaxRole::Number,
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Punct, // ]
                SyntaxRole::Punct, // }
            ]
        );
        assert_eq!(span_text(text, 1), "\"a\"");
    }

    #[test]
    fn flujo_multilinea_y_crlf() {
        let text = "a: [1,\r\n  2]\r\nb: x\r\n";
        let roles = roles(text);
        assert_eq!(
            roles,
            vec![
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Punct,
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::String,
            ]
        );
    }

    #[test]
    fn claves_con_espacios_y_valores_con_dos_puntos() {
        let text = "\"my key\": v\nurl: http://example.com\n";
        assert_eq!(
            roles(text),
            vec![
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::String,
                SyntaxRole::Key,
                SyntaxRole::Punct,
                SyntaxRole::String,
            ]
        );
        // El `:` de la URL no lleva espacio: no corta el valor.
        assert_eq!(span_text(text, 5), "http://example.com");
    }

    #[test]
    fn numeros_yaml() {
        assert!(is_yaml_number(b"42"));
        assert!(is_yaml_number(b"-7"));
        assert!(is_yaml_number(b"+3.5"));
        assert!(is_yaml_number(b"1e10"));
        assert!(is_yaml_number(b"-2.5E-3"));
        assert!(is_yaml_number(b"0"));
        assert!(!is_yaml_number(b""));
        assert!(!is_yaml_number(b"-"));
        assert!(!is_yaml_number(b"1.2.3"));
        assert!(!is_yaml_number(b"0x1f"));
        assert!(!is_yaml_number(b"1e"));
        assert!(!is_yaml_number(b"abc"));
    }

    #[test]
    fn texto_vacio_y_comentarios_solo() {
        assert!(highlight("").is_empty());
        assert_eq!(highlight("# c").len(), 1);
        assert!(highlight("\n\n").is_empty());
    }
}
