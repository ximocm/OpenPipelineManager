// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Editor de Markdown Mode (ISSUE-015, §8).
//!
//! Buffer editable con operaciones insert/delete/replace, cursor y selección
//! como posición semántica (§8.6) e historial Undo/Redo **único y compartido**
//! entre Markdown y Visual (§8.5): el historial vive en el editor del buffer,
//! no en los modos, y cambiar de modo no lo reinicia.

use crate::document::Document;
use crate::position::{Position, Range, Selection};
use std::fmt;

/// Error de edición: posición o rango inválido.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// Posición fuera del buffer.
    InvalidPosition(Position),
    /// Rangos de [`Editor::replace_all`] solapados (deben ser disjuntos).
    OverlappingRanges(Range),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPosition(p) => write!(
                f,
                "posición fuera del buffer: línea {}, columna {}",
                p.line, p.column
            ),
            Self::OverlappingRanges(r) => write!(
                f,
                "rangos de reemplazo solapados: ({}, {})–({}, {})",
                r.start.line, r.start.column, r.end.line, r.end.column
            ),
        }
    }
}

impl std::error::Error for EditError {}

/// Editor sobre el buffer Markdown único (§6.1).
pub struct Editor {
    document: Document,
    selection: Selection,
    history: History,
    /// Offset de inicio de cada línea (índice de posiciones semánticas).
    line_starts: Vec<usize>,
}

/// Historial Undo/Redo compartido por todos los modos (§8.5).
#[derive(Default)]
struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
}

/// Una transacción = una operación de edición completa, con el estado del
/// cursor antes y después (§8.6).
struct Transaction {
    changes: Vec<Change>,
    before: Selection,
    after: Selection,
}

/// Cambio atómico sobre rangos de bytes del buffer.
struct Change {
    start: usize,
    deleted: String,
    inserted: String,
}

