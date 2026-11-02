//! Panel de logs (§6, port de `LogsPanel.tsx`).
//!
//! Cabecera `Logs: <stepId>` con botón **Stop siempre visible** (§6) y
//! contenido monoespaciado con anexo en vivo: [`LogsPanel::append_line`]
//! solo añade líneas del paso mostrado (paridad con el SPA, que anexa los
//! eventos del paso seleccionado); la selección de paso o el final de la
//! ejecución recargan el contenido completo desde `store.log_content`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;

use crate::theme::Palette;

/// Panel inferior de logs.
pub struct LogsPanel {
    root: gtk::Box,
    title: gtk::Label,
    stop: gtk::Button,
    view: gtk::TextView,
    buffer: gtk::TextBuffer,
    end: gtk::TextMark,
    /// El buffer muestra el placeholder «No log output» (sin contenido real).
    placeholder: Cell<bool>,
    selected: RefCell<Option<String>>,
}

impl LogsPanel {
    /// Construye el panel; `on_stop` se invoca con el botón Stop (§6).
    pub fn new(palette: &Palette, on_stop: Rc<dyn Fn()>) -> Rc<Self> {
        // Consola «borde + cabecera» (§6): caja `opm-panel` con cabecera
        // `opm-panel-header` (título `Logs: <stepId>` + Stop) y cuerpo
        // `opm-panel-body opm-console` (suelo tintado de consola).
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("opm-panel");
        root.add_css_class("opm-logs-panel");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("opm-panel-header");
        header.add_css_class("opm-logs-header");
        let title = gtk::Label::new(Some("Logs"));
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        title.set_xalign(0.0);
        title.add_css_class("opm-logs-title");
        let stop = gtk::Button::with_label("Stop");
        stop.set_tooltip_text(Some("Stop execution"));
        stop.add_css_class("opm-stop-button");
        {
            let on_stop = Rc::clone(&on_stop);
            stop.connect_clicked(move |_| on_stop());
        }
        header.append(&title);
        header.append(&stop);
        root.append(&header);

        let view = gtk::TextView::new();
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        view.set_wrap_mode(gtk::WrapMode::None);
        view.set_left_margin(8);
        view.set_right_margin(8);
        view.set_top_margin(6);
        view.set_bottom_margin(6);
        let buffer = view.buffer();
        let end = buffer.create_mark(None, &buffer.end_iter(), false);
        let placeholder_tag = gtk::TextTag::new(Some("opm-placeholder"));
        placeholder_tag.set_foreground(Some(palette.muted));
        buffer.tag_table().add(&placeholder_tag);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        scroll.set_vexpand(true);
        scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroll.add_css_class("opm-panel-body");
        scroll.add_css_class("opm-console");
        scroll.add_css_class("opm-logs-scroll");
        root.append(&scroll);

        let panel = Rc::new(Self {
            root,
            title,
            stop,
            view,
            buffer,
            end,
            placeholder: Cell::new(true),
            selected: RefCell::new(None),
        });
        panel.set_placeholder();
        panel
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Botón Stop (§6: siempre visible; su sensibilidad la fija `main.rs`).
    pub fn stop_button(&self) -> gtk::Button {
        self.stop.clone()
    }

    /// Paso cuyo log se muestra.
    pub fn selected(&self) -> Option<String> {
        self.selected.borrow().clone()
    }

    /// Cabecera + contenido completo: `Logs: <stepId>` (o `Logs` sin paso).
    /// `content == ""` deja el placeholder «No log output» (paridad SPA).
    pub fn show_step(&self, step_id: Option<&str>, content: &str) {
        *self.selected.borrow_mut() = step_id.map(str::to_owned);
        match step_id {
            Some(id) => self.title.set_text(&format!("Logs: {id}")),
            None => self.title.set_text("Logs"),
        }
        self.set_content(content);
    }

    /// Sustituye todo el contenido (selección de paso o fin de ejecución).
    pub fn set_content(&self, content: &str) {
        self.buffer.set_text("");
        if content.is_empty() {
            self.placeholder.set(true);
            self.set_placeholder();
        } else {
            self.placeholder.set(false);
            let mut end = self.buffer.end_iter();
            self.buffer.insert(&mut end, content);
            self.scroll_end();
        }
    }

    /// Limpia el panel (inicio de ejecución, paridad con `setLogs('')`).
    pub fn clear(&self) {
        self.set_content("");
    }

    /// Anexo en vivo: solo si `step_id` es el paso mostrado (paridad SPA).
    /// Inserta `\n` como separador cuando el buffer ya tiene contenido.
    pub fn append_line(&self, step_id: &str, line: &str) {
        if self.selected.borrow().as_deref() != Some(step_id) {
            return;
        }
        if self.placeholder.get() {
            self.buffer.set_text("");
            self.placeholder.set(false);
        }
        if self.buffer.char_count() > 0 {
            let mut end = self.buffer.end_iter();
            self.buffer.insert(&mut end, "\n");
        }
        let mut end = self.buffer.end_iter();
        self.buffer.insert(&mut end, line);
        self.scroll_end();
    }

    fn set_placeholder(&self) {
        let mut end = self.buffer.end_iter();
        self.buffer
            .insert_with_tags_by_name(&mut end, "No log output", &["opm-placeholder"]);
        self.view.scroll_to_mark(&self.end, 0.0, false, 0.0, 0.0);
    }

    fn scroll_end(&self) {
        self.view.scroll_to_mark(&self.end, 0.0, false, 0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Texto completo del buffer del panel (los tests son hijos y leen los
    /// campos privados).
    fn buffer_text(buffer: &gtk::TextBuffer) -> String {
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .to_string()
    }

    /// Montaje del panel (requiere sesión gráfica: se ignora por defecto,
    /// patrón de minmark).
    #[test]
    #[ignore = "requiere sesión gráfica GTK"]
    fn panel_de_logs_se_construye_y_anexa_solo_el_paso_activo() {
        assert!(gtk::init().is_ok());
        let panel = LogsPanel::new(&crate::theme::Palette::LIGHT, Rc::new(|| {}));
        panel.show_step(Some("prepare"), "");
        assert_eq!(panel.selected().as_deref(), Some("prepare"));
        // Placeholder sin contenido.
        assert_eq!(buffer_text(&panel.buffer), "No log output");
        // El primer anexo real sustituye al placeholder; los de OTRO paso
        // se ignoran (paridad SPA).
        panel.append_line("prepare", "hola");
        panel.append_line("other", "no");
        panel.append_line("prepare", "mundo");
        assert_eq!(buffer_text(&panel.buffer), "hola\nmundo");
        // Recarga completa vacía vuelve al placeholder.
        panel.set_content("");
        assert_eq!(buffer_text(&panel.buffer), "No log output");
    }
}
