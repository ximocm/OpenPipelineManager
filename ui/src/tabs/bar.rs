// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Barra de pestañas (§37).
//!
//! Barra `gtk::Box` horizontal con un botón por pestaña rotulado con
//! [`TabManager::title`] («● nombre» si hay cambios sin guardar). La barra se
//! oculta con un solo documento abierto (§37).

use crate::tabs::TabManager;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Barra de pestañas GTK (§37). `sync` la re-renderiza desde el modelo.
pub struct TabBar {
    root: gtk::Box,
    on_select: Rc<dyn Fn(usize)>,
    /// Ventana de proyecto (§11 A11): la barra SIEMPRE queda visible porque
    /// la página de texto debe seguir alcanzable junto a la pestaña
    /// «Pipeline» permanente, con un solo documento o no.
    persistent: Cell<bool>,
}

impl TabBar {
    /// Crea la barra; `on_select(index)` se invoca al pulsar la pestaña
    /// `index`. La visibilidad la fija [`TabBar::sync`] (§37).
    pub fn new(on_select: Rc<dyn Fn(usize)>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_visible(false);
        Self {
            root,
            on_select,
            persistent: Cell::new(false),
        }
    }

    /// Fija la visibilidad fija de la barra (ventanas de proyecto, A11).
    pub fn set_persistent(&self, persistent: bool) {
        self.persistent.set(persistent);
    }

    /// Widget raíz de la barra, para insertarla en la ventana.
    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Re-renderiza los botones desde `manager` (§37): un botón por pestaña
    /// con `title(index)`, el activo resaltado solo cuando el editor es la
    /// página visible (A11), y `set_visible(len > 1)` salvo barra persistente.
    pub fn sync(&self, manager: &TabManager, editor_focused: bool) {
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }
        for index in 0..manager.len() {
            let button = gtk::Button::with_label(&manager.title(index));
            if index == manager.active_index() && editor_focused {
                // §37: el tab activo va resaltado.
                button.add_css_class("suggested-action");
            }
            let on_select = Rc::clone(&self.on_select);
            button.connect_clicked(move |_| on_select(index));
            self.root.append(&button);
        }
        self.root
            .set_visible(self.persistent.get() || manager.len() > 1);
    }
}
