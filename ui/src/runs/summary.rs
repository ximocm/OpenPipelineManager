//! Contadores del toolbar y panel-resumen de validación (§6 `runs/`).
//!
//! Dos botones `N blockers` / `M warnings` (paridad con la status strip del
//! SPA, `App.tsx`) abren un popover con la lista de issues: `gtk::ListBox`
//! con [`crate::list_scroll::follow_selection`] y activación por fila que
//! lleva al paso del issue (`on_select`). El texto de cada fila es el
//! [`format_blocker`] (`step.field: msg`) del formateo SPA.

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;

use opm_pipeline::{IssueSeverity, ValidationIssue};

use super::{count_severities, format_blocker};
use crate::list_scroll::follow_selection;

/// Contadores + popover-resumen de validación del toolbar.
pub struct Counters {
    root: gtk::Box,
    blockers_label: gtk::Label,
    warnings_label: gtk::Label,
    blocker_popover: gtk::Popover,
    warning_popover: gtk::Popover,
    blocker_list: gtk::ListBox,
    warning_list: gtk::ListBox,
}

impl Counters {
    /// `on_select(step_id)` al activar una fila del resumen (doble clic o
    /// Enter sobre la fila).
    pub fn new(on_select: Rc<dyn Fn(String)>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        root.add_css_class("opm-counters");

        let blockers_label = gtk::Label::new(Some("0 blockers"));
        blockers_label.add_css_class("opm-counter");
        let warnings_label = gtk::Label::new(Some("0 warnings"));
        warnings_label.add_css_class("opm-counter");

        let (blocker_popover, blocker_list) = build_summary_popover(&on_select);
        let (warning_popover, warning_list) = build_summary_popover(&on_select);

        let blockers_button = counter_button(&blockers_label, blocker_popover.clone());
        let warnings_button = counter_button(&warnings_label, warning_popover.clone());
        root.append(&blockers_button);
        root.append(&warnings_button);

        Rc::new(Self {
            root,
            blockers_label,
            warnings_label,
            blocker_popover,
            warning_popover,
            blocker_list,
            warning_list,
        })
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Actualiza los contadores y el contenido de ambos popovers.
    pub fn set_issues(&self, issues: &[ValidationIssue]) {
        let (blockers, warnings) = count_severities(issues);
        self.blockers_label.set_text(&format!("{blockers} blockers"));
        self.warnings_label.set_text(&format!("{warnings} warnings"));
        set_hot(&self.blockers_label, blockers > 0);
        set_hot(&self.warnings_label, warnings > 0);

        fill_list(&self.blocker_list, issues);
        fill_list(&self.warning_list, issues);
    }

    /// Cierra los popovers (p. ej. antes de cerrar la ventana).
    pub fn popdown(&self) {
        self.blocker_popover.popdown();
        self.warning_popover.popdown();
    }
}

/// Botón contador con su popover atado (patrón `gtk::MenuButton`: clic →
/// popover anclado al propio botón).
fn counter_button(label: &gtk::Label, popover: gtk::Popover) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_child(Some(label));
    button.set_popover(Some(&popover));
    button.set_tooltip_text(Some("Validation summary"));
    button.add_css_class("opm-counter-button");
    button
}

/// Construye un popover-resumen: lista `gtk::ListBox` con selección que se
/// mantiene visible (`crate::list_scroll::follow_selection`).
fn build_summary_popover(on_select: &Rc<dyn Fn(String)>) -> (gtk::Popover, gtk::ListBox) {
    let popover = gtk::Popover::new();
    popover.add_css_class("opm-validation-popover");
    popover.set_position(gtk::PositionType::Bottom);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("opm-validation-list");
    follow_selection(&list);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&list));
    scroll.set_min_content_height(96);
    scroll.set_min_content_width(420);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    popover.set_child(Some(&scroll));

    // Activar una fila (doble clic / Enter) lleva al paso del issue; la fila
    // lleva el paso en su tooltip (las filas sin paso no seleccionan nada).
    {
        let on_select = Rc::clone(on_select);
        let popover = popover.clone();
        list.connect_row_activated(move |_list, row| {
            if let Some(step_id) = row
                .child()
                .and_then(|child| child.tooltip_text())
                .map(|text| text.to_string())
                .filter(|text| !text.is_empty())
            {
                on_select(step_id);
            }
            popover.popdown();
        });
    }

    (popover, list)
}

/// Vacía `list` y la rellena con los issues (fila = `step.field: msg`,
/// coloreada por severidad; el paso va en el tooltip para la activación).
fn fill_list(list: &gtk::ListBox, issues: &[ValidationIssue]) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    for issue in issues {
        let label = gtk::Label::new(Some(&format_blocker(issue)));
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Start);
        label.set_margin_start(8);
        label.set_margin_end(8);
        label.set_margin_top(3);
        label.set_margin_bottom(3);
        label.set_max_width_chars(56);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.add_css_class(match issue.severity {
            IssueSeverity::Blocker => "opm-issue-blocker",
            IssueSeverity::Warning => "opm-issue-warning",
        });
        if let Some(step) = &issue.step_id {
            label.set_tooltip_text(Some(step));
        }
        list.append(&label);
    }
}

/// «Caliente» = hay algo que contar (resaltado del contador §6).
fn set_hot(label: &gtk::Label, hot: bool) {
    if hot {
        label.add_css_class("opm-counter-hot");
    } else {
        label.remove_css_class("opm-counter-hot");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(severity: IssueSeverity, step: Option<&str>, field: Option<&str>) -> ValidationIssue {
        ValidationIssue {
            severity,
            message: "msg".to_owned(),
            step_id: step.map(str::to_owned),
            field: field.map(str::to_owned),
        }
    }

    fn row_count(list: &gtk::ListBox) -> usize {
        let mut rows = 0;
        let mut child = list.first_child();
        while let Some(row) = child {
            rows += 1;
            child = row.next_sibling();
        }
        rows
    }

    /// Contadores + filas del resumen (montaje GTK; requiere sesión gráfica).
    #[test]
    #[ignore = "requiere sesión gráfica GTK"]
    fn contadores_y_resumen_se_construyen() {
        assert!(gtk::init().is_ok());
        let counters = Counters::new(Rc::new(|_step: String| {}));
        assert_eq!(counters.blockers_label.text().as_str(), "0 blockers");
        assert_eq!(counters.warnings_label.text().as_str(), "0 warnings");
        let issues = vec![
            issue(IssueSeverity::Blocker, Some("analyze"), Some("input_file")),
            issue(IssueSeverity::Warning, None, None),
        ];
        counters.set_issues(&issues);
        assert_eq!(counters.blockers_label.text().as_str(), "1 blockers");
        assert_eq!(counters.warnings_label.text().as_str(), "1 warnings");
        assert_eq!(row_count(&counters.blocker_list), 2);
        assert_eq!(row_count(&counters.warning_list), 2);
        // Sin issues vuelve a cero.
        counters.set_issues(&[]);
        assert_eq!(counters.blockers_label.text().as_str(), "0 blockers");
        assert_eq!(row_count(&counters.blocker_list), 0);
    }
}
