// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Motor de expresiones regulares propio, std-only (§4.1, ISSUE-027).
//!
//! Subconjunto soportado para el modo «regex» de búsqueda (§40.1):
//!
//! - literales y escapes `\n` `\r` `\t` `\f` `\v` `\\` y puntuación escapada
//!   (`\.`, `\*`, …); una letra de escape desconocida es un error;
//! - `.`: cualquier carácter salvo `\n`;
//! - clases `[abc]`, `[a-z]`, `[^…]` (una `]` inicial o un `-` inicial/final
//!   son literales), con escapes y `\d` `\D` `\w` `\W` `\s` `\S` dentro;
//! - anclas de línea `^` y `$` (principio/final de línea, también del buffer);
//! - límites de palabra `\b` y `\B`;
//! - grupos `(...)` de agrupación **y captura** (para `$0`..`$9` en los
//!   reemplazos, [`crate::search::Matcher::expand_replacement`]);
//! - alternancia `|`;
//! - cuantificadores `*`, `+`, `?` codiciosos; sin perezosos ni `{n,m}` (una
//!   llave suelta es un literal).
//!
//! Clases de carácter: `\d` = dígito ASCII `[0-9]`; `\w` = alfanumérico
//! Unicode o `_`; `\s` = espacio Unicode. El límite de palabra `\b` usa `\w`.
//! Sin distinguir mayúsculas se prueban las variantes `to_lowercase` /
//! `to_uppercase` de cada carácter (aproximación por carácter, sin plegado
//! Unicode completo).
//!
//! Semántica de coincidencia: primera por la izquierda y, a igual inicio, la
//! alternativa de **mayor prioridad** (estilo Perl/ECMAScript: `a|ab` sobre
//! «ab» empareja «a»; `a*` sobre «aa» empareja «aa»).
//!
//! Implementación: compilación a un programa estilo Thompson ejecutado por
//! simulación NFA con prioridad — **sin backtracking** — de coste lineal en
//! texto × programa. El trabajo de cada intento está acotado por
//! [`MAX_STEPS`]; al agotarlo se devuelve [`SearchError::TooComplex`], nunca
//! un bucle infinito ni un panic (§47.5: la búsqueda no puede colgar la UI).

use crate::search::SearchError;
use std::rc::Rc;

/// Presupuesto de pasos de simulación por intento de emparejado.
const MAX_STEPS: u32 = 20_000_000;

/// Carácter en la posición `at` y la siguiente frontera de carácter
/// (offsets de bytes).
pub(crate) fn char_at(text: &str, at: usize) -> Option<(char, usize)> {
    text[at..].chars().next().map(|c| (c, at + c.len_utf8()))
}

/// Carácter anterior a `at` y su frontera (offsets de bytes).
pub(crate) fn char_before(text: &str, at: usize) -> Option<(char, usize)> {
    text[..at]
        .chars()
        .next_back()
        .map(|c| (c, at - c.len_utf8()))
}

/// `\w` Unicode: alfanumérico o `_` (documentado arriba).
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Clase de caracteres `[...]` o escape `\d`, `\w`, `\s` y negaciones.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Class {
    negated: bool,
    items: Vec<ClassItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClassItem {
    Char(char),
    Range(char, char),
    Digit,
    NotDigit,
    Word,
    NotWord,
    Space,
    NotSpace,
}

impl ClassItem {
    fn matches(&self, c: char) -> bool {
        match *self {
            Self::Char(x) => c == x,
            Self::Range(a, b) => a <= c && c <= b,
            Self::Digit => c.is_ascii_digit(),
            Self::NotDigit => !c.is_ascii_digit(),
            Self::Word => is_word_char(c),
            Self::NotWord => !is_word_char(c),
            Self::Space => c.is_whitespace(),
            Self::NotSpace => !c.is_whitespace(),
        }
    }
}

impl Class {
    fn single(c: char) -> Self {
        Self {
            negated: false,
            items: vec![ClassItem::Char(c)],
        }
    }

    fn of(item: ClassItem) -> Self {
        Self {
            negated: false,
            items: vec![item],
        }
    }

    fn negated(item: ClassItem) -> Self {
        Self {
            negated: true,
            items: vec![item],
        }
    }

    fn matches(&self, c: char) -> bool {
        self.items.iter().any(|item| item.matches(c)) != self.negated
    }

    /// Equivalencia sin distinguir mayúsculas: prueban las variantes por
    /// carácter (aproximación documentada).
    fn matches_fold(&self, c: char) -> bool {
        self.matches(c)
            || c.to_lowercase().any(|v| v != c && self.matches(v))
            || c.to_uppercase().any(|v| v != c && self.matches(v))
    }
}

/// Aserción de ancho cero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Assertion {
    StartLine,
    EndLine,
    WordBoundary,
    NotWordBoundary,
}

