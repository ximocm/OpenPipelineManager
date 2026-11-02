// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Barra de búsqueda y reemplazo del archivo actual (§40.1, §39, ISSUE-027).
//!
//! `Ctrl+F` la abre en modo búsqueda y `Ctrl+H` con la fila de reemplazo
//! (§39). Sobre el tab activo ofrece los modos §40.1 — «Aa» (case
//! sensitive), «palabra» (whole word), «.*» (regex) — el contador `n/total`,
//! la navegación anterior/siguiente **con wrap** y el reemplazo de la
//! coincidencia activa o de todas (una sola transacción §8.5 vía
//! [`opm_document::Editor::replace_all`]).
//!
//! La lógica ([`FindState`]) es pura y sin GTK (patrón de [`crate::editor::tags`]):
//! los tests no inicializan widgets. [`FindBar`] materializa el estado en
//! widgets y emite [`FindAction`] al integrador (patrón de
//! [`crate::explorer::ExplorerPane`]); una regex inválida es un error NO
//! bloqueante (etiqueta en la barra) que conserva la última búsqueda válida
//! con sus resultados (§40.1 criterio 4).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, Button, Entry, Label, Orientation, ToggleButton, Widget};

use opm_document::{Editor, Matcher, Position, Range, SearchOptions};

/// Acciones de la barra hacia el integrador (main.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindAction {
    /// La consulta o las opciones cambiaron: rebuscar.
    Search,
    /// Siguiente coincidencia (§40.1).
    Next,
    /// Coincidencia anterior (§40.1).
    Previous,
    /// Reemplazar la coincidencia activa y avanzar (§40.1).
    Replace,
    /// Reemplazar todas las coincidencias en una transacción (§8.5).
    ReplaceAll,
    /// Cerrar la barra y limpiar el marcado (§40.1).
    Close,
}

/// Receptor de acciones de la barra (patrón de [`crate::explorer`]).
pub type FindSink = Rc<dyn Fn(FindAction)>;

/// Estado de la búsqueda del archivo actual (§40.1): lógica pura, sin GTK.
#[derive(Debug)]
pub struct FindState {
    pub query: String,
    pub options: SearchOptions,
    pub replacement: String,
    /// La fila de reemplazo es visible (`Ctrl+H`, §39).
    pub show_replace: bool,
    /// Ancla de búsqueda: la activa es la primera coincidencia en o después
    /// de esta posición (cursor al abrir o fin del último reemplazo). No se
    /// mueve al teclear la consulta: solo al editar o reemplazar.
    anchor: Position,
    matches: Vec<Range>,
    active: Option<usize>,
    error: Option<String>,
}

impl Default for FindState {
    fn default() -> Self {
        Self {
            query: String::new(),
            options: SearchOptions::default(),
            replacement: String::new(),
            show_replace: false,
            anchor: Position::new(0, 0),
            matches: Vec::new(),
            active: None,
            error: None,
        }
    }
}

impl FindState {
    /// Fija el ancla de búsqueda (§40.1).
    pub fn set_anchor(&mut self, anchor: Position) {
        self.anchor = anchor;
    }

    /// Recalcula las coincidencias sobre `editor` (§8.6: rangos semánticos).
    ///
    /// La activa es la primera que empieza en o después del ancla (con wrap a
    /// la primera). Una regex inválida o una búsqueda demasiado compleja
    /// **conservan** la última búsqueda válida con sus resultados y solo fijan
    /// [`FindState::error`] (criterio 4: error no bloqueante).
    pub fn refresh(&mut self, editor: &Editor) {
        if self.query.is_empty() {
            self.matches.clear();
            self.active = None;
            self.error = None;
            return;
        }
        let matcher = match Matcher::new(&self.query, self.options) {
            Ok(matcher) => matcher,
            Err(err) => {
                self.error = Some(err.to_string());
                return;
            }
        };
        match matcher.find_all(editor) {
            Ok(found) => {
                self.error = None;
                self.active = if found.is_empty() {
                    None
                } else {
                    Some(
                        found
                            .iter()
                            .position(|m| m.start >= self.anchor)
                            .unwrap_or(0),
                    )
                };
                self.matches = found;
            }
            Err(err) => self.error = Some(err.to_string()),
        }
    }

