// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Búsqueda y reemplazo en el archivo actual (§40.1, ISSUE-027).
//!
//! [`Matcher`] cubre los modos §40.1 — *case sensitive*, *whole word* y
//! *regex* (motor propio std-only, [`crate::regex`]) — sobre el contenido del
//! [`Editor`], devolviendo rangos en posiciones semánticas (§8.6) y con
//! navegación siguiente/anterior **con wrap** (§40.1).
//!
//! El reemplazo no vive aquí: se aplica con [`Editor::replace`] (una
//! coincidencia = una transacción) y [`Editor::replace_all`] (todas en una
//! sola transacción) sobre el historial Undo/Redo único (§8.5).
//!
//! Las coincidencias vacías (patrones tipo `x*` sobre texto sin `x`) se
//! descartan: reemplazarlas sería insertar el texto en cada posición.

use crate::editor::Editor;
use crate::position::Range;
use crate::regex::{char_at, char_before, is_word_char, Regex};
use std::fmt;

/// Opciones de búsqueda (§40.1: case sensitive, whole word, regex).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOptions {
    /// Distinguir mayúsculas y minúsculas («Aa»).
    pub case_sensitive: bool,
    /// Solo coincidencias entre límites de palabra («palabra»).
    pub whole_word: bool,
    /// `pattern` es una expresión regular del subconjunto soportado («.*»).
    pub regex: bool,
}

/// Error de búsqueda (§40.1 criterio 4): nunca panic ni bloqueo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    /// Expresión regular inválida; el mensaje describe el fallo.
    InvalidRegex(String),
    /// Presupuesto de simulación agotado (§47.5: la búsqueda no puede
    /// colgar la UI); se informa al usuario y se conserva la búsqueda
    /// anterior válida.
    TooComplex(String),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRegex(message) => write!(f, "regex inválida: {message}"),
            Self::TooComplex(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SearchError {}

/// Patrón compilado según las opciones.
#[derive(Debug)]
enum Pattern {
    /// Texto literal (el motor regex no interviene).
    Literal(String),
    Regex(Regex),
}

/// Coincidencia sobre texto plano con offsets de bytes (nivel texto para
/// consumidores externos, p. ej. la búsqueda de proyecto).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextMatch {
    /// Inicio en bytes (frontera de carácter).
    pub start: usize,
    /// Fin en bytes (frontera de carácter, exclusivo).
    pub end: usize,
    /// Pares `(inicio, fin)` por grupo: `groups[0]` es la coincidencia
    /// completa; el resto, los grupos de captura del regex (para `$1`..`$9`
    /// en los reemplazos). Un grupo que no participó es `None`. En modo
    /// literal solo hay `groups[0]`.
    pub groups: Vec<Option<(usize, usize)>>,
}

/// Buscador del archivo actual (§40.1) sobre el buffer del [`Editor`].
#[derive(Debug)]
pub struct Matcher {
    pattern: Pattern,
    options: SearchOptions,
}

impl Matcher {
    /// Compila `pattern` con `options`. Un patrón vacío es válido y no
    /// produce coincidencias; una regex inválida es [`SearchError`].
    pub fn new(pattern: &str, options: SearchOptions) -> Result<Self, SearchError> {
        let compiled = if options.regex {
            Pattern::Regex(Regex::parse(pattern, !options.case_sensitive)?)
        } else {
            Pattern::Literal(pattern.to_string())
        };
        Ok(Self {
            pattern: compiled,
            options,
        })
    }

    /// Todas las coincidencias en orden, como rangos semánticos (§8.6).
    pub fn find_all(&self, editor: &Editor) -> Result<Vec<Range>, SearchError> {
        let text = editor.document().content();
        let (found, truncated) = self.scan_text(text);
        if let Some(err) = truncated {
            return Err(err);
        }
        Ok(found
            .into_iter()
            .map(|found| {
                Range::new(
                    editor.position_of(found.start),
                    editor.position_of(found.end),
                )
            })
            .collect())
    }

    /// Todas las coincidencias sobre `text` plano con offsets de bytes
    /// (ISSUE-028: nivel texto, para ficheros sin [`Editor`]) y la misma
    /// semántica case/whole-word/regex que [`Matcher::find_all`].
    ///
    /// Si una coincidencia agota el presupuesto de simulación del motor
    /// (§47.5: solo patologías), la exploración se detiene y se devuelven
    /// las coincidencias halladas hasta ese punto.
    pub fn find_all_in_text(&self, text: &str) -> Vec<TextMatch> {
        self.scan_text(text).0
    }

