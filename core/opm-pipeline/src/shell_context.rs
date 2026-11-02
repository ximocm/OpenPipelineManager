//! Escáner de contextos de shell para la sustitución de placeholders (F1).
//!
//! La sustitución de Python modela comillas simples/dobles y barras
//! invertidas, pero hay contextos cuyo contenido NO debe (o no puede)
//! influir en el estado de comillas:
//!
//! - **comentarios** `#` (a inicio de palabra y sin comillar): un apóstrofo
//!   dentro del comentario corrompía el estado de comillas de la línea
//!   siguiente (el PoC del revisor: `# user's pipeline` + `printf '%s'
//!   {label}` sustituía SIN cotizar);
//! - **sustituciones** `$(...)`, `${...}` y `` `...` ``: el contexto interno
//!   es independiente del exterior — un placeholder ahí no se cotiza bien;
//! - **cuerpos de heredoc** (`<<EOF` … `EOF`): son datos; un valor con una
//!   línea igual al delimitador puede cerrar el heredoc prematuramente.
//!
//! [`ShellScanner`] es la máquina de estados única que decide, paso a paso,
//! qué se copia verbatim, qué placeholder se sustituye (y con qué contexto
//! de comillas) y qué placeholder hay que RECHAZAR
//! ([`ScanStep::Unsupported`] → `ExecError::UnsupportedContext` en
//! `opm-exec`). [`unsupported_context_placeholder`] es la vista de solo
//! rechazo que usa la validación para bloquear antes de ejecutar.

/// Devuelve el PRIMER placeholder (`{name}`, misma regex que
/// [`crate::placeholders`]) situado en un contexto de shell donde OPM no
/// puede sustituir con seguridad: comentario `#`, sustitución
/// `$()`/`${...}`/backtick o cuerpo de heredoc. `None` = nada que rechazar.
pub fn unsupported_context_placeholder(command: &str) -> Option<String> {
    let mut scanner = ShellScanner::new(command);
    while let Some(step) = scanner.next_step() {
        if let ScanStep::Unsupported { name } = step {
            return Some(name);
        }
    }
    None
}

/// Contexto de comillas de un placeholder sustituible (paridad con el
/// `QuoteContext` del bucle de sustitución de `opm-exec`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ShellQuote {
    Unquoted,
    Single,
    Double,
}

/// Qué hace el caller con el tramo siguiente del comando.
///
/// Cada paso avanza exactamente `len` caracteres (salvo
/// [`ScanStep::Unsupported`], terminal): el caller copia/sustituye y avanza
/// su propio cursor en sincronía.
#[derive(Debug, PartialEq)]
pub enum ScanStep {
    /// Copiar estos caracteres literalmente (el estado del escáner ya se
    /// actualizó: comillas, escapes, palabras — o nada, en comentario/cuerpo
    /// de heredoc, que NO alteran el estado).
    Copy(usize),
    /// Placeholder `{name}` de `len` caracteres en el comando principal con
    /// el contexto de comillas dado: sustituir con la cotización de ese
    /// contexto.
    Substitute {
        name: String,
        len: usize,
        quote: ShellQuote,
    },
    /// Placeholder en un contexto no sustituible (comentario, `$()`,
    /// backtick o heredoc): abortar la sustitución con error.
    Unsupported { name: String },
}

/// Escáner contexto-consciente paso a paso (F1).
///
/// Un solo estado sirve a la sustitución (`opm-exec`) y a la validación:
/// nunca hay dos lecturas distintas del mismo comando.
#[derive(Debug)]
pub struct ShellScanner {
    chars: Vec<char>,
    cursor: usize,
    /// Pila de frames; la base es `Top` y nunca se popúa.
    frames: Vec<Frame>,
    /// Inicio de palabra (para el `#` de comentario): entrada, whitespace y
    /// operadores lo reinician.
    at_word_start: bool,
    /// Heredocs pendientes: su cuerpo empieza en el `\n` de la línea actual.
    pending_heredocs: Vec<Heredoc>,
}

impl ShellScanner {
    pub fn new(command: &str) -> Self {
        ShellScanner {
            chars: command.chars().collect(),
            cursor: 0,
            frames: vec![Frame::new(FrameKind::Top)],
            at_word_start: true,
            pending_heredocs: Vec::new(),
        }
    }

