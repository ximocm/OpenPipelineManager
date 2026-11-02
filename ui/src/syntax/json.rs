//! Lexer JSON: claves, cadenas, números, literales y puntuación.
//!
//! Escáner byte a byte (UTF-8 seguro: los delimitadores son ASCII y los
//! bytes de continuación multibyte nunca los igualan):
//!
//! - `"…"` con escapes `\"`/`\\`/`\uXXXX` → [`SyntaxRole::String`], salvo
//!   que tras la cadena venga `:` (ignorando espacios), en cuyo caso es
//!   [`SyntaxRole::Key`] — la regla clásica de resaltado JSON;
//! - `-?d+(.d+)?(e±d)?` → [`SyntaxRole::Number`];
//! - `true`/`false`/`null` con límite de palabra → [`SyntaxRole::Keyword`];
//! - `{}[]:,` → [`SyntaxRole::Punct`].
//!
//! Un JSON inválido no panic: las cadenas sin cerrar se marcan hasta el fin
//! del documento y los bytes inesperados se ignoran.

use super::SyntaxRole;

pub(crate) fn highlight(text: &str) -> Vec<(usize, usize, SyntaxRole)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i < len {
        match bytes[i] {
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                spans.push((i, i + 1, SyntaxRole::Punct));
                i += 1;
            }
            b'"' => {
                let start = i;
                let mut j = i + 1;
                let mut closed = false;
                while j < len {
                    match bytes[j] {
                        b'\\' => j += 2,
                        b'"' => {
                            j += 1;
                            closed = true;
                            break;
                        }
                        _ => j += 1,
                    }
                }
                // Cierre: el final exclusivo queda tras la comilla; sin cierre
                // se consume hasta EOF (JSON roto, sin panic).
                let end = j.min(len);
                let after = {
                    let mut k = end;
                    while k < len && bytes[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    k
                };
                let role = if closed && after < len && bytes[after] == b':' {
                    SyntaxRole::Key
                } else {
                    SyntaxRole::String
                };
                spans.push((start, end, role));
                i = end;
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                if bytes[i] == b'-' {
                    i += 1;
                }
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < len && bytes[i] == b'.' {
                    i += 1;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
                    let mut j = i + 1;
                    if j < len && (bytes[j] == b'+' || bytes[j] == b'-') {
                        j += 1;
                    }
                    if j < len && bytes[j].is_ascii_digit() {
                        i = j;
                        while i < len && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                }
                if i > start + usize::from(bytes[start] == b'-') {
                    spans.push((start, i, SyntaxRole::Number));
                } else {
                    // `-` sin dígitos: no es número, se ignora el guion.
                    i = start + 1;
                }
            }
            b't' | b'f' | b'n' => {
                let word = ["true", "false", "null"]
                    .into_iter()
                    .find(|literal| text[i..].starts_with(literal))
                    .and_then(|literal| {
                        let end = i + literal.len();
                        let boundary = end >= len || !is_word_byte(bytes[end]);
                        boundary.then_some((literal.len(), end))
                    });
                match word {
                    Some((size, end)) => {
                        spans.push((i, end, SyntaxRole::Keyword));
                        i += size;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    spans
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::highlight;
    use crate::syntax::SyntaxRole;

    fn roles(text: &str) -> Vec<SyntaxRole> {
        highlight(text).iter().map(|(_, _, r)| *r).collect()
    }

    fn span_text<'a>(text: &'a str, index: usize) -> &'a str {
        let (start, end, _) = highlight(text)[index];
        &text[start..end]
    }

    #[test]
    fn objeto_con_claves_strings_numeros_y_literales() {
        let text = r#"{"nombre": "Ana", "edad": 3, "pi": -2.5e10, "ok": true, "nada": null}"#;
        assert_eq!(
            roles(text),
            vec![
                SyntaxRole::Punct, // {
                SyntaxRole::Key,   // "nombre"
                SyntaxRole::Punct, // :
                SyntaxRole::String, // "Ana"
                SyntaxRole::Punct, // ,
                SyntaxRole::Key,   // "edad"
                SyntaxRole::Punct, // :
                SyntaxRole::Number, // 3
                SyntaxRole::Punct, // ,
                SyntaxRole::Key,   // "pi"
                SyntaxRole::Punct, // :
                SyntaxRole::Number, // -2.5e10
                SyntaxRole::Punct, // ,
                SyntaxRole::Key,   // "ok"
                SyntaxRole::Punct, // :
                SyntaxRole::Keyword, // true
                SyntaxRole::Punct, // ,
                SyntaxRole::Key,   // "nada"
                SyntaxRole::Punct, // :
                SyntaxRole::Keyword, // null
                SyntaxRole::Punct,   // }
            ]
        );
        assert_eq!(span_text(text, 1), "\"nombre\"");
        assert_eq!(span_text(text, 11), "-2.5e10");
    }

    #[test]
    fn arrays_y_tipos() {
        let text = "[1, 22, 3.14, 1e3, 4E-2]";
        assert_eq!(
            roles(text),
            vec![
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Punct,
                SyntaxRole::Number,
                SyntaxRole::Punct,
            ]
        );
        assert_eq!(span_text(text, 3), "22");
        assert_eq!(span_text(text, 5), "3.14");
        assert_eq!(span_text(text, 7), "1e3");
        assert_eq!(span_text(text, 9), "4E-2");
    }

    #[test]
    fn comillas_escape_y_comillas_dentro_de_comillas() {
        // `\"` cierra el escape, no la cadena: el segundo campo sigue String.
        let text = r#"{"s": "dijo \"hola\"", "k": 1}"#;
        let spans = highlight(text);
        let s = spans
            .iter()
            .find(|(_, _, role)| *role == SyntaxRole::String)
            .map(|(start, end, _)| &text[*start..*end])
            .unwrap();
        assert_eq!(s, r#""dijo \"hola\"""#);
        // La clave tras el objeto-escape es Key.
        assert_eq!(
            spans
                .iter()
                .filter(|(_, _, role)| *role == SyntaxRole::Key)
                .count(),
            2
        );
    }

    #[test]
    fn cadenas_multilinea_escape_y_cjk() {
        // Las cadenas JSON no contienen CR/LF literal, pero un fichero roto
        // con uno dentro no debe partirse ni panic (se marca hasta EOF).
        let text = "{\"a\": \"línea\\nuna\"}\n\"cjk 日本語 🧬\"";
        let spans = highlight(text);
        for (start, end, _) in &spans {
            assert!(text.is_char_boundary(*start) && text.is_char_boundary(*end));
        }
        // La segunda cadena (tras la llave de cierre) es String: no lleva `:`.
        let last = *spans.last().unwrap();
        assert_eq!(last.2, SyntaxRole::String);
        assert_eq!(&text[last.0..last.1], "\"cjk 日本語 🧬\"");
    }

    #[test]
    fn invalidos_no_panican() {
        // Cadena sin cerrar → hasta EOF (el guion suelto no forma número);
        // literal a media palabra → no es keyword.
        let text = r#"-"truex"#;
        let spans = highlight(text);
        assert_eq!(spans, vec![(1, 7, SyntaxRole::String)]);
        assert_eq!(highlight("nul").len(), 0);
        assert_eq!(highlight("").len(), 0);
    }

    #[test]
    fn true_falso_con_limite_de_palabra() {
        // `truex` y `nullable` no son literales: solo queda puntuación.
        let marcada = roles("[truex, nullable]");
        assert!(marcada.iter().all(|role| *role == SyntaxRole::Punct));
        assert_eq!(
            roles("[true]"),
            vec![SyntaxRole::Punct, SyntaxRole::Keyword, SyntaxRole::Punct]
        );
    }
}
