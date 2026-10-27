//! Lexer shell: comentarios, cadenas, palabras clave, variables y comandos.
//!
//! Escáner por líneas con estado mínimo compartido entre líneas:
//!
//! - cadenas `'…'`/`"…"` sin cerrar continúan en la línea siguiente
//!   (multilineas reales de sh) — con `\'` escapable solo en dobles;
//! - `#` al inicio de token (o de línea) → [`SyntaxRole::Comment`] hasta el
//!   fin de la línea (el `#` de `${VAR#pat}` queda dentro del token de
//!   variable y nunca se ve);
//! - `$VAR`, `${VAR}`, `$1`, `$?`… → [`SyntaxRole::Variable`];
//! - palabras clave de control (`if then else elif fi for while do done
//!   case esac in select until function`) → [`SyntaxRole::Keyword`];
//! - la primera palabra de cada orden (inicio de línea o tras `;`/`&`/`|`/
//!   `(`/`)` y tras los keywords que introducen orden) →
//!   [`SyntaxRole::Command`]; las asignaciones `VAR=…` no cuentan como
//!   orden y su valor se restaura al contexto previo tras el espacio;
//! - `;` `&` `|` `(` `)` `<` `>` → [`SyntaxRole::Punct`].

use super::SyntaxRole;

/// Palabras clave de control de POSIX sh/bash (minúsculas, case-sensitive).
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac", "in",
    "select", "until", "function",
];

/// Tras estas palabras el siguiente token es una orden (o no hay orden):
/// `expect_cmd = true`. En `for`/`case`/`in` viene el sujeto, no una orden.
fn keyword_expects_command(keyword: &str) -> bool {
    !matches!(keyword, "for" | "case" | "in")
}