    /// Siguiente paso; `None` al final del comando.
    pub fn next_step(&mut self) -> Option<ScanStep> {
        if self.cursor >= self.chars.len() {
            return None;
        }
        let i = self.cursor;
        let current = self.chars[i];
        let top = self.frames.last().expect("base frame never popped");
        let (kind, quote) = (top.kind, top.quote);

        // (a) dentro de backticks: crudo hasta el cierre (escapes incluidos).
        if kind == FrameKind::Backtick {
            if let Some((key, _)) = match_placeholder(&self.chars, i) {
                return Some(ScanStep::Unsupported { name: key });
            }
            let n = match current {
                '\\' if i + 1 < self.chars.len() => 2,
                '`' => {
                    self.frames.pop();
                    1
                }
                _ => 1,
            };
            return Some(self.advance(n));
        }

        // (b) comentario: `#` a inicio de palabra y sin comillar. Se copia
        // verbatim SIN tocar el estado de comillas (el PoC del revisor).
        if quote == ShellQuote::Unquoted && self.at_word_start && current == '#' {
            let mut j = i;
            while j < self.chars.len() && self.chars[j] != '\n' {
                if let Some((key, _)) = match_placeholder(&self.chars, j) {
                    return Some(ScanStep::Unsupported { name: key });
                }
                j += 1;
            }
            return Some(self.advance(j - i));
        }

        // (c) introducción de heredoc (`<<` sin cotexto; `<<<` es
        // here-string y se traga entero).
        if quote == ShellQuote::Unquoted && current == '<' && self.chars.get(i + 1) == Some(&'<')
        {
            if self.chars.get(i + 2) == Some(&'<') {
                self.at_word_start = false;
                return Some(self.advance(3));
            }
            if let Some((delimiter, strip_tabs, end)) =
                parse_heredoc_delimiter(&self.chars, i + 2)
            {
                self.pending_heredocs.push(Heredoc {
                    delimiter,
                    strip_tabs,
                });
                self.at_word_start = false;
                return Some(self.advance(end - i));
            }
            self.at_word_start = false;
            return Some(self.advance(2));
        }

        // (d) placeholder: en el comando principal se sustituye con su
        // contexto; dentro de una sustitución anidada → no seguro.
        if let Some((key, end)) = match_placeholder(&self.chars, i) {
            if self.frames.len() > 1 {
                return Some(ScanStep::Unsupported { name: key });
            }
            let step = ScanStep::Substitute {
                name: key,
                len: end - i,
                quote,
            };
            self.cursor = end;
            self.at_word_start = false;
            return Some(step);
        }

        // (e) abrir sustitución de comando / expansión de parámetro /
        // backticks (nunca dentro de comillas simples).
        if quote != ShellQuote::Single {
            if current == '$' && self.chars.get(i + 1) == Some(&'(') {
                self.frames.push(Frame::new(FrameKind::Paren));
                self.at_word_start = true;
                return Some(self.advance(2));
            }
            if current == '$' && self.chars.get(i + 1) == Some(&'{') {
                // Solo se consume el `$`: la `{` siguiente abre el frame
                // (y puede SER el inicio de un placeholder `{name}`).
                let mut frame = Frame::new(FrameKind::Brace);
                frame.opener_pending = true;
                self.frames.push(frame);
                self.at_word_start = false;
                return Some(self.advance(1));
            }
            if current == '`' {
                self.frames.push(Frame::new(FrameKind::Backtick));
                self.at_word_start = true;
                return Some(self.advance(1));
            }
        }

        // Cierre de línea con heredocs pendientes: el cuerpo empieza aquí y
        // se copia verbatim (con rechazo de placeholders dentro).
        if quote == ShellQuote::Unquoted
            && current == '\n'
            && !self.pending_heredocs.is_empty()
        {
            return match self.consume_heredoc_bodies(i + 1) {
                Ok(next) => {
                    self.at_word_start = true;
                    Some(self.advance(next - i))
                }
                Err(name) => Some(ScanStep::Unsupported { name }),
            };
        }

        // (f) paso ordinario: comillas, escapes, profundidad y palabras.
        let n = self.step(kind);
        Some(self.advance(n))
    }

