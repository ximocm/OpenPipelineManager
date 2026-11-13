use gtk::prelude::*;
use gtk4 as gtk;
use opm_store::{ToolDefinition, ToolOptions};
use std::cell::RefCell;
use std::rc::Rc;

use super::tool_library;

type FilterRows = Rc<RefCell<Vec<(gtk::ListBoxRow, String)>>>;

pub struct ToolPalette {
    root: gtk::Box,
    rows: gtk::ListBox,
    search: gtk::SearchEntry,
    filter_rows: FilterRows,
    status: gtk::Label,
    on_tool: Rc<dyn Fn(ToolDefinition)>,
}

impl ToolPalette {
    pub fn new(
        on_tool: Rc<dyn Fn(ToolDefinition)>,
        on_marketplace: Rc<dyn Fn()>,
        on_clear: Rc<dyn Fn()>,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_size_request(220, -1);
        let market = gtk::Button::with_label("Marketplace…");
        market.connect_clicked(move |_| on_marketplace());
        root.append(&market);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search installed tools"));
        root.append(&search);
        let status = gtk::Label::new(Some("Choose a tool to paint steps"));
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_max_width_chars(26);
        status.set_xalign(0.0);
        root.append(&status);
        let hint = gtk::Label::new(Some("Click canvas to place\nEsc to stop painting"));
        hint.set_xalign(0.0);
        hint.add_css_class("dim-label");
        root.append(&hint);
        let clear = gtk::Button::with_label("Selection tool · Esc");
        clear.connect_clicked(move |_| on_clear());
        root.append(&clear);
        let rows = gtk::ListBox::new();
        rows.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&rows)
            .build();
        root.append(&scroll);
        let filter_rows: FilterRows = Rc::new(RefCell::new(Vec::new()));
        search.connect_search_changed({
            let rows = filter_rows.clone();
            move |search| {
                let query = search.text().to_lowercase();
                for (row, text) in rows.borrow().iter() {
                    row.set_visible(text.contains(&query));
                }
            }
        });
        let palette = Rc::new(Self {
            root,
            rows,
            search,
            filter_rows,
            status,
            on_tool,
        });
        palette.refresh();
        palette
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    pub fn set_brush(&self, name: Option<&str>) {
        self.status.set_text(
            &name
                .map(|s| format!("Brush: {s}"))
                .unwrap_or_else(|| "Choose a tool to paint steps".to_owned()),
        );
    }

    pub fn refresh(&self) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        self.filter_rows.borrow_mut().clear();
        match tool_library().list() {
            Err(err) => {
                self.rows.append(&gtk::Label::new(Some(&err.to_string())));
            }
            Ok(tools) if tools.is_empty() => {
                let label = gtk::Label::new(Some(
                    "No tools installed yet.\nOpen Marketplace or import a tool JSON.",
                ));
                label.set_wrap(true);
                self.rows.append(&label);
            }
            Ok(tools) => {
                for tool in tools {
                    let row = gtk::ListBoxRow::new();
                    let button =
                        gtk::Button::with_label(&format!("{}\n{}", tool.name, tool.version));
                    button.set_tooltip_text(Some(&tool.description));
                    button.add_css_class("flat");
                    let callback = self.on_tool.clone();
                    let chosen = tool.clone();
                    button.connect_clicked(move |_| callback(chosen.clone()));
                    row.set_child(Some(&button));
                    self.rows.append(&row);
                    let text =
                        format!("{} {} {}", tool.name, tool.id, tool.description).to_lowercase();
                    row.set_visible(text.contains(&self.search.text().to_lowercase()));
                    self.filter_rows.borrow_mut().push((row, text));
                }
            }
        }
    }
}

pub fn configure(
    parent: &impl IsA<gtk::Window>,
    tool: ToolDefinition,
    on_use: Rc<dyn Fn(ToolDefinition, ToolOptions)>,
) {
    let window = gtk::Window::builder()
        .title(format!("{} · Tool brush", tool.name))
        .transient_for(parent)
        .modal(true)
        .default_width(520)
        .default_height(420)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_start(16);
    body.set_margin_end(16);
    body.set_margin_top(16);
    body.set_margin_bottom(16);
    let description = gtk::Label::new(Some(&format!(
        "{}\n\nProgram: {} · Tool definition {}",
        tool.description, tool.executable, tool.version
    )));
    description.set_wrap(true);
    description.set_xalign(0.0);
    body.append(&description);
    let options = tool.default_options();
    let mut toggles = Vec::new();
    let mut counts = Vec::new();
    for key in tool.optional_keys() {
        let check = gtk::CheckButton::with_label(&format!("Include {}", key.replace('_', " ")));
        check.set_active(options.enabled.contains(&key));
        check.set_tooltip_text(Some(&tool.parameters[&key].description));
        body.append(&check);
        toggles.push((key, check));
    }
    for (key, count) in &options.counts {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&format!("Number of {} inputs", key.replace('_', " "))));
        label.set_hexpand(true);
        label.set_xalign(0.0);
        row.append(&label);
        let spin = gtk::SpinButton::with_range(1.0, 32.0, 1.0);
        spin.set_value(*count as f64);
        row.append(&spin);
        body.append(&row);
        counts.push((key.clone(), spin));
    }
    let hint = gtk::Label::new(Some("Choose the arguments to include, then paint steps on the canvas. Edit each step to set its defaults and connect its inputs. The program must be available in your execution environment."));
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    body.append(&hint);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.connect_clicked({
        let window = window.downgrade();
        move |_| {
            if let Some(w) = window.upgrade() {
                w.close();
            }
        }
    });
    let accept = gtk::Button::with_label("Use brush");
    accept.add_css_class("suggested-action");
    accept.connect_clicked({
        let window = window.downgrade();
        move |_| {
            let options = ToolOptions {
                enabled: toggles
                    .iter()
                    .filter(|(_, toggle)| toggle.is_active())
                    .map(|(key, _)| key.clone())
                    .collect(),
                counts: counts
                    .iter()
                    .map(|(key, spin)| (key.clone(), spin.value_as_int() as usize))
                    .collect(),
            };
            if let Some(w) = window.upgrade() {
                w.close();
            }
            on_use(tool.clone(), options);
        }
    });
    buttons.append(&cancel);
    buttons.append(&accept);
    body.append(&buttons);
    let scroll = gtk::ScrolledWindow::builder().child(&body).build();
    window.set_child(Some(&scroll));
    window.present();
}