impl Assertion {
    fn holds(&self, text: &str, at: usize) -> bool {
        match self {
            Self::StartLine => at == 0 || text.as_bytes()[at - 1] == b'\n',
            Self::EndLine => at == text.len() || text.as_bytes()[at] == b'\n',
            Self::WordBoundary | Self::NotWordBoundary => {
                let before = char_before(text, at)
                    .map(|(c, _)| is_word_char(c))
                    .unwrap_or(false);
                let after = char_at(text, at)
                    .map(|(c, _)| is_word_char(c))
                    .unwrap_or(false);
                (before != after) == matches!(self, Self::WordBoundary)
            }
        }
    }
}

/// Árbol sintáctico del subconjunto.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Empty,
    Assert(Assertion),
    Any,
    Class(Class),
    /// Grupo `(...)`: `index` (1-based) es su número de captura.
    Group { inner: Box<Node>, index: usize },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Star(Box<Node>),
    Plus(Box<Node>),
    Opt(Box<Node>),
}

/// Analizador descendente recursivo sobre los caracteres del patrón.
struct Parser<'a> {
    pat: &'a [char],
    at: usize,
    /// Grupos `(...)` vistos (para numerar las capturas).
    groups: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.pat.get(self.at).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.pat.get(self.at + offset).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.at += 1;
        }
        c
    }

    /// alternación: `concat ('|' concat)*`.
    fn parse_alt(&mut self) -> Result<Node, SearchError> {
        let mut branches = vec![self.parse_concat()?];
        while self.peek() == Some('|') {
            self.bump();
            branches.push(self.parse_concat()?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().expect("una rama")
        } else {
            Node::Alt(branches)
        })
    }

    /// concatenación de términos; se detiene en `|`, `)` o final.
    fn parse_concat(&mut self) -> Result<Node, SearchError> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            items.push(self.parse_term()?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().expect("un término"),
            _ => Node::Concat(items),
        })
    }

    /// `átomo cuantificador?` con cuantificadores codiciosos `*`, `+`, `?`.
    fn parse_term(&mut self) -> Result<Node, SearchError> {
        let mut node = self.parse_atom()?;
        let mut quantified = false;
        loop {
            node = match self.peek() {
                Some('*') => Node::Star(Box::new(node)),
                Some('+') => Node::Plus(Box::new(node)),
                Some('?') => Node::Opt(Box::new(node)),
                _ => break,
            };
            self.bump();
            if quantified {
                return Err(SearchError::InvalidRegex(
                    "cuantificador repetido (los perezosos no están soportados)".to_string(),
                ));
            }
            quantified = true;
        }
        Ok(node)
    }

    fn parse_atom(&mut self) -> Result<Node, SearchError> {
        let c = self
            .bump()
            .expect("parse_atom solo se llama con carácter disponible");
        match c {
            '(' => {
                self.groups += 1;
                let index = self.groups;
                let inner = self.parse_alt()?;
                if self.bump() != Some(')') {
                    return Err(SearchError::InvalidRegex("grupo sin cerrar «(»".to_string()));
                }
                Ok(Node::Group {
                    inner: Box::new(inner),
                    index,
                })
            }
            '[' => self.parse_class(),
            '.' => Ok(Node::Any),
            '^' => Ok(Node::Assert(Assertion::StartLine)),
            '$' => Ok(Node::Assert(Assertion::EndLine)),
            '\\' => self.parse_escape(),
            '*' | '+' | '?' => Err(SearchError::InvalidRegex(
                "cuantificador sin átomo que cuantificar".to_string(),
            )),
            _ => Ok(Node::Class(Class::single(c))),
        }
    }

    /// Escape fuera de clase (cursor tras `\`).
    fn parse_escape(&mut self) -> Result<Node, SearchError> {
        // `\b` y `\B` son aserciones de límite de palabra, no literales.
        match self.peek() {
            Some('b') => {
                self.bump();
                return Ok(Node::Assert(Assertion::WordBoundary));
            }
            Some('B') => {
                self.bump();
                return Ok(Node::Assert(Assertion::NotWordBoundary));
            }
            _ => {}
        }
        let (item, literal) = self.parse_escape_item()?;
        match literal {
            Some(c) => Ok(Node::Class(Class::single(c))),
            None => Ok(match item {
                ClassItem::Digit => Node::Class(Class::of(ClassItem::Digit)),
                ClassItem::NotDigit => Node::Class(Class::negated(ClassItem::Digit)),
                ClassItem::Word => Node::Class(Class::of(ClassItem::Word)),
                ClassItem::NotWord => Node::Class(Class::negated(ClassItem::Word)),
                ClassItem::Space => Node::Class(Class::of(ClassItem::Space)),
                ClassItem::NotSpace => Node::Class(Class::negated(ClassItem::Space)),
                ClassItem::Char(c) => Node::Class(Class::single(c)),
                ClassItem::Range(a, b) => Node::Class(Class {
                    negated: false,
                    items: vec![ClassItem::Range(a, b)],
                }),
            }),
        }
    }

    /// Escape común a átomos y clases (cursor tras `\`): el ítem de clase y,
    /// si es un carácter concreto, su valor.
    fn parse_escape_item(&mut self) -> Result<(ClassItem, Option<char>), SearchError> {
        let Some(c) = self.bump() else {
            return Err(SearchError::InvalidRegex("escape incompleto «\\»".to_string()));
        };
        let item = match c {
            'd' => ClassItem::Digit,
            'D' => ClassItem::NotDigit,
            'w' => ClassItem::Word,
            'W' => ClassItem::NotWord,
            's' => ClassItem::Space,
            'S' => ClassItem::NotSpace,
            'n' => ClassItem::Char('\n'),
            'r' => ClassItem::Char('\r'),
            't' => ClassItem::Char('\t'),
            'f' => ClassItem::Char('\u{c}'),
            'v' => ClassItem::Char('\u{b}'),
            _ if !c.is_alphanumeric() => ClassItem::Char(c),
            _ => {
                return Err(SearchError::InvalidRegex(format!(
                    "escape no reconocido: «\\{c}»"
                )))
            }
        };
        let literal = match item {
            ClassItem::Char(c) => Some(c),
            _ => None,
        };
        Ok((item, literal))
    }

    /// Clase `[...]` (cursor tras `[`).
    fn parse_class(&mut self) -> Result<Node, SearchError> {
        let mut negated = false;
        if self.peek() == Some('^') {
            self.bump();
            negated = true;
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                return Err(SearchError::InvalidRegex(
                    "clase de caracteres sin cerrar «[»".to_string(),
                ));
            };
            if c == ']' && !first {
                self.bump();
                break;
            }
            first = false;
            let (item, literal) = if c == '\\' {
                self.bump();
                self.parse_escape_item()?
            } else {
                self.bump();
                (ClassItem::Char(c), Some(c))
            };
            // Rango `a-z`: solo si sigue `-` y algo que no sea `]`.
            if self.peek() == Some('-') && self.peek_at(1).is_some_and(|n| n != ']') {
                self.bump();
                let start = literal.ok_or_else(|| {
                    SearchError::InvalidRegex("rango de clase inválido".to_string())
                })?;
                let end_c = self.bump().ok_or_else(|| {
                    SearchError::InvalidRegex(
                        "clase de caracteres sin cerrar «[»".to_string(),
                    )
                })?;
                let end = if end_c == '\\' {
                    let (_, literal) = self.parse_escape_item()?;
                    literal.ok_or_else(|| {
                        SearchError::InvalidRegex("rango de clase inválido".to_string())
                    })?
                } else {
                    end_c
                };
                if end < start {
                    return Err(SearchError::InvalidRegex(format!(
                        "rango invertido en clase: «{start}-{end}»"
                    )));
                }
                items.push(ClassItem::Range(start, end));
            } else {
                items.push(item);
            }
        }
        Ok(Node::Class(Class { negated, items }))
    }
}