    fn advance(&mut self, n: usize) -> ScanStep {
        debug_assert!(n > 0, "every step must make progress");
        self.cursor += n;
        ScanStep::Copy(n)
    }

    /// Un paso de escaneo sobre el carácter actual del frame superior.
    /// Devuelve cuántos caracteres consumió (1 o 2 con escape).
    fn step(&mut self, kind: FrameKind) -> usize {
        let i = self.cursor;
        let current = self.chars[i];
        let frame = self.frames.last_mut().expect("base frame never popped");
        match frame.quote {
            ShellQuote::Single => {
                if current == '\'' {
                    frame.quote = ShellQuote::Unquoted;
                }
                self.at_word_start = false;
                1
            }
            ShellQuote::Double => match current {
                '"' => {
                    frame.quote = ShellQuote::Unquoted;
                    self.at_word_start = false;
                    1
                }
                '\\' if i + 1 < self.chars.len() => {
                    self.at_word_start = false;
                    2
                }
                _ => {
                    self.at_word_start = false;
                    1
                }
            },
            ShellQuote::Unquoted => match current {
                '\'' => {
                    frame.quote = ShellQuote::Single;
                    self.at_word_start = false;
                    1
                }
                '"' => {
                    frame.quote = ShellQuote::Double;
                    self.at_word_start = false;
                    1
                }
                '\\' if i + 1 < self.chars.len() => {
                    self.at_word_start = false;
                    2
                }
                '\n' | '\r' | ' ' | '\t' => {
                    self.at_word_start = true;
                    1
                }
                ';' | '&' | '|' | '<' | '>' => {
                    self.at_word_start = true;
                    1
                }
                '(' => {
                    if kind == FrameKind::Paren {
                        frame.depth += 1;
                    }
                    self.at_word_start = true;
                    1
                }
                ')' => {
                    if kind == FrameKind::Paren && frame.depth == 0 {
                        self.frames.pop();
                        // `$(x)rest` sigue siendo la misma palabra: sin
                        // inicio de palabra tras el cierre.
                        self.at_word_start = false;
                        return 1;
                    }
                    if kind == FrameKind::Paren {
                        frame.depth -= 1;
                    }
                    self.at_word_start = true;
                    1
                }
                '{' if frame.opener_pending => {
                    // La `{` que abre `${…}`: ya no hay marcador.
                    frame.opener_pending = false;
                    self.at_word_start = false;
                    1
                }
                '{' => {
                    if kind == FrameKind::Brace {
                        frame.depth += 1;
                    }
                    self.at_word_start = false;
                    1
                }
                '}' => {
                    if kind == FrameKind::Brace && frame.depth == 0 {
                        self.frames.pop();
                        self.at_word_start = false;
                        return 1;
                    }
                    if kind == FrameKind::Brace {
                        frame.depth -= 1;
                    }
                    self.at_word_start = false;
                    1
                }
                _ => {
                    self.at_word_start = false;
                    1
                }
            },
        }
    }