impl Editor {
    pub fn new(document: Document) -> Self {
        let line_starts = compute_line_starts(document.content());
        Self {
            document,
            selection: Selection::collapsed(Position::new(0, 0)),
            history: History::default(),
            line_starts,
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Acceso mutable para guardado explícito (ISSUE-013): no modifica el
    /// contenido ni el historial.
    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// Mueve el cursor o fija la selección (§8.4: move cursor, select).
    /// No entra en el historial: los movimientos no son deshacibles.
    pub fn set_selection(&mut self, selection: Selection) -> Result<(), EditError> {
        self.offset_of(selection.anchor)?;
        self.offset_of(selection.head)?;
        self.selection = selection;
        Ok(())
    }

    /// Inserta `text` reemplazando la selección actual (§8.4: insert).
    pub fn insert(&mut self, text: &str) -> Result<(), EditError> {
        self.replace(self.selection.range(), text)
    }

    /// Borra la selección actual (§8.4: delete).
    pub fn delete(&mut self) -> Result<(), EditError> {
        self.replace(self.selection.range(), "")
    }

    /// Reemplaza `range` por `text` (§8.4: replace). El cursor queda al final
    /// del texto insertado. Es la operación base: insert y delete se derivan.
    pub fn replace(&mut self, range: Range, text: &str) -> Result<(), EditError> {
        let a = self.offset_of(range.start)?;
        let b = self.offset_of(range.end)?;
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        if start == end && text.is_empty() {
            return Ok(());
        }

        let before = self.selection;
        let change = Change {
            start,
            deleted: self.document.content()[start..end].to_string(),
            inserted: text.to_string(),
        };
        self.apply(&change);
        let after = Selection::collapsed(self.position_of(start + text.len()));
        self.history.undo.push(Transaction {
            changes: vec![change],
            before,
            after,
        });
        self.history.redo.clear();
        self.selection = after;
        Ok(())
    }

    /// Reemplaza varios rangos de una vez como **UNA sola transacción** del
    /// historial Undo/Redo único (§8.5): un solo `undo` revierte el reemplazo
    /// masivo completo y un solo `redo` lo reaplica.
    ///
    /// Los `edits` se ordenan por inicio y no deben solaparse
    /// ([`EditError::OverlappingRanges`]); el texto de reemplazo se inserta
    /// literalmente. La selección se restaura con la semántica de las
    /// transacciones (§8.6): antes la que había y después el cursor al final
    /// del último texto insertado. Una lista vacía o solo con operaciones
    /// nulas no entra en el historial.
    pub fn replace_all(&mut self, edits: &[(Range, String)]) -> Result<(), EditError> {
        // Coordenadas del buffer original y textos, ordenados por inicio.
        let mut plan: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
        for (range, text) in edits {
            let a = self.offset_of(range.start)?;
            let b = self.offset_of(range.end)?;
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            if start == end && text.is_empty() {
                continue; // operación nula: no entra en el historial
            }
            plan.push((start, end, text.as_str()));
        }
        plan.sort_by(|x, y| (x.0, x.1).cmp(&(y.0, y.1)));
        for pair in plan.windows(2) {
            if pair[0].1 > pair[1].0 {
                let (start, end) = (pair[1].0, pair[1].1);
                return Err(EditError::OverlappingRanges(Range::new(
                    self.position_of(start),
                    self.position_of(end),
                )));
            }
        }
        if plan.is_empty() {
            return Ok(());
        }

        // Los offsets de cada cambio se encadenan: cada uno es válido sobre el
        // buffer resultante de aplicar los anteriores (como en redo), y sus
        // inversos en orden inverso reconstruyen el original (como en undo).
        let before = self.selection;
        let mut changes = Vec::with_capacity(plan.len());
        let mut delta: isize = 0;
        let mut end_after = 0;
        for (start, end, text) in plan {
            let start_now = (start as isize + delta) as usize;
            delta += text.len() as isize - (end - start) as isize;
            end_after = (end as isize + delta) as usize;
            changes.push(Change {
                start: start_now,
                deleted: self.document.content()[start..end].to_string(),
                inserted: text.to_string(),
            });
        }
        for change in &changes {
            self.apply(change);
        }
        let after = Selection::collapsed(self.position_of(end_after));
        self.history.undo.push(Transaction {
            changes,
            before,
            after,
        });
        self.history.redo.clear();
        self.selection = after;
        Ok(())
    }

    /// Sustituye TODO el contenido del buffer por `content` sin crear
    /// transacción (recarga tras una escritura PROPIA en disco, §12.7 — p.
    /// ej. el reemplazo masivo §40.2): §8.5 el historial Undo/Redo no se toca
    /// (la escritura no pasa por él: ni gana entradas ni se reinicia) y §8.6
    /// la selección se conserva, con el cursor al final del contenido si la
    /// nueva dejara de validarla.
    ///
    /// A diferencia de [`Document::replace_content`], recalcula el índice de
    /// líneas de las posiciones semánticas (§8.6).
    pub fn replace_content(&mut self, content: &str) {
        self.document.replace_content(content);
        self.line_starts = compute_line_starts(self.document.content());
        let valida = self.offset_of(self.selection.anchor).is_ok()
            && self.offset_of(self.selection.head).is_ok();
        if !valida {
            let fin = self.position_of(self.document.content().len());
            self.selection = Selection::collapsed(fin);
        }
    }

    /// Deshace la última transacción. `false` = no hay nada que deshacer.
    pub fn undo(&mut self) -> bool {
        let Some(transaction) = self.history.undo.pop() else {
            return false;
        };
        for change in transaction.changes.iter().rev() {
            let inverse = Change {
                start: change.start,
                deleted: change.inserted.clone(),
                inserted: change.deleted.clone(),
            };
            self.apply(&inverse);
        }
        self.selection = transaction.before;
        self.history.redo.push(transaction);
        true
    }

    /// Rehace la última transacción deshecha.
    pub fn redo(&mut self) -> bool {
        let Some(transaction) = self.history.redo.pop() else {
            return false;
        };
        for change in &transaction.changes {
            self.apply(change);
        }
        self.selection = transaction.after;
        self.history.undo.push(transaction);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    fn apply(&mut self, change: &Change) {
        let end = change.start + change.deleted.len();
        self.document
            .replace_range(change.start, end, &change.inserted);
        self.reindex(change.start, end, &change.inserted);
    }

    /// Mantiene el índice de líneas tras sustituir `[start, end)` por
    /// `inserted`.
    fn reindex(&mut self, start: usize, end: usize, inserted: &str) {
        let delta = inserted.len() as isize - (end - start) as isize;
        // Los inicios de línea en (start, end] desaparecen con su salto.
        let lo = self.line_starts.partition_point(|&p| p <= start);
        let hi = self.line_starts.partition_point(|&p| p <= end);
        self.line_starts.drain(lo..hi);
        for p in &mut self.line_starts[lo..] {
            *p = (*p as isize + delta) as usize;
        }
        // Nuevos inicios de línea del texto insertado.
        let new: Vec<usize> = inserted
            .bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(i, _)| start + i + 1)
            .collect();
        self.line_starts.splice(lo..lo, new);
    }

    /// Offset de bytes en el fuente de una posición semántica (§8.6).
    ///
    /// También es la frontera Preview ↔ Editor (§9.1): el doble clic en un
    /// elemento del Preview usa `position_of` sobre el rango del elemento.
    pub fn offset_of(&self, position: Position) -> Result<usize, EditError> {
        let line = position.line as usize;
        let invalid = || EditError::InvalidPosition(position);
        let &line_start = self.line_starts.get(line).ok_or_else(invalid)?;
        let segment_end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.document.content().len());
        let segment = &self.document.content()[line_start..segment_end];
        let column = position.column as usize;
        if column == segment.chars().count() {
            return Ok(segment_end);
        }
        segment
            .char_indices()
            .nth(column)
            .map(|(i, _)| line_start + i)
            .ok_or_else(invalid)
    }

    /// Posición semántica de un offset de bytes del fuente (§9.1: doble clic
    /// en Preview → cursor en el editor).
    pub fn position_of(&self, offset: usize) -> Position {
        let line = self.line_starts.partition_point(|&p| p <= offset) - 1;
        let line_start = self.line_starts[line];
        let column = self.document.content()[line_start..offset].chars().count();
        Position::new(line as u32, column as u32)
    }
}

fn compute_line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        content
            .bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(i, _)| i + 1),
    );
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        Editor::new(Document::from_text(None, text.to_string()))
    }

    fn content(editor: &Editor) -> &str {
        editor.document().content()
    }

    #[test]
    fn insert_delete_y_replace() {
        let mut e = editor("hola mundo");
        e.set_selection(Selection::collapsed(Position::new(0, 4))).unwrap();
        e.insert(" caro").unwrap();
        assert_eq!(content(&e), "hola caro mundo");
        assert_eq!(e.selection().head, Position::new(0, 9));

        // Reemplazar selección.
        e.set_selection(Selection {
            anchor: Position::new(0, 0),
            head: Position::new(0, 4),
        })
        .unwrap();
        e.insert("adiós").unwrap();
        assert_eq!(content(&e), "adiós caro mundo");

        // Borrar selección (incluido el espacio precedente).
        e.set_selection(Selection {
            anchor: Position::new(0, 5),
            head: Position::new(0, 10),
        })
        .unwrap();
        e.delete().unwrap();
        assert_eq!(content(&e), "adiós mundo");
    }

    #[test]
    fn undo_redo_restauran_contenido_y_cursor() {
        let mut e = editor("");
        e.insert("uno\ndos").unwrap();
        assert_eq!(content(&e), "uno\ndos");
        let after_first = e.selection();
        assert_eq!(after_first.head, Position::new(1, 3));

        e.set_selection(Selection::collapsed(Position::new(0, 0))).unwrap();
        e.insert("X").unwrap();
        assert_eq!(content(&e), "Xuno\ndos");

        // Undo deshace por transacción y restaura el cursor anterior (§8.6).
        assert!(e.undo());
        assert_eq!(content(&e), "uno\ndos");
        assert_eq!(e.selection().head, Position::new(0, 0));
        assert!(e.undo());
        assert_eq!(content(&e), "");
        assert!(!e.can_undo());
        assert!(e.can_redo());

        // Redo reaplica en orden.
        assert!(e.redo());
        assert_eq!(content(&e), "uno\ndos");
        assert_eq!(e.selection(), after_first);
        assert!(e.redo());
        assert_eq!(content(&e), "Xuno\ndos");
        assert!(!e.can_redo());
    }

    #[test]
    fn historial_compartido_entre_modos() {
        // §8.5: Markdown y Visual comparten historial; cambiar de modo no lo
        // reinicia. Simulamos ediciones intercaladas de ambos modos.
        let mut e = editor("base ");
        e.set_selection(Selection::collapsed(Position::new(0, 5)))
            .unwrap();
        e.insert("desde-markdown").unwrap(); // modo Markdown
        e.set_selection(Selection::collapsed(Position::new(0, 0)))
            .unwrap(); // switch
        e.insert("«").unwrap(); // modo Visual
        assert_eq!(content(&e), "«base desde-markdown");

        assert!(e.undo()); // deshace la edición de Visual
        assert_eq!(content(&e), "base desde-markdown");
        assert!(e.undo()); // y la de Markdown, sin reinicio al cambiar de modo
        assert_eq!(content(&e), "base ");
    }

    #[test]
    fn edicion_nueva_vacia_el_redo() {
        let mut e = editor("a");
        e.insert("b").unwrap();
        assert!(e.undo());
        assert!(e.can_redo());
        e.insert("c").unwrap();
        assert!(!e.can_redo());
    }

    #[test]
    fn posiciones_unicode_y_multilinea() {
        let mut e = editor("áé\nñ");
        // Columnas en caracteres Unicode, no bytes.
        assert_eq!(e.offset_of(Position::new(0, 2)).unwrap(), "áé".len());
        e.set_selection(Selection::collapsed(Position::new(0, 2))).unwrap();
        e.insert("ó\nó").unwrap();
        assert_eq!(content(&e), "áéó\nó\nñ");
        assert_eq!(e.selection().head, Position::new(1, 1));

        // Roundtrip offset → posición.
        assert_eq!(e.position_of("áéó\n".len()), Position::new(1, 0));
    }

    #[test]
    fn posiciones_invalidas_son_error() {
        let mut e = editor("hola");
        let invalid = Selection::collapsed(Position::new(5, 0));
        assert!(matches!(
            e.set_selection(invalid),
            Err(EditError::InvalidPosition(_))
        ));
        let invalid_column = Selection::collapsed(Position::new(0, 9));
        assert!(matches!(
            e.set_selection(invalid_column),
            Err(EditError::InvalidPosition(_))
        ));
    }

    #[test]
    fn operacion_vacia_no_entra_en_historial() {
        let mut e = editor("hola");
        e.set_selection(Selection::collapsed(Position::new(0, 4))).unwrap();
        e.insert("").unwrap();
        e.delete().unwrap(); // sin selección: no-op
        assert!(!e.can_undo());
    }

    #[test]
    fn replace_all_es_una_sola_transaccion() {
        // §8.5/ISSUE-027: el reemplazo masivo entra como UNA transacción;
        // un solo undo lo revierte entero, sin transacciones parciales.
        let mut e = editor("uno dos uno tres uno");
        let edits = [
            (
                Range::new(Position::new(0, 0), Position::new(0, 3)),
                "ONE".to_string(),
            ),
            (
                Range::new(Position::new(0, 8), Position::new(0, 11)),
                "ONE".to_string(),
            ),
            (
                Range::new(Position::new(0, 17), Position::new(0, 20)),
                "ONE".to_string(),
            ),
        ];
        e.replace_all(&edits).unwrap();
        assert_eq!(content(&e), "ONE dos ONE tres ONE");

        assert!(e.undo());
        assert_eq!(content(&e), "uno dos uno tres uno", "un solo undo revierte todo");
        assert!(!e.can_undo(), "sin transacciones parciales");
        assert!(e.redo());
        assert_eq!(content(&e), "ONE dos ONE tres ONE");
        assert!(!e.can_redo());
    }

    #[test]
    fn replace_all_multilinea_con_seleccion_8_6() {
        // §8.6: la transacción masiva restaura la selección previa al
        // deshacer y deja el cursor al final del último texto insertado.
        let mut e = editor("a\nb\nc");
        let previa = Selection {
            anchor: Position::new(2, 0),
            head: Position::new(2, 1),
        };
        e.set_selection(previa).unwrap();
        let edits = [
            (
                Range::new(Position::new(0, 0), Position::new(0, 1)),
                "X\nY".to_string(),
            ),
            (
                Range::new(Position::new(2, 0), Position::new(2, 1)),
                "Z".to_string(),
            ),
        ];
        e.replace_all(&edits).unwrap();
        assert_eq!(content(&e), "X\nY\nb\nZ");
        assert_eq!(e.selection(), Selection::collapsed(Position::new(3, 1)));

        assert!(e.undo());
        assert_eq!(content(&e), "a\nb\nc");
        assert_eq!(e.selection(), previa, "undo restaura la selección (§8.6)");
    }

    #[test]
    fn replace_all_con_solape_es_error() {
        let mut e = editor("abcdef");
        let edits = [
            (
                Range::new(Position::new(0, 0), Position::new(0, 3)),
                "X".to_string(),
            ),
            (
                Range::new(Position::new(0, 2), Position::new(0, 5)),
                "Y".to_string(),
            ),
        ];
        assert!(matches!(
            e.replace_all(&edits),
            Err(EditError::OverlappingRanges(_))
        ));
        assert_eq!(content(&e), "abcdef", "sin cambios");
        assert!(!e.can_undo(), "sin transacciones");
    }

    #[test]
    fn replace_all_ordena_los_rangos() {
        let mut e = editor("aa bb aa");
        let edits = [
            (
                Range::new(Position::new(0, 6), Position::new(0, 8)),
                "X".to_string(),
            ),
            (
                Range::new(Position::new(0, 0), Position::new(0, 2)),
                "Y".to_string(),
            ),
        ];
        e.replace_all(&edits).unwrap();
        assert_eq!(content(&e), "Y bb X");
    }

    #[test]
    fn latencia_de_edicion_en_20mb() {
        // NFR-005 (<16 ms por interacción de escritura) y NFR-006 (20 MB
        // fluidos) sobre la máquina del protocolo de benchmark (ISSUE-007).
        use std::time::{Duration, Instant};

        let line = "## Título de sección\n\nPárrafo con **negrita**, `código` y [enlace](a.md).\n\n";
        let mut content = String::new();
        while content.len() < 20_000_000 {
            content.push_str(line);
        }
        let lines = content.bytes().filter(|b| *b == b'\n').count() as u32;
        let mut e = Editor::new(Document::from_text(None, content));
        e.set_selection(Selection::collapsed(Position::new(lines / 2, 3)))
            .unwrap();

        let start = Instant::now();
        e.insert("x").unwrap();
        let insert_time = start.elapsed();

        let start = Instant::now();
        assert!(e.undo());
        let undo_time = start.elapsed();

        assert!(
            insert_time < Duration::from_millis(16),
            "insert en 20 MB tardó {insert_time:?}"
        );
        assert!(
            undo_time < Duration::from_millis(16),
            "undo en 20 MB tardó {undo_time:?}"
        );
    }
}