    /// Contador `n/total` (§40.1); sin coincidencias, `0/0`.
    pub fn counter_text(&self) -> String {
        let total = self.matches.len();
        let current = self.active.map(|index| index + 1).unwrap_or(0);
        format!("{current}/{total}")
    }

    /// Siguiente coincidencia **con wrap** (§40.1).
    pub fn next(&mut self) -> Option<Range> {
        let total = self.matches.len();
        if total == 0 {
            return None;
        }
        let index = self.active.map(|i| (i + 1) % total).unwrap_or(0);
        self.active = Some(index);
        Some(self.matches[index])
    }

    /// Coincidencia anterior **con wrap** (§40.1).
    pub fn previous(&mut self) -> Option<Range> {
        let total = self.matches.len();
        if total == 0 {
            return None;
        }
        let index = match self.active {
            Some(0) | None => total - 1,
            Some(i) => i - 1,
        };
        self.active = Some(index);
        Some(self.matches[index])
    }

    /// Rango activo, si lo hay.
    pub fn active_match(&self) -> Option<Range> {
        self.active.map(|index| self.matches[index])
    }

    /// Índice de la coincidencia activa (para el marcado del `TextBuffer`).
    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    /// Reemplazo de la coincidencia activa (§40.1): `(rango, texto)`.
    pub fn replace_edit(&self) -> Option<(Range, String)> {
        self.active_match()
            .map(|range| (range, self.replacement.clone()))
    }

    /// Todos los reemplazos para [`opm_document::Editor::replace_all`]
    /// (una sola transacción §8.5).
    pub fn replace_all_edits(&self) -> Vec<(Range, String)> {
        self.matches
            .iter()
            .map(|range| (*range, self.replacement.clone()))
            .collect()
    }

    /// Todas las coincidencias marcadas (§40.1: resaltado discreto).
    pub fn matches(&self) -> &[Range] {
        &self.matches
    }

    /// Error visible no bloqueante (criterio 4), si lo hay.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Limpia resultados y error (al cerrar la barra, §40.1). La consulta y
    /// las opciones se conservan para reabrir.
    pub fn clear_results(&mut self) {
        self.matches.clear();
        self.active = None;
        self.error = None;
    }
}

/// Barra de búsqueda/reemplazo (§40.1): widgets GTK sobre [`FindState`].
pub struct FindBar {
    root: GtkBox,
    search_entry: Entry,
    replace_row: GtkBox,
    counter: Label,
    error_label: Label,
    state: Rc<RefCell<FindState>>,
    open: Cell<bool>,
}