    /// Consume el cuerpo de los heredocs pendientes desde `start` (justo tras
    /// el `\n`). Devuelve la posición de reanudación del escaneo o el
    /// placeholder hallado dentro de un cuerpo.
    fn consume_heredoc_bodies(&mut self, start: usize) -> Result<usize, String> {
        let mut i = start;
        while !self.pending_heredocs.is_empty() {
            if i >= self.chars.len() {
                // Sin `\n` posterior no hay cuerpo que consumir.
                return Ok(i);
            }
            let line_end = self.chars[i..]
                .iter()
                .position(|&c| c == '\n')
                .map(|offset| i + offset)
                .unwrap_or(self.chars.len());
            let line: String = self.chars[i..line_end].iter().collect();
            let heredoc = &self.pending_heredocs[0];
            let candidate = if heredoc.strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line.as_str()
            };
            if candidate == heredoc.delimiter {
                self.pending_heredocs.remove(0);
            } else {
                let mut j = i;
                while j < line_end {
                    if let Some((key, _)) = match_placeholder(&self.chars, j) {
                        return Err(key);
                    }
                    j += 1;
                }
            }
            i = if line_end < self.chars.len() {
                line_end + 1
            } else {
                line_end
            };
        }
        Ok(i)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum FrameKind {
    /// Comando completo (sin anidar).
    Top,
    /// `$(` … `)` — cotexto propio con comillas locales.
    Paren,
    /// `${` … `}` — expansiones de parámetro.
    Brace,
    /// `` ` `` … `` ` `` — contenido crudo hasta el cierre.
    Backtick,
}

#[derive(Debug)]
struct Frame {
    kind: FrameKind,
    quote: ShellQuote,
    /// Paréntesis/llaves anidados dentro del frame (no incluye el delimitador
    /// que abrió el frame).
    depth: usize,
    /// `true` hasta consumir la `{` que abre una expansión `${`.
    opener_pending: bool,
}

impl Frame {
    fn new(kind: FrameKind) -> Self {
        Frame {
            kind,
            quote: ShellQuote::Unquoted,
            depth: 0,
            opener_pending: false,
        }
    }
}

/// Heredoc pendiente: su cuerpo empieza en el `\n` que cierre la línea actual.
#[derive(Debug)]
struct Heredoc {
    delimiter: String,
    strip_tabs: bool,
}

/// `{([A-Za-z_][A-Za-z0-9_-]*)}` anclado en `start` (misma regex que
/// `placeholders` y que la sustitución de `opm-exec`).
fn match_placeholder(chars: &[char], start: usize) -> Option<(String, usize)> {
    if chars.get(start) != Some(&'{') {
        return None;
    }
    let mut cursor = start + 1;
    let first = *chars.get(cursor)?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    cursor += 1;
    while let Some(&next) = chars.get(cursor) {
        if next.is_ascii_alphanumeric() || next == '_' || next == '-' {
            cursor += 1;
        } else {
            break;
        }
    }
    if chars.get(cursor) != Some(&'}') {
        return None;
    }
    let key: String = chars[start + 1..cursor].iter().collect();
    Some((key, cursor + 1))
}

/// Tras `<<` o `<<-` en `start`: `-` opcional, espacios, y la palabra
/// delimitadora (comillas eliminadas — un cuerpo con comillas simples no
/// expande, pero un placeholder en él sigue rechazándose). Devuelve
/// `(delimitador, recorta_tabs, índice final)` o `None` si no hay palabra.
fn parse_heredoc_delimiter(chars: &[char], start: usize) -> Option<(String, bool, usize)> {
    let mut i = start;
    let mut strip_tabs = false;
    if chars.get(i) == Some(&'-') {
        strip_tabs = true;
        i += 1;
    }
    while matches!(chars.get(i), Some(' ') | Some('\t')) {
        i += 1;
    }
    let first = *chars.get(i)?;
    if first == '\'' || first == '"' {
        let mut delimiter = String::new();
        let mut j = i + 1;
        while let Some(&c) = chars.get(j) {
            if c == first {
                if delimiter.is_empty() {
                    return None;
                }
                return Some((delimiter, strip_tabs, j + 1));
            }
            if c == '\n' {
                return None;
            }
            delimiter.push(c);
            j += 1;
        }
        return None;
    }
    let (mut delimiter, mut j) = (String::new(), i);
    if first == '\\' {
        j += 1;
        while let Some(&c) = chars.get(j) {
            if c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>') {
                break;
            }
            delimiter.push(c);
            j += 1;
        }
    } else {
        while let Some(&c) = chars.get(j) {
            if c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>') {
                break;
            }
            delimiter.push(c);
            j += 1;
        }
    }
    if delimiter.is_empty() {
        None
    } else {
        Some((delimiter, strip_tabs, j))
    }
}

#[cfg(test)]
mod tests {
    use super::{ScanStep, ShellQuote, ShellScanner, unsupported_context_placeholder};

    fn find(command: &str) -> Option<String> {
        unsupported_context_placeholder(command)
    }