    /// Expansión del reemplazo (ISSUE-028) sobre `text` según los grupos de
    /// `m`: `$$` → `$` y `$0`..`$9` → el texto del grupo (vacío si no
    /// participó o no existe); `$` seguido de otra cosa es literal.
    ///
    /// En modo literal solo se procesa `$$` (el template sale tal cual).
    pub fn expand_replacement(&self, text: &str, m: &TextMatch, template: &str) -> String {
        let mut out = String::new();
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '$' {
                out.push(c);
                continue;
            }
            match chars.peek().copied() {
                Some('$') => {
                    chars.next();
                    out.push('$');
                }
                Some(digit) if self.options.regex && digit.is_ascii_digit() => {
                    chars.next();
                    let group = digit.to_digit(10).expect("dígito ASCII") as usize;
                    if let Some((start, end)) = m.groups.get(group).copied().flatten() {
                        if let Some(piece) = text.get(start..end) {
                            out.push_str(piece);
                        }
                    }
                }
                _ => out.push('$'),
            }
        }
        out
    }

    /// Siguiente coincidencia respecto de la selección actual (§8.6), con
    /// **wrap** al principio del buffer (§40.1). Con selección vacía se
    /// incluye la coincidencia que empieza en el cursor.
    pub fn find_next(&self, editor: &Editor) -> Result<Option<Range>, SearchError> {
        let matches = self.find_all(editor)?;
        let selection = editor.selection();
        let from = selection.range().start;
        let hit = if selection.is_empty() {
            matches.iter().find(|m| m.start >= from)
        } else {
            matches.iter().find(|m| m.start > from)
        };
        Ok(hit.or_else(|| matches.first()).copied())
    }

    /// Coincidencia anterior respecto de la selección actual, con **wrap** al
    /// final del buffer (§40.1).
    pub fn find_previous(&self, editor: &Editor) -> Result<Option<Range>, SearchError> {
        let matches = self.find_all(editor)?;
        let from = editor.selection().range().start;
        let hit = matches.iter().rev().find(|m| m.start < from);
        Ok(hit.or_else(|| matches.last()).copied())
    }

    /// Coincidencias como [`TextMatch`] y, si se agotó el presupuesto de
    /// simulación, el error que truncó la exploración.
    fn scan_text(&self, text: &str) -> (Vec<TextMatch>, Option<SearchError>) {
        let whole_word = self.options.whole_word;
        let mut matches = Vec::new();
        let mut truncated = None;
        let mut at = 0;
        'scan: while at < text.len() {
            let found = match &self.pattern {
                Pattern::Literal(pattern) if !self.options.case_sensitive => {
                    literal_match_at(text, at, pattern, true).map(|end| TextMatch {
                        start: at,
                        end,
                        groups: vec![Some((at, end))],
                    })
                }
                Pattern::Literal(pattern) => text[at..]
                    .starts_with(pattern.as_str())
                    .then(|| at + pattern.len())
                    .map(|end| TextMatch {
                        start: at,
                        end,
                        groups: vec![Some((at, end))],
                    }),
                Pattern::Regex(regex) => match regex.find_at_full(text, at) {
                    Ok(found) => found.map(|sub| TextMatch {
                        start: at,
                        end: sub.end,
                        groups: sub.groups,
                    }),
                    Err(err) => {
                        truncated = Some(err);
                        break 'scan;
                    }
                },
            };
            if let Some(found) = found {
                if found.end > found.start
                    && (!whole_word || word_limits(text, found.start, found.end))
                {
                    at = found.end;
                    matches.push(found);
                    continue;
                }
            }
            match char_at(text, at) {
                Some((_, next)) => at = next,
                None => break,
            }
        }
        (matches, truncated)
    }
}

/// `whole_word` (§40.1): el rango `[start, end)` cae entre límites de
/// palabra (los extremos del buffer también son límite).
fn word_limits(text: &str, start: usize, end: usize) -> bool {
    let before = char_before(text, start)
        .map(|(c, _)| is_word_char(c))
        .unwrap_or(false);
    let after = char_at(text, end)
        .map(|(c, _)| is_word_char(c))
        .unwrap_or(false);
    !before && !after
}

/// Coincidencia literal anclada en `start`; `fold` = sin distinguir
/// mayúsculas (equivalencia por carácter, sin plegado Unicode completo).
fn literal_match_at(text: &str, start: usize, pattern: &str, fold: bool) -> Option<usize> {
    if pattern.is_empty() {
        return None;
    }
    let mut at = start;
    for expected in pattern.chars() {
        let (c, next) = char_at(text, at)?;
        if !chars_equal(c, expected, fold) {
            return None;
        }
        at = next;
    }
    Some(at)
}