/// Instrucción del programa compilado (estilo Thompson).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Inst {
    /// Consume cualquier carácter salvo `\n`.
    Any,
    /// Consume un carácter de la clase.
    Class(Class),
    /// Prueba la rama `0` (mayor prioridad) y después la `1`.
    Split(usize, usize),
    /// Salto incondicional.
    Jump(usize),
    /// Aserción de ancho cero.
    Assert(Assertion),
    /// Registra la posición actual en el slot de captura `n`.
    Save(usize),
    /// Fin de la coincidencia.
    Match,
}

/// Coincidencia con sus capturas (offsets de bytes).
pub(crate) struct SubMatch {
    pub(crate) end: usize,
    /// Pares `(inicio, fin)` por grupo; `groups[0]` es la coincidencia
    /// completa; un grupo que no participó es `None`.
    pub(crate) groups: Vec<Option<(usize, usize)>>,
}

/// Expresión regular compilada.
#[derive(Debug, Clone)]
pub(crate) struct Regex {
    prog: Vec<Inst>,
    /// `true` = equivalencia sin distinguir mayúsculas.
    fold: bool,
    /// Número de slots de captura: `2 · (grupos + 1)`.
    slots: usize,
}

/// Emite el programa de `node` (Thompson con parcheado de saltos).
fn emit(node: &Node, prog: &mut Vec<Inst>) {
    match node {
        Node::Empty => {}
        Node::Assert(kind) => prog.push(Inst::Assert(*kind)),
        Node::Any => prog.push(Inst::Any),
        Node::Class(class) => prog.push(Inst::Class(class.clone())),
        Node::Group { inner, index } => {
            // Slots `2·index` (inicio) y `2·index + 1` (fin); el par 0/1 es
            // la coincidencia completa.
            prog.push(Inst::Save(2 * index));
            emit(inner, prog);
            prog.push(Inst::Save(2 * index + 1));
        }
        Node::Concat(items) => {
            for item in items {
                emit(item, prog);
            }
        }
        Node::Alt(branches) => {
            let mut fixes = Vec::new();
            for (index, branch) in branches.iter().enumerate() {
                if index + 1 == branches.len() {
                    emit(branch, prog);
                } else {
                    let split = prog.len();
                    prog.push(Inst::Jump(0)); // split parcheado después
                    emit(branch, prog);
                    fixes.push(prog.len());
                    prog.push(Inst::Jump(0)); // salto al final parcheado después
                    let next = prog.len();
                    prog[split] = Inst::Split(split + 1, next);
                }
            }
            let end = prog.len();
            for at in fixes {
                prog[at] = Inst::Jump(end);
            }
        }
        Node::Star(inner) => {
            let split = prog.len();
            prog.push(Inst::Jump(0)); // split parcheado después
            emit(inner, prog);
            prog.push(Inst::Jump(split));
            let end = prog.len();
            prog[split] = Inst::Split(split + 1, end);
        }
        Node::Plus(inner) => {
            let body = prog.len();
            emit(inner, prog);
            let split = prog.len();
            prog.push(Inst::Split(body, split + 1));
        }
        Node::Opt(inner) => {
            let split = prog.len();
            prog.push(Inst::Jump(0)); // split parcheado después
            emit(inner, prog);
            let end = prog.len();
            prog[split] = Inst::Split(split + 1, end);
        }
    }
}