    #[test]
    fn ordinary_contexts_are_clean() {
        assert_eq!(find("echo {label}"), None);
        assert_eq!(find("echo \"{label}\" '{other}'"), None);
        assert_eq!(find("echo a\\#b {label}"), None);
        assert_eq!(find(r#"printf '%s' "$(date)""#), None);
        // Sin placeholders: cualquier sintaxis es inofensiva.
        assert_eq!(find("# comment with 'quotes and $(stuff)"), None);
        assert_eq!(find("cat <<EOF | grep 'x'\nliteral line\nEOF"), None);
        assert_eq!(find("echo `date +%s`"), None);
    }

    #[test]
    fn placeholder_in_comment_is_reported() {
        assert_eq!(
            find("# it's a note about {label}\necho ok"),
            Some("label".to_string())
        );
        assert_eq!(
            find("echo ok # trailing {label}"),
            Some("label".to_string())
        );
        // `#` a mitad de palabra no abre comentario.
        assert_eq!(find("echo a#b {label}"), None);
    }

    #[test]
    fn placeholder_in_command_substitution_is_reported() {
        assert_eq!(
            find(r#"printf '%s' "$(printf '%s' {label})""#),
            Some("label".to_string())
        );
        assert_eq!(find("echo $(run {label})"), Some("label".to_string()));
        assert_eq!(find("echo `run {label}`"), Some("label".to_string()));
        assert_eq!(find("echo ${label}"), Some("label".to_string()));
        // Anidamiento y comillas locales dentro de `$()`.
        assert_eq!(find("echo $(f \"({label})\")"), Some("label".to_string()));
        // Placeholder fuera de la sustitución: limpio.
        assert_eq!(find("echo $(date) {label}"), None);
        assert_eq!(find(r#"echo "$(date)" {label}"#), None);
    }

    #[test]
    fn placeholder_in_heredoc_body_is_reported() {
        assert_eq!(find("cat <<EOF\n{label}\nEOF"), Some("label".to_string()));
        assert_eq!(
            find("cat <<-EOF\n\t{label}\n\tEOF"),
            Some("label".to_string())
        );
        assert_eq!(
            find("cat <<'EOF'\n{label}\nEOF"),
            Some("label".to_string())
        );
        // Cuerpo sin cerrar: el resto del comando es cuerpo.
        assert_eq!(find("cat <<EOF\n{label}"), Some("label".to_string()));
        // El delimitador y los argumentos de la línea no son cuerpo.
        assert_eq!(find("cat <<EOF {label}"), None);
    }

    /// El contrato de avance: los pasos cubren el comando ENTERO (copia +
    /// sustitución) sin solaparse; el cursor del caller no se desincroniza.
    #[test]
    fn steps_cover_the_whole_command_without_gaps() {
        let command = "# user's pipeline\nprintf '%s' {label} \"$(date)\" <<EOF\nbody\nEOF\n";
        let chars: Vec<char> = command.chars().collect();
        let mut scanner = ShellScanner::new(command);
        let mut cursor = 0;
        let mut substitutions = Vec::new();
        while let Some(step) = scanner.next_step() {
            match step {
                ScanStep::Copy(len) => {
                    assert!(len > 0, "zero-length copy at {cursor}");
                    cursor += len;
                }
                ScanStep::Substitute {
                    name,
                    len,
                    quote: ShellQuote::Unquoted,
                } => {
                    assert!(cursor < chars.len() && chars[cursor] == '{');
                    cursor += len;
                    substitutions.push(name);
                }
                ScanStep::Substitute { name, .. } => panic!("unexpected context for {name}"),
                ScanStep::Unsupported { name } => panic!("unexpected unsupported {name}"),
            }
        }
        assert_eq!(cursor, chars.len());
        assert_eq!(substitutions, ["label"]);
    }

    /// El PoC del revisor a nivel de escáner: el comentario NO corrompe el
    /// estado — `{label}` de la línea 2 se reporta como sustituible sin
    /// comillas, y los apóstrofos del comentario se copian verbatim.
    #[test]
    fn comment_does_not_leak_quote_state_to_the_next_line() {
        let mut scanner = ShellScanner::new("# user's pipeline\nprintf '%s' {label}");
        let mut found = None;
        while let Some(step) = scanner.next_step() {
            if let ScanStep::Substitute { name, quote, .. } = step {
                found = Some((name, quote));
            }
        }
        assert_eq!(found, Some(("label".to_string(), ShellQuote::Unquoted)));
    }
}