/// Escanea una comilla que continúa desde el byte 0 de la línea (la de
/// cierre puede estar justo ahí, o un escape en el primer byte). Doble: `\`
/// escapa el siguiente byte; simple: la siguiente `'` cierra.
fn scan_continuation(bytes: &[u8], quote: u8) -> (usize, bool) {
    let len = bytes.len();
    let mut i = 0;
    if quote == b'\'' {
        while i < len {
            if bytes[i] == b'\'' {
                return (i + 1, true);
            }
            i += 1;
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

/// Escanea una comilla abierta en `from` (que apunta a la comilla de
/// apertura): devuelve el fin exclusivo y si la comilla cerró en ESTA línea.
fn scan_quote(bytes: &[u8], from: usize, quote: u8) -> (usize, bool) {
    let len = bytes.len();
    let mut i = from + 1;
    if quote == b'\'' {
        while i < len {
            if bytes[i] == b'\'' {
                return (i + 1, true);
            }
            i += 1;
        }
        return (len, false);
    }
    // Doble comilla: `\` escapa el siguiente byte (incluida otra `\`).
    while i < len {
        match bytes[i] {
            b'\\' => i = (i + 2).min(len),
            b'"' => return (i + 1, true),
            _ => i += 1,
        }
    }
    (len, false)
}

/// ¿El byte cierra una palabra (no es constituyente de palabra)?
fn is_word_stop(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' | b'\'' | b'"' | b'$' | b'`' | b'#'
                | b'=' | b'{' | b'}' | b'[' | b']'
        )
}

/// Unidad de recorrido de una línea. `expect_cmd` marca que la próxima
/// palabra es orden; `assign_restore` difiere la restauración de ese
/// contexto hasta el próximo espacio (valor de `VAR=…`).
struct LineScanner {
    /// Offset de byte del inicio de línea dentro del documento.
    base: usize,
    pos: usize,
    expect_cmd: bool,
    assign_restore: Option<bool>,
    spans: Vec<(usize, usize, SyntaxRole)>,
}

impl LineScanner {
    fn push(&mut self, start: usize, end: usize, role: SyntaxRole) {
        if start < end {
            self.spans.push((self.base + start, self.base + end, role));
        }
    }

    fn skip_control_flow(&mut self) {
        // Un operador de control manda sobre cualquier restauración diferida.
        self.assign_restore = None;
        self.expect_cmd = true;
    }

    fn on_whitespace(&mut self) {
        if let Some(previous) = self.assign_restore.take() {
            self.expect_cmd = previous;
        }
    }
}

pub(crate) fn highlight(text: &str) -> Vec<(usize, usize, SyntaxRole)> {
    let mut spans = Vec::new();
    // Comilla sin cerrar que continúa en la próxima línea.
    let mut carry: Option<u8> = None;
    for (offset, line) in super::lines_with_offsets(text) {
        // Fuera de comillas, cada línea nueva empieza una orden (el fin de
        // línea es separador en sh; una continuación `|`/`&&` también deja
        // el contexto en «esperando orden», que es lo mismo).
        let bytes = line.as_bytes();
        let mut scanner = LineScanner {
            base: offset,
            pos: 0,
            expect_cmd: true,
            assign_restore: None,
            spans: Vec::new(),
        };

        if let Some(quote) = carry {
            let (end, closed) = scan_continuation(bytes, quote);
            scanner.push(0, end, SyntaxRole::String);
            scanner.pos = end;
            if !closed {
                spans.extend(scanner.spans);
                continue; // `carry` sigue viva.
            }
            carry = None;
            scanner.expect_cmd = false;
        }

        while scanner.pos < bytes.len() {
            let byte = bytes[scanner.pos];
            if byte.is_ascii_whitespace() {
                scanner.on_whitespace();
                scanner.pos += 1;
                continue;
            }
            // Comentario: `#` al inicio de token (línea o tras espacio).
            if byte == b'#' && (scanner.pos == 0 || bytes[scanner.pos - 1].is_ascii_whitespace()) {
                scanner.push(scanner.pos, bytes.len(), SyntaxRole::Comment);
                scanner.pos = bytes.len();
                break;
            }
            match byte {
                b'\'' | b'"' => {
                    let start = scanner.pos;
                    let (end, closed) = scan_quote(bytes, start, byte);
                    scanner.push(start, end, SyntaxRole::String);
                    scanner.pos = end;
                    scanner.expect_cmd = false;
                    if !closed {
                        carry = Some(byte);
                        break;
                    }
                }
                b'$' => {
                    let start = scanner.pos;
                    scanner.pos += 1;
                    if scanner.pos >= bytes.len() {
                        // `$` final: sin token de variable.
                        continue;
                    }
                    match bytes[scanner.pos] {
                        b'(' => { /* sustitución: el `(` se procesa como operador */ }
                        b'{' => {
                            while scanner.pos < bytes.len() && bytes[scanner.pos] != b'}' {
                                scanner.pos += 1;
                            }
                            if scanner.pos < bytes.len() {
                                scanner.pos += 1;
                            }
                            scanner.push(start, scanner.pos, SyntaxRole::Variable);
                            scanner.expect_cmd = false;
                        }
                        byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                            while scanner.pos < bytes.len()
                                && (bytes[scanner.pos].is_ascii_alphanumeric()
                                    || bytes[scanner.pos] == b'_')
                            {
                                scanner.pos += 1;
                            }
                            scanner.push(start, scanner.pos, SyntaxRole::Variable);
                            scanner.expect_cmd = false;
                        }
                        b'0'..=b'9' | b'?' | b'!' | b'@' | b'#' | b'*' | b'-' => {
                            scanner.pos += 1;
                            scanner.push(start, scanner.pos, SyntaxRole::Variable);
                            scanner.expect_cmd = false;
                        }
                        _ => continue, // `$` suelto antes de otro byte: nada.
                    }
                }
                b';' | b'|' | b'(' | b')' | b'{' | b'}' | b'<' | b'>' => {
                    // `>&N` de redirección: el `&` no cambia el contexto de
                    // orden (es parte de la redirección, no un operador).
                    scanner.push(scanner.pos, scanner.pos + 1, SyntaxRole::Punct);
                    scanner.pos += 1;
                    if byte != b'<' && byte != b'>' {
                        scanner.skip_control_flow();
                    }
                }
                b'&' => {
                    let redirected = scanner.pos > 0 && bytes[scanner.pos - 1] == b'>';
                    scanner.push(scanner.pos, scanner.pos + 1, SyntaxRole::Punct);
                    scanner.pos += 1;
                    if !redirected {
                        scanner.skip_control_flow();
                    }
                }
                b'=' => {
                    // Asignación vista como token: el valor no es orden y el
                    // contexto previo se restaura en el próximo espacio.
                    scanner.on_whitespace(); // por si acaso hay `a = b`
                    scanner.assign_restore = Some(scanner.expect_cmd);
                    scanner.expect_cmd = false;
                    scanner.pos += 1;
                }
                _ => {
                    let start = scanner.pos;
                    while scanner.pos < bytes.len() && !is_word_stop(bytes[scanner.pos]) {
                        scanner.pos += 1;
                    }
                    if start == scanner.pos {
                        // Byte que no inicia palabra (`[`, `]`, `=`, `#`…):
                        // avanza sin colorir.
                        scanner.pos += 1;
                        continue;
                    }
                    let word = &line[start..scanner.pos];
                    let role = classify_word(word, bytes, scanner.pos, &mut scanner.expect_cmd);
                    if let Some(role) = role {
                        scanner.push(start, scanner.pos, role);
                    }
                }
            }
        }
        spans.extend(scanner.spans);
    }
    spans
}

/// Rol de una palabra escaneada: keyword, orden o ninguno. Actualiza
/// `expect_cmd` según la palabra y su contexto (asignaciones incluidas).
fn classify_word(
    word: &str,
    bytes: &[u8],
    end: usize,
    expect_cmd: &mut bool,
) -> Option<SyntaxRole> {
    // Palabras-clave de control.
    if KEYWORDS.contains(&word) {
        *expect_cmd = keyword_expects_command(word);
        return Some(SyntaxRole::Keyword);
    }
    // Asignación `VAR=…`: no es orden; el valor decide después.
    if bytes.get(end) == Some(&b'=') {
        return None;
    }
    // Llaves de expansión sueltas: solo estructura, sin rol de orden.
    if word.bytes().all(|byte| byte == b'{' || byte == b'}') {
        return None;
    }
    if *expect_cmd {
        *expect_cmd = false;
        return Some(SyntaxRole::Command);
    }
    None
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
    fn comentario_cadena_y_comando() {
        let text = "#!/bin/bash\necho \"hola $NAME\"\n";
        let spans = highlight(text);
        let roles: Vec<SyntaxRole> = spans.iter().map(|(_, _, r)| *r).collect();
        assert_eq!(
            roles,
            vec![
                SyntaxRole::Comment,
                SyntaxRole::Command,
                SyntaxRole::String,
            ]
        );
        assert_eq!(span_text(&text, 0), "#!/bin/bash");
        assert_eq!(span_text(&text, 1), "echo");
        // La cadena incluye la variable: dentro de comillas es String.
        assert_eq!(span_text(&text, 2), "\"hola $NAME\"");
    }

    #[test]
    fn keywords_y_flujo_de_comandos() {
        let text = "if [ -f x ]; then\n  process > out 2>&1;\nfi\n";
        let spans = highlight(text);
        let roles: Vec<SyntaxRole> = spans.iter().map(|(_, _, r)| *r).collect();
        assert_eq!(
            roles,
            vec![
                SyntaxRole::Keyword, // if
                SyntaxRole::Command, // [
                SyntaxRole::Punct,   // ;
                SyntaxRole::Keyword, // then
                SyntaxRole::Command, // process
                SyntaxRole::Punct,   // >
                SyntaxRole::Punct,   // 2>&1: >
                SyntaxRole::Punct,   // &
                SyntaxRole::Punct,   // ;
                SyntaxRole::Keyword, // fi
            ]
        );
    }

    #[test]
    fn tuberia_como_comando() {
        let text = "ls -l | grep x || true";
        let spans = highlight(text);
        let commands: Vec<&str> = spans
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::Command)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(commands, vec!["ls", "grep", "true"]);
        // `|` y los dos `|` de `||`.
        assert_eq!(
            spans
                .iter()
                .filter(|(_, _, r)| *r == SyntaxRole::Punct)
                .count(),
            3
        );
    }

    #[test]
    fn variables_fuera_de_comillas() {
        let text = "run ${HOME}/x $1 $? $plain";
        let vars: Vec<&str> = highlight(text)
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::Variable)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(vars, vec!["${HOME}", "$1", "$?", "$plain"]);
    }

    #[test]
    fn asignacion_no_es_comando_y_el_contexto_se_restaura() {
        // `FOO=bar cmd`: `FOO` y `bar` sin rol, `cmd` es la orden.
        let text = "FOO=bar cmd\n";
        let commands: Vec<&str> = highlight(text)
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::Command)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(commands, vec!["cmd"]);

        // Tras `echo` (contexto ya consumido) el `a=b` es asignación común.
        let text = "echo a=b c\n";
        let commands: Vec<&str> = highlight(text)
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::Command)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(commands, vec!["echo"]);
    }

    #[test]
    fn cadena_multilinea_con_comillas_dentro() {
        // Comilla simple sin cerrar continúa en la línea siguiente; las
        // comillas dobles DENTRO de la simple son texto, no cierran nada.
        let text = "echo 'a \"b\" c\ndos'\nnext\n";
        let strings: Vec<&str> = highlight(text)
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::String)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(strings.len(), 2);
        // La primera línea marca desde la comilla de apertura hasta su fin;
        // la segunda, desde el principio hasta la comilla de cierre.
        assert_eq!(strings[0], "'a \"b\" c");
        assert_eq!(strings[1], "dos'");
        // La tercera línea recupera el contexto de comando.
        let commands: Vec<&str> = highlight(text)
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::Command)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(commands, vec!["echo", "next"]);
    }

    #[test]
    fn escape_y_comillas_dentro_de_comillas_dobles() {
        let text = r#"echo "a \"b\" c 'd'""#;
        let spans = highlight(text);
        let string = spans
            .iter()
            .find(|(_, _, r)| *r == SyntaxRole::String)
            .map(|(s, e, _)| &text[*s..*e])
            .unwrap();
        assert_eq!(string, r#""a \"b\" c 'd'""#);
        // Solo una cadena: el escape no cierra antes de tiempo.
        assert_eq!(
            spans
                .iter()
                .filter(|(_, _, r)| *r == SyntaxRole::String)
                .count(),
            1
        );
    }

    #[test]
    fn continuacion_de_comilla_doble_con_escape_en_el_primer_byte() {
        // La línea 2 continúa la comilla: `\"` es escape (no cierra en el
        // byte 1) y tras el cierre `run` vuelve a ser orden.
        let text = "echo \"ab\ncd\\\"ef\" && run\n";
        let spans = highlight(text);
        let strings: Vec<&str> = spans
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::String)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(strings, vec!["\"ab", "cd\\\"ef\""]);
        let commands: Vec<&str> = spans
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::Command)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(commands, vec!["echo", "run"]);
    }

    #[test]
    fn crlf_y_unicode() {
        let text = "echo \"héllo 🧬\"\r\n# coment\r\n";
        let spans = highlight(text);
        for (start, end, _) in &spans {
            assert!(text.is_char_boundary(*start) && text.is_char_boundary(*end));
        }
        assert_eq!(
            spans.iter().map(|(_, _, r)| *r).collect::<Vec<_>>(),
            vec![
                SyntaxRole::Command,
                SyntaxRole::String,
                SyntaxRole::Comment,
            ]
        );
        assert_eq!(span_text(text, 1), "\"héllo 🧬\"");
    }

    #[test]
    fn hash_gluetado_no_es_comentario() {
        // `${VAR#pat}`: el `#` vive dentro del token de variable, y el `#`
        // pegado a una palabra no abre comentario (no lleva espacio).
        let text = "echo ${PATH#*:} file#name\n";
        let roles = roles(text);
        assert_eq!(roles, vec![SyntaxRole::Command, SyntaxRole::Variable]);
        // Ni comentario extra ni segundo comando: `file#name` es un argumento.
        let commands: Vec<&str> = highlight(text)
            .iter()
            .filter(|(_, _, r)| *r == SyntaxRole::Command)
            .map(|(s, e, _)| &text[*s..*e])
            .collect();
        assert_eq!(commands, vec!["echo"]);
    }
}