impl FindBar {
    /// Construye la barra (oculta) y conecta sus controles con `on_action`.
    pub fn new(on_action: FindSink) -> Self {
        let state = Rc::new(RefCell::new(FindState::default()));

        let root = GtkBox::new(Orientation::Vertical, 2);
        root.add_css_class("opm-find-bar");

        let row = GtkBox::new(Orientation::Horizontal, 4);
        let search_entry = Entry::new();
        search_entry.set_placeholder_text(Some("Buscar"));
        search_entry.set_hexpand(true);
        {
            let state = Rc::clone(&state);
            let on_action = Rc::clone(&on_action);
            search_entry.connect_changed(move |entry| {
                state.borrow_mut().query = entry.text().to_string();
                on_action(FindAction::Search);
            });
        }

        let case_button = ToggleButton::with_label("Aa");
        case_button.set_tooltip_text(Some("Distinguir mayúsculas (case sensitive)"));
        {
            let state = Rc::clone(&state);
            let on_action = Rc::clone(&on_action);
            case_button.connect_toggled(move |button| {
                state.borrow_mut().options.case_sensitive = button.is_active();
                on_action(FindAction::Search);
            });
        }
        let word_button = ToggleButton::with_label("palabra");
        word_button.set_tooltip_text(Some("Solo palabras enteras (whole word)"));
        {
            let state = Rc::clone(&state);
            let on_action = Rc::clone(&on_action);
            word_button.connect_toggled(move |button| {
                state.borrow_mut().options.whole_word = button.is_active();
                on_action(FindAction::Search);
            });
        }
        let regex_button = ToggleButton::with_label(".*");
        regex_button.set_tooltip_text(Some("Expresión regular (regex)"));
        {
            let state = Rc::clone(&state);
            let on_action = Rc::clone(&on_action);
            regex_button.connect_toggled(move |button| {
                state.borrow_mut().options.regex = button.is_active();
                on_action(FindAction::Search);
            });
        }

        let counter = Label::new(Some("0/0"));
        counter.set_tooltip_text(Some("Coincidencia activa / total"));
        {
            let previous = Button::with_label("Anterior");
            {
                let on_action = Rc::clone(&on_action);
                previous.connect_clicked(move |_| on_action(FindAction::Previous));
            }
            let next = Button::with_label("Siguiente");
            {
                let on_action = Rc::clone(&on_action);
                next.connect_clicked(move |_| on_action(FindAction::Next));
            }
            let close = Button::with_label("Cerrar");
            {
                let on_action = Rc::clone(&on_action);
                close.connect_clicked(move |_| on_action(FindAction::Close));
            }

            row.append(&search_entry);
            row.append(&case_button);
            row.append(&word_button);
            row.append(&regex_button);
            row.append(&counter);
            row.append(&previous);
            row.append(&next);
            row.append(&close);
        }

        let replace_row = GtkBox::new(Orientation::Horizontal, 4);
        let replace_entry = Entry::new();
        replace_entry.set_placeholder_text(Some("Reemplazar"));
        replace_entry.set_hexpand(true);
        {
            let state = Rc::clone(&state);
            replace_entry.connect_changed(move |entry| {
                state.borrow_mut().replacement = entry.text().to_string();
            });
        }
        {
            let replace = Button::with_label("Reemplazar");
            {
                let on_action = Rc::clone(&on_action);
                replace.connect_clicked(move |_| on_action(FindAction::Replace));
            }
            let replace_all = Button::with_label("Reemplazar todo");
            {
                let on_action = Rc::clone(&on_action);
                replace_all.connect_clicked(move |_| on_action(FindAction::ReplaceAll));
            }
            replace_row.append(&replace_entry);
            replace_row.append(&replace);
            replace_row.append(&replace_all);
        }

        let error_label = Label::new(None);
        error_label.add_css_class("error");
        error_label.set_xalign(0.0);
        error_label.set_visible(false);

        root.append(&row);
        root.append(&replace_row);
        root.append(&error_label);
        root.set_visible(false);
        replace_row.set_visible(false);

        Self {
            root,
            search_entry,
            replace_row,
            counter,
            error_label,
            state,
            open: Cell::new(false),
        }
    }

    /// Widget raíz de la barra.
    pub fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    /// Abre la barra: `Ctrl+F` solo búsqueda, `Ctrl+H` con la fila de
    /// reemplazo (§39). El error se limpia; la consulta persiste.
    pub fn open(&self, show_replace: bool) {
        self.state.borrow_mut().show_replace = show_replace;
        self.replace_row.set_visible(show_replace);
        self.root.set_visible(true);
        self.open.set(true);
        self.set_error(None);
    }