/// Lista de hilos del NFA en una posición: instrucciones de consumo listas,
/// en orden de prioridad, con marcas de «ya añadido» por sello. Cada hilo
/// lleva sus slots de captura (compartidos salvo al cruzar un `Save`).
struct Threads {
    list: Vec<(usize, Rc<Vec<Option<usize>>>)>,
    seen: Vec<u32>,
    stamp: u32,
}

impl Threads {
    fn new(size: usize) -> Self {
        Self {
            list: Vec::new(),
            seen: vec![0; size],
            stamp: 0,
        }
    }

    /// Abre un paso de simulación (lista vacía, sello nuevo).
    fn begin(&mut self) {
        self.stamp += 1;
        self.list.clear();
    }
}

impl Regex {
    /// Compila el patrón; `fold` = sin distinguir mayúsculas.
    pub(crate) fn parse(pattern: &str, fold: bool) -> Result<Self, SearchError> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut parser = Parser {
            pat: &chars,
            at: 0,
            groups: 0,
        };
        let root = parser.parse_alt()?;
        if parser.at < chars.len() {
            return Err(SearchError::InvalidRegex(
                "paréntesis de cierre «)» sin apertura".to_string(),
            ));
        }
        let mut prog = Vec::new();
        prog.push(Inst::Save(0));
        emit(&root, &mut prog);
        prog.push(Inst::Save(1));
        prog.push(Inst::Match);
        Ok(Self {
            prog,
            fold,
            slots: 2 * (parser.groups + 1),
        })
    }

    /// Fin de la coincidencia **no vacía** de mayor prioridad anclada en
    /// `start` (offset de bytes), con las capturas de cada grupo; `None` si
    /// no hay ninguna.
    ///
    /// Las coincidencias vacías se descartan (§40.1: reemplazarlas sería
    /// insertar en cada posición) sin cortar las ramas de igual inicio que
    /// sí consumen caracteres.
    pub(crate) fn find_at_full(
        &self,
        text: &str,
        start: usize,
    ) -> Result<Option<SubMatch>, SearchError> {
        self.run_at(text, start)
    }

    /// Simulación NFA con prioridad desde `start` (sin backtracking).
    fn run_at(&self, text: &str, start: usize) -> Result<Option<SubMatch>, SearchError> {
        let mut steps: u32 = 0;
        let mut matched: Option<SubMatch> = None;
        let mut current = Threads::new(self.prog.len());
        let mut next = Threads::new(self.prog.len());
        let slots = Rc::new(vec![None; self.slots]);
        current.begin();
        self.add_thread(
            text,
            start,
            start,
            0,
            Rc::clone(&slots),
            &mut current,
            &mut matched,
            &mut steps,
        )?;
        let mut pos = start;
        while !current.list.is_empty() {
            let Some((c, next_pos)) = char_at(text, pos) else {
                break;
            };
            next.begin();
            for (pc, slots) in &current.list {
                let pc = *pc;
                steps += 1;
                if steps > MAX_STEPS {
                    return Err(SearchError::TooComplex(
                        "búsqueda demasiado compleja (presupuesto de simulación agotado)"
                            .to_string(),
                    ));
                }
                let consumes = match &self.prog[pc] {
                    Inst::Any => c != '\n',
                    Inst::Class(class) => {
                        if self.fold {
                            class.matches_fold(c)
                        } else {
                            class.matches(c)
                        }
                    }
                    _ => false,
                };
                if consumes
                    && !self.add_thread(
                        text,
                        next_pos,
                        start,
                        pc + 1,
                        Rc::clone(slots),
                        &mut next,
                        &mut matched,
                        &mut steps,
                    )?
                {
                    // Match alcanzado: se corta lo de menor prioridad en este
                    // paso; los hilos de mayor prioridad siguen vivos.
                    break;
                }
            }
            std::mem::swap(&mut current, &mut next);
            pos = next_pos;
        }
        Ok(matched)
    }

    /// Cierra `pc` por ε en la posición `pos` y lo registra en `threads`.
    ///
    /// Devuelve `false` al alcanzar `Match`: fija `matched` (con las capturas
    /// del hilo) y corta las ramas de menor prioridad (semántica de
    /// prioridad estilo Perl). Las coincidencias vacías se rechazan **sin**
    /// cortar para que las ramas que consumen algo del mismo inicio sigan
    /// compitiendo.
    #[allow(clippy::too_many_arguments)]
    fn add_thread(
        &self,
        text: &str,
        pos: usize,
        start: usize,
        pc: usize,
        slots: Rc<Vec<Option<usize>>>,
        threads: &mut Threads,
        matched: &mut Option<SubMatch>,
        steps: &mut u32,
    ) -> Result<bool, SearchError> {
        *steps += 1;
        if *steps > MAX_STEPS {
            return Err(SearchError::TooComplex(
                "búsqueda demasiado compleja (presupuesto de simulación agotado)".to_string(),
            ));
        }
        if threads.seen[pc] == threads.stamp {
            return Ok(true);
        }
        threads.seen[pc] = threads.stamp;
        match &self.prog[pc] {
            Inst::Jump(target) => self.add_thread(
                text,
                pos,
                start,
                *target,
                slots,
                threads,
                matched,
                steps,
            ),
            Inst::Split(a, b) => {
                if !self.add_thread(
                    text,
                    pos,
                    start,
                    *a,
                    Rc::clone(&slots),
                    threads,
                    matched,
                    steps,
                )? {
                    return Ok(false);
                }
                self.add_thread(text, pos, start, *b, slots, threads, matched, steps)
            }
            Inst::Assert(kind) => {
                if kind.holds(text, pos) {
                    self.add_thread(
                        text,
                        pos,
                        start,
                        pc + 1,
                        slots,
                        threads,
                        matched,
                        steps,
                    )
                } else {
                    Ok(true)
                }
            }
            Inst::Save(slot) => {
                if slots[*slot] == Some(pos) {
                    return self.add_thread(
                        text,
                        pos,
                        start,
                        pc + 1,
                        slots,
                        threads,
                        matched,
                        steps,
                    );
                }
                let mut updated = (*slots).clone();
                updated[*slot] = Some(pos);
                self.add_thread(
                    text,
                    pos,
                    start,
                    pc + 1,
                    Rc::new(updated),
                    threads,
                    matched,
                    steps,
                )
            }
            Inst::Match => {
                if pos == start {
                    Ok(true) // coincidencia vacía: descartada, sin cortar
                } else {
                    *matched = Some(SubMatch {
                        end: pos,
                        groups: groups_from_slots(&slots, pos),
                    });
                    Ok(false) // corta lo de menor prioridad
                }
            }
            Inst::Any | Inst::Class(_) => {
                threads.list.push((pc, slots));
                Ok(true)
            }
        }
    }
}