fn chars_equal(a: char, b: char, fold: bool) -> bool {
    a == b || (fold && a.to_lowercase().eq(b.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::position::Position;

    fn editor(text: &str) -> Editor {
        Editor::new(Document::from_text(None, text.to_string()))
    }

    fn options() -> SearchOptions {
        SearchOptions::default()
    }

    fn ranges(text: &str, pattern: &str, opts: SearchOptions) -> Vec<Range> {
        Matcher::new(pattern, opts)
            .expect("patrón válido")
            .find_all(&editor(text))
            .expect("búsqueda completada")
    }

    #[test]
    fn literal_case_sensitive_e_insensible() {
        let texto = "Uno uno UNO dos";
        let strict = ranges(texto, "uno", SearchOptions {
            case_sensitive: true,
            ..options()
        });
        assert_eq!(strict.len(), 1);
        assert_eq!(strict[0].start, Position::new(0, 4), "solo «uno» minúsculas");

        let laxo = ranges(texto, "uno", options());
        assert_eq!(laxo.len(), 3, "sin distinguir encuentra Uno/uno/UNO");
    }

    #[test]
    fn palabra_entera() {
        let texto = "gato gatos gato.";
        let solo_palabra = ranges(texto, "gato", SearchOptions {
            whole_word: true,
            ..options()
        });
        assert_eq!(solo_palabra.len(), 2, "«gatos» no cuenta");
        assert_eq!(solo_palabra[0].start, Position::new(0, 0));
        assert_eq!(solo_palabra[1].start, Position::new(0, 11));

        let subcadenas = ranges(texto, "gato", options());
        assert_eq!(subcadenas.len(), 3, "sin whole_word también «gatos»");
    }

    #[test]
    fn regex_con_subconjunto_documentado() {
        let texto = "# Título\n\nPrecio: 12 y 345.";
        let numeros = ranges(texto, r"\d+", SearchOptions {
            regex: true,
            ..options()
        });
        assert_eq!(numeros.len(), 2);
        assert_eq!(numeros[0], Range::new(Position::new(2, 8), Position::new(2, 10)));
        assert_eq!(numeros[1], Range::new(Position::new(2, 13), Position::new(2, 16)));

        let titulos = ranges(texto, "^# .+", SearchOptions {
            regex: true,
            ..options()
        });
        assert_eq!(titulos.len(), 1);
        assert_eq!(titulos[0].end, Position::new(0, 8));

        // whole_word + regex: el rango debe caer entre límites de palabra.
        let precio = ranges(texto, r"Preci\w+", SearchOptions {
            regex: true,
            whole_word: true,
            ..options()
        });
        assert_eq!(precio.len(), 1);
    }

    #[test]
    fn regex_invalida_es_error_sin_panic() {
        let error = Matcher::new("[a-", SearchOptions {
            regex: true,
            ..options()
        })
        .expect_err("clase sin cerrar");
        assert!(matches!(error, SearchError::InvalidRegex(_)));
        assert!(error.to_string().contains("regex inválida"), "{error}");

        let repetida = Matcher::new("a**", SearchOptions {
            regex: true,
            ..options()
        })
        .expect_err("cuantificador repetido");
        assert!(matches!(repetida, SearchError::InvalidRegex(_)));
    }

    #[test]
    fn coincidencias_vacias_se_descartan() {
        let texto = "sin nada";
        assert!(ranges(texto, "x*", SearchOptions {
            regex: true,
            ..options()
        })
        .is_empty());
        assert!(ranges(texto, "", options()).is_empty(), "patrón vacío");
    }

    #[test]
    fn nivel_texto_y_grupos_de_captura() {
        // Nivel texto (ISSUE-028): offsets de bytes y grupos para `$1`..`$9`.
        let texto = "ana@rosa y juan@paz";
        let re = Matcher::new(
            r"(\w+)@(\w+)",
            SearchOptions {
                regex: true,
                ..options()
            },
        )
        .unwrap();
        let found = re.find_all_in_text(texto);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].start, found[0].end), (0, 8));
        assert_eq!(
            found[0].groups,
            vec![Some((0, 8)), Some((0, 3)), Some((4, 8))]
        );
        assert_eq!((found[1].start, found[1].end), (11, 19));
        assert_eq!(
            found[1].groups,
            vec![Some((11, 19)), Some((11, 15)), Some((16, 19))]
        );

        // Modo literal: solo el rango completo.
        let literal = Matcher::new("ana", options()).unwrap();
        assert_eq!(
            literal.find_all_in_text(texto)[0].groups,
            vec![Some((0, 3))]
        );

        // Grupo opcional que no participa.
        let opcional = Matcher::new(
            "(a)?b",
            SearchOptions {
                regex: true,
                ..options()
            },
        )
        .unwrap();
        assert_eq!(
            opcional.find_all_in_text("b")[0].groups,
            vec![Some((0, 1)), None]
        );
    }

    #[test]
    fn nivel_texto_respeta_case_y_palabra() {
        let texto = "gato Gatos GATO";
        let laxo = Matcher::new("gato", options()).unwrap();
        assert_eq!(laxo.find_all_in_text(texto).len(), 3);
        let strict = Matcher::new(
            "gato",
            SearchOptions {
                case_sensitive: true,
                ..options()
            },
        )
        .unwrap();
        assert_eq!(strict.find_all_in_text(texto).len(), 1);
        let palabra = Matcher::new(
            "gato",
            SearchOptions {
                whole_word: true,
                ..options()
            },
        )
        .unwrap();
        assert_eq!(
            palabra.find_all_in_text(texto).len(),
            2,
            "«Gatos» no es palabra entera"
        );
    }

    #[test]
    fn expand_replacement_expande_grupos() {
        let texto = "ana@rosa";
        let re = Matcher::new(
            r"(\w+)@(\w+)",
            SearchOptions {
                regex: true,
                ..options()
            },
        )
        .unwrap();
        let found = re.find_all_in_text(texto);
        let m = &found[0];
        assert_eq!(re.expand_replacement(texto, m, "$2<$1>"), "rosa<ana>");
        assert_eq!(re.expand_replacement(texto, m, "$0!"), "ana@rosa!");
        assert_eq!(re.expand_replacement(texto, m, "100$$"), "100$");
        assert_eq!(
            re.expand_replacement(texto, m, "$9"),
            "",
            "grupo inexistente: vacío"
        );
        assert_eq!(
            re.expand_replacement(texto, m, "$x"),
            "$x",
            "$ sin dígito es literal"
        );

        // Modo literal: el template sale tal cual salvo `$$`.
        let literal = Matcher::new("ana", options()).unwrap();
        let m = &literal.find_all_in_text(texto)[0];
        assert_eq!(
            literal.expand_replacement(texto, m, "$1 perro"),
            "$1 perro"
        );
        assert_eq!(literal.expand_replacement(texto, m, "a$$b"), "a$b");
    }

    #[test]
    fn navegacion_siguiente_y_anterior_con_wrap() {
        // §40.1: siguiente/anterior con wrap sobre posiciones semánticas (§8.6).
        let mut e = editor("aa bb aa bb aa");
        let matcher = Matcher::new("aa", options()).unwrap();
        let todas = matcher.find_all(&e).unwrap();
        assert_eq!(todas.len(), 3);

        // Cursor al principio: la primera coincidencia.
        let primera = matcher.find_next(&e).unwrap().unwrap();
        assert_eq!(primera, todas[0]);

        // Con la primera seleccionada, siguiente avanza; al final, wrap.
        e.set_selection(crate::position::Selection {
            anchor: todas[0].start,
            head: todas[0].end,
        })
        .unwrap();
        assert_eq!(matcher.find_next(&e).unwrap().unwrap(), todas[1]);
        e.set_selection(crate::position::Selection {
            anchor: todas[1].start,
            head: todas[1].end,
        })
        .unwrap();
        assert_eq!(matcher.find_next(&e).unwrap().unwrap(), todas[2]);
        e.set_selection(crate::position::Selection {
            anchor: todas[2].start,
            head: todas[2].end,
        })
        .unwrap();
        assert_eq!(
            matcher.find_next(&e).unwrap().unwrap(),
            todas[0],
            "wrap al principio"
        );

        // Anterior es simétrico: desde la primera, wrap al final.
        e.set_selection(crate::position::Selection {
            anchor: todas[0].start,
            head: todas[0].end,
        })
        .unwrap();
        assert_eq!(
            matcher.find_previous(&e).unwrap().unwrap(),
            todas[2],
            "wrap al final"
        );
        e.set_selection(crate::position::Selection {
            anchor: todas[1].start,
            head: todas[1].end,
        })
        .unwrap();
        assert_eq!(matcher.find_previous(&e).unwrap().unwrap(), todas[0]);
    }

    #[test]
    fn navegacion_desde_cursor_vacio() {
        // Selección vacía: la coincidencia que empieza en el cursor cuenta
        // como actual (siguiente la incluye, anterior no).
        let mut e = editor("aa xx aa");
        let matcher = Matcher::new("aa", options()).unwrap();
        let todas = matcher.find_all(&e).unwrap();
        e.set_selection(crate::position::Selection::collapsed(Position::new(0, 6)))
            .unwrap();
        assert_eq!(matcher.find_next(&e).unwrap().unwrap(), todas[1]);
        assert_eq!(matcher.find_previous(&e).unwrap().unwrap(), todas[0]);
    }
}