    /// Oculta la barra (el marcado lo limpia el integrador, §40.1).
    pub fn close(&self) {
        self.root.set_visible(false);
        self.open.set(false);
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Estado de la búsqueda (§40.1).
    pub fn state(&self) -> &RefCell<FindState> {
        &self.state
    }

    /// Actualiza el contador `n/total`.
    pub fn set_counter(&self, text: &str) {
        self.counter.set_text(text);
    }

    /// Error no bloqueante visible en la barra (criterio 4); `None` lo oculta.
    pub fn set_error(&self, message: Option<&str>) {
        match message {
            Some(message) => {
                self.error_label.set_text(message);
                self.error_label.set_visible(true);
            }
            None => {
                self.error_label.set_visible(false);
                self.error_label.set_text("");
            }
        }
    }

    /// Foco en el campo de búsqueda (§40.1: teclear busca de inmediato).
    pub fn focus_search(&self) {
        self.search_entry.grab_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opm_document::{Document, Selection};

    fn editor(text: &str) -> Editor {
        Editor::new(Document::from_text(None, text.to_string()))
    }

    fn state_with(text: &str, query: &str) -> (Editor, FindState) {
        let mut e = editor(text);
        let mut state = FindState {
            query: query.to_string(),
            ..FindState::default()
        };
        state.refresh(&e);
        // Colocamos el cursor al principio para las navegaciones.
        e.set_selection(Selection::collapsed(Position::new(0, 0)))
            .unwrap();
        (e, state)
    }

    #[test]
    fn navegacion_siguiente_y_anterior_con_wrap() {
        // §40.1: siguiente/anterior con wrap sobre el archivo actual.
        // «aa xx aa xx aa»: coincidencias en las columnas 0, 6 y 12.
        let (_e, mut state) = state_with("aa xx aa xx aa", "aa");
        assert_eq!(state.counter_text(), "1/3");
        assert_eq!(state.next().unwrap().start, Position::new(0, 6));
        assert_eq!(state.next().unwrap().start, Position::new(0, 12));
        assert_eq!(
            state.next().unwrap().start,
            Position::new(0, 0),
            "wrap al principio"
        );
        assert_eq!(
            state.previous().unwrap().start,
            Position::new(0, 12),
            "wrap al final"
        );
        assert_eq!(state.previous().unwrap().start, Position::new(0, 6));
        assert_eq!(state.previous().unwrap().start, Position::new(0, 0));
    }

    #[test]
    fn contador_n_sobre_total() {
        let (_e, mut state) = state_with("uno dos uno", "uno");
        assert_eq!(state.counter_text(), "1/2");
        state.next();
        assert_eq!(state.counter_text(), "2/2");

        let (_e, vacio) = state_with("uno dos", "tres");
        assert_eq!(vacio.counter_text(), "0/0");
    }

    #[test]
    fn regex_invalida_conserva_la_ultima_busqueda_valida() {
        // §40.1 criterio 4: error no bloqueante sin perder resultados.
        let (e, mut state) = state_with("hola hola", "hola");
        assert_eq!(state.matches().len(), 2);
        assert!(state.error().is_none());

        state.query = "[a-".to_string();
        state.options.regex = true;
        state.refresh(&e);
        assert!(state.error().is_some(), "regex inválida avisa");
        assert!(state.error().unwrap().contains("regex inválida"));
        assert_eq!(state.matches().len(), 2, "conserva los resultados previos");
        assert_eq!(state.counter_text(), "1/2");

        // Al volver a una consulta válida se recupera el flujo.
        state.query = "hola".to_string();
        state.options.regex = false;
        state.refresh(&e);
        assert!(state.error().is_none());
        assert_eq!(state.matches().len(), 2);
    }

    #[test]
    fn planes_de_reemplazo() {
        let (_e, mut state) = state_with("a b a", "a");
        state.replacement = "X".to_string();
        assert_eq!(
            state.replace_edit().unwrap(),
            (
                Range::new(Position::new(0, 0), Position::new(0, 1)),
                "X".to_string()
            )
        );
        let todos = state.replace_all_edits();
        assert_eq!(todos.len(), 2);
        assert_eq!(
            todos[1].0,
            Range::new(Position::new(0, 4), Position::new(0, 5))
        );
    }

    #[test]
    fn refresh_con_ancla_tras_reemplazo() {
        // Tras reemplazar, el ancla es el final del texto insertado: la activa
        // es la primera coincidencia en o después («Reemplazar» avanza).
        let e = editor("aa aa aa");
        let mut state = FindState {
            query: "aa".to_string(),
            ..FindState::default()
        };
        state.set_anchor(Position::new(0, 3));
        state.refresh(&e);
        assert_eq!(state.counter_text(), "2/3");
        state.set_anchor(Position::new(0, 100));
        state.refresh(&e);
        assert_eq!(
            state.counter_text(),
            "1/3",
            "sin nada después: wrap a la primera"
        );
    }

    #[test]
    fn consulta_vacia_y_cierre_limpian_resultados() {
        let (e, mut state) = state_with("aa aa", "aa");
        assert_eq!(state.matches().len(), 2);
        state.query = String::new();
        state.refresh(&e);
        assert_eq!(state.counter_text(), "0/0");

        state.query = "aa".to_string();
        state.refresh(&e);
        state.clear_results();
        assert_eq!(state.matches().len(), 0);
        assert!(state.active_match().is_none());
    }
}