/// Convierte los slots (`2·grupo`: inicio; `2·grupo + 1`: fin) en pares de
/// captura; `groups[0]` es la coincidencia completa (el slot 1 se sustituye
/// por el `end` real). Un grupo sin cerrar queda en `None`.
fn groups_from_slots(slots: &[Option<usize>], end: usize) -> Vec<Option<(usize, usize)>> {
    slots
        .chunks(2)
        .enumerate()
        .map(|(index, pair)| {
            let finish = if index == 0 {
                Some(end)
            } else {
                pair.get(1).copied().flatten()
            };
            match (pair[0], finish) {
                (Some(s), Some(e)) => Some((s, e)),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiled(pattern: &str) -> Regex {
        Regex::parse(pattern, false).expect("patrón válido")
    }

    fn match_at(pattern: &str, text: &str, start: usize) -> Option<usize> {
        compiled(pattern)
            .find_at_full(text, start)
            .expect("sin agotar pasos")
            .map(|found| found.end)
    }

    #[test]
    fn literales_y_puntuacion_escapada() {
        assert_eq!(match_at(r"a\.b", "xa.by", 1), Some(4));
        assert_eq!(match_at(r"a.b", "xa.by", 1), Some(4));
        assert_eq!(match_at(r"\*", "a*b", 1), Some(2));
    }

    #[test]
    fn punto_no_cruza_saltos_de_linea() {
        assert_eq!(match_at("a.b", "a\nb", 0), None);
        assert_eq!(match_at("a.b", "axb", 0), Some(3));
    }

    #[test]
    fn cuantificadores_codiciosos() {
        assert_eq!(match_at("a*", "aa", 0), Some(2), "codicioso: «aa», no vacío");
        assert_eq!(match_at("a*", "b", 0), None, "la coincidencia vacía se descarta");
        assert_eq!(match_at("a*b", "aab", 0), Some(3));
        assert_eq!(match_at("ab+", "abbb", 0), Some(4));
        assert_eq!(match_at("ab?c", "ac", 0), Some(2));
        assert_eq!(match_at("ab?c", "abc", 0), Some(3));
    }

    #[test]
    fn clases_rangos_y_negacion() {
        assert_eq!(match_at("[abc]+", "xbcay", 1), Some(4));
        assert_eq!(match_at("[a-f0-9]+", "xyabc12z", 2), Some(7));
        assert_eq!(match_at("[^abc]+", "abxyz", 2), Some(5));
        assert_eq!(match_at("[]]+", "x]y", 1), Some(2), "] inicial es literal");
        assert_eq!(match_at("[a-]+", "x-y", 1), Some(2), "- final es literal");
    }

    #[test]
    fn escapes_de_clase_y_aserciones() {
        assert_eq!(match_at(r"\d+", "ab 123", 3), Some(6));
        assert_eq!(match_at(r"\w+", " ¡hola!", 3), Some(7), "inicio en frontera de carácter");
        assert_eq!(match_at(r"\s+", "ab \t c", 2), Some(5));
        assert_eq!(match_at(r"\bb", "ab b", 3), Some(4));
        assert_eq!(match_at(r"\Bb", "ab b", 1), Some(2));
        assert_eq!(match_at("^dos", "uno\ndos", 4), Some(7));
        assert_eq!(match_at("uno$", "uno\ndos", 0), Some(3));
    }

    #[test]
    fn grupos_alternancia_y_prioridad() {
        assert_eq!(match_at("(ab)+c", "ababc", 0), Some(5));
        assert_eq!(match_at("a|ab", "ab", 0), Some(1), "estilo Perl: la primera rama");
        assert_eq!(match_at("(ab|a)b", "ab", 0), Some(2), "backtracking sin VM: la rama larga");
        assert_eq!(match_at("colou?r", "color", 0), Some(5));
    }

    #[test]
    fn sin_distinguir_mayusculas() {
        let re = Regex::parse("hola", true).unwrap();
        let fin = |found: Option<SubMatch>| found.map(|found| found.end);
        assert_eq!(
            fin(re.find_at_full("HOLA mundo", 0).unwrap()),
            Some(4)
        );
        assert_eq!(fin(re.find_at_full("¡Hola!", 2).unwrap()), Some(6));
    }

    #[test]
    fn grupos_de_captura_se_registran() {
        let re = compiled(r"(\w+)@(\w+)");
        let found = re.find_at_full("ana@rosa", 0).unwrap().unwrap();
        assert_eq!(found.end, 8);
        assert_eq!(
            found.groups,
            vec![Some((0, 8)), Some((0, 3)), Some((4, 8))],
            "completa, usuario y dominio"
        );

        // Un grupo que no participa es `None`.
        let opcional = compiled(r"(a)?b");
        assert_eq!(
            opcional.find_at_full("b", 0).unwrap().unwrap().groups,
            vec![Some((0, 1)), None]
        );

        // La rama que gana (prioridad) es la que captura.
        let alternancia = compiled(r"(a|ab)c");
        assert_eq!(
            alternancia.find_at_full("abc", 0).unwrap().unwrap().groups,
            vec![Some((0, 3)), Some((0, 2))]
        );
    }

    #[test]
    fn patrones_invalidos_son_error_sin_panic() {
        for (patrón, por_que) in [
            ("(ab", "grupo sin cerrar"),
            ("a)", "paréntesis sin apertura"),
            ("[a", "clase sin cerrar"),
            ("*", "cuantificador sin átomo"),
            ("a**", "cuantificador repetido"),
            (r"a\", "escape incompleto"),
            (r"\q", "escape desconocido"),
            ("[z-a]", "rango invertido"),
            (r"[\d-z]", "rango inválido"),
        ] {
            let error = Regex::parse(patrón, false)
                .expect_err(&format!("{patrón:?} debería fallar ({por_que})"));
            assert!(
                matches!(error, SearchError::InvalidRegex(_)),
                "{patrón:?}: {error}"
            );
        }
    }

    #[test]
    fn presupuesto_de_simulacion_acotado() {
        // Un patrón que recorre el buffer entero desde cada posición agota el
        // presupuesto con un texto suficientemente grande: error explícito,
        // nunca bucle infinito (§47.5).
        let re = compiled(".*x");
        let text = "a".repeat(5_000_000);
        assert!(matches!(
            re.find_at_full(&text, 0),
            Err(SearchError::TooComplex(_))
        ));
    }
}
