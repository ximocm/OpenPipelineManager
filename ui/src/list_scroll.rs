// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Mantiene visible la selección de listas navegadas desde otro widget.
//! Lo consume el resumen de validación (`crate::runs::summary`), que navega
//! su `gtk::ListBox` desde el popover de contadores.
use gtk::prelude::*;
use gtk4 as gtk;

/// Mantiene visible la selección de listas navegadas desde otro widget.
pub fn follow_selection(list: &gtk::ListBox) {
    list.connect_row_selected(|list, row| {
        if row.is_none() {
            return;
        }
        reveal_selected(list);
        // Tras repintar, la fila puede no tener aún su tamaño definitivo.
        list.add_tick_callback(|list, _| {
            reveal_selected(list);
            gtk::glib::ControlFlow::Break
        });
    });
}

/// Desplaza el scroll hasta la fila seleccionada (ayudante de
/// [`follow_selection`]).
fn reveal_selected(list: &gtk::ListBox) {
    let Some(row) = list.selected_row() else {
        return;
    };
    let Some(scroll) = list
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok())
    else {
        return;
    };
    let Some(bounds) = row.compute_bounds(list) else {
        return;
    };
    if bounds.height() > 0.0 {
        scroll
            .vadjustment()
            .clamp_page(bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
    }
}
