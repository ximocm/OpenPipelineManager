// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Posiciones semánticas en el buffer (§8.6).
//!
//! El cursor y la selección se expresan como línea y columna, no como offset
//! lineal: es la posición que Markdown y Visual comparten al cambiar de modo
//! sobre el buffer único (§6.1, §8.6).

use std::cmp::Ordering;

/// Posición semántica en el buffer: línea y columna, ambas 0-based. La
/// columna cuenta caracteres Unicode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

impl Position {
    pub fn new(line: u32, column: u32) -> Self {
        Self { line, column }
    }
}

impl Ord for Position {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.line, self.column).cmp(&(other.line, other.column))
    }
}

impl PartialOrd for Position {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Rango semántico `[start, end]` de posiciones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// Selección semántica (§8.6). Con `anchor == head` es un cursor sin
/// selección; el texto seleccionado va de la posición mínima a la máxima.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Position,
    pub head: Position,
}

impl Selection {
    pub fn collapsed(at: Position) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Rango normalizado (start ≤ end).
    pub fn range(&self) -> Range {
        if self.anchor <= self.head {
            Range::new(self.anchor, self.head)
        } else {
            Range::new(self.head, self.anchor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orden_y_normalizacion() {
        let a = Position::new(1, 2);
        let b = Position::new(3, 0);
        assert!(a < b);

        let selection = Selection { anchor: b, head: a };
        assert_eq!(selection.range(), Range::new(a, b));
        assert!(Selection::collapsed(a).is_empty());
    }
}
