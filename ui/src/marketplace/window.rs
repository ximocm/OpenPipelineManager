use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4 as gtk;
use opm_store::{SaveMode, ToolDefinition};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use super::{
    catalog::{self, Asset, CatalogItem},
    tool_library,
};

type Rows = Rc<RefCell<Vec<(gtk::ListBoxRow, String)>>>;

fn clear(list: &gtk::ListBox, rows: &Rows) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    rows.borrow_mut().clear();
}

fn text_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.0);
    label
}

fn item_row(name: &str, description: &str, detail: &str) -> (gtk::ListBoxRow, gtk::Box) {
    let row = gtk::ListBoxRow::new();
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    let title = text_label(name);
    title.add_css_class("heading");
    text.append(&title);
    text.append(&text_label(description));
    let detail = text_label(detail);
    detail.add_css_class("dim-label");
    text.append(&detail);
    body.append(&text);
    row.set_child(Some(&body));
    (row, body)
}

async fn replace(parent: &gtk::Window, name: &str) -> bool {
    gtk::AlertDialog::builder().message(format!("Replace {name}?"))
        .detail("This replaces the installed definition. Existing pipeline steps and work folders keep their own copies.")
        .buttons(["Cancel", "Replace"]).cancel_button(0).default_button(0).modal(true).build()
        .choose_future(Some(parent)).await == Ok(1)
}

async fn install(parent: &gtk::Window, item: &CatalogItem) -> Result<bool, String> {
    match &item.asset {
        Asset::Tool(tool) => {
            let library = tool_library();
            let exists = library.root().join(format!("{}.json", tool.id)).exists();
            if exists && !replace(parent, &tool.name).await {
                return Ok(false);
            }
            library
                .save(
                    tool,
                    if exists {
                        SaveMode::Overwrite
                    } else {
                        SaveMode::CreateNew
                    },
                )
                .map_err(|e| e.to_string())?;
        }
        Asset::Pipeline(doc) => {
            let library = crate::project::library();
            let stem = crate::pipeline::design::library_stem(item.stem()).map_err(|e| e.to_string())?;
            let exists = library.root().join(format!("{stem}.pipeline")).exists();
            if exists && !replace(parent, &doc.name).await {
                return Ok(false);
            }
            let mut doc = doc.clone();
            doc.name = stem.clone();
            doc.description.push_str(&format!(
                "\n\nRegistry: {}@{} · {}",
                item.repository, item.revision, item.path
            ));
            doc.normalize();
            library
                .save(
                    &stem,
                    &doc,
                    if exists {
                        SaveMode::Overwrite
                    } else {
                        SaveMode::CreateNew
                    },
                )
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(true)
}

fn installed(
    list: &gtk::ListBox,
    rows: &Rows,
    query: &str,
    parent: &glib::WeakRef<gtk::Window>,
    on_use: &Rc<dyn Fn(ToolDefinition)>,
) {
    clear(list, rows);
    match tool_library().list() {
        Err(err) => list.append(&text_label(&err.to_string())),
        Ok(tools) if tools.is_empty() => list.append(&text_label(
            "No tools installed. Browse Marketplace or import a tool JSON.",
        )),
        Ok(tools) => {
            for tool in tools {
                let detail = format!(
                    "{} · version {} · program {}",
                    tool.id, tool.version, tool.executable
                );
                let (row, body) = item_row(&tool.name, &tool.description, &detail);
                let button = gtk::Button::with_label("Use brush");
                let callback = on_use.clone();
                let weak = parent.clone();
                let chosen = tool.clone();
                button.connect_clicked(move |_| {
                    if let Some(window) = weak.upgrade() {
                        window.close();
                    }
                    callback(chosen.clone());
                });
                body.append(&button);
                let text = format!("{} {} {}", tool.name, tool.id, tool.description).to_lowercase();
                row.set_visible(text.contains(query));
                list.append(&row);
                rows.borrow_mut().push((row, text));
            }
        }
    }
}

pub fn open(
    parent: &impl IsA<gtk::Window>,
    on_change: Rc<dyn Fn()>,
    on_use: Rc<dyn Fn(ToolDefinition)>,
) {
    let window = gtk::Window::builder()
        .title("Tools & Pipeline Marketplace")
        .transient_for(parent)
        .default_width(860)
        .default_height(650)
        .build();
    let weak = window.downgrade();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_start(16);
    body.set_margin_end(16);
    body.set_margin_top(16);
    body.set_margin_bottom(16);
    let heading = text_label("Add reusable tools and pipelines");
    heading.add_css_class("title-2");
    body.append(&heading);
    body.append(&text_label("Install definitions here, then choose a tool and paint steps on a design canvas. Programs are provided by your execution environment."));
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search tools and pipelines"));
    body.append(&search);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let import = gtk::Button::with_label("Import tool JSON…");
    actions.append(&import);
    let docs = gtk::LinkButton::with_label(
        "https://github.com/ximocm/opm-registry#readme",
        "Registry documentation",
    );
    actions.append(&docs);
    body.append(&actions);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    let switcher = gtk::StackSwitcher::new();
    switcher.set_stack(Some(&stack));
    body.append(&switcher);
    let installed_list = gtk::ListBox::new();
    installed_list.set_selection_mode(gtk::SelectionMode::None);
    let installed_rows: Rows = Rc::new(RefCell::new(Vec::new()));
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&installed_list)
        .build();
    stack.add_titled(&scroll, Some("installed"), "Installed tools");
    let catalog_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let source = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let repository = gtk::Entry::builder()
        .text(catalog::DEFAULT_REPOSITORY)
        .placeholder_text("owner/repository")
        .hexpand(true)
        .build();
    let reference = gtk::Entry::builder()
        .text(catalog::DEFAULT_REF)
        .placeholder_text("Branch, tag or commit")
        .width_chars(14)
        .build();
    let refresh = gtk::Button::with_label("Refresh catalog");
    source.append(&repository);
    source.append(&reference);
    source.append(&refresh);
    catalog_box.append(&source);
    let spinner = gtk::Spinner::new();
    catalog_box.append(&spinner);
    let status =
        text_label("Browse a public GitHub registry. Installed tools remain available offline.");
    catalog_box.append(&status);
    let remote_list = gtk::ListBox::new();
    remote_list.set_selection_mode(gtk::SelectionMode::None);
    let remote_rows: Rows = Rc::new(RefCell::new(Vec::new()));
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&remote_list)
        .build();
    catalog_box.append(&scroll);
    stack.add_titled(&catalog_box, Some("catalog"), "Marketplace");
    body.append(&stack);
    let location = text_label(&format!(
        "Installed tools: {}",
        tool_library().root().display()
    ));
    location.add_css_class("dim-label");
    body.append(&location);
    window.set_child(Some(&body));
    search.connect_search_changed({
        let installed = installed_rows.clone();
        let remote = remote_rows.clone();
        move |entry| {
            let query = entry.text().to_lowercase();
            for rows in [&installed, &remote] {
                for (row, text) in rows.borrow().iter() {
                    row.set_visible(text.contains(&query));
                }
            }
        }
    });
    let refresh_installed: Rc<dyn Fn()> = Rc::new({
        let list = installed_list.clone();
        let rows = installed_rows.clone();
        let search = search.clone();
        let weak = weak.clone();
        let on_use = on_use.clone();
        move || installed(&list, &rows, &search.text().to_lowercase(), &weak, &on_use)
    });
    refresh_installed();
    import.connect_clicked({
        let weak = weak.clone();
        let on_change = on_change.clone();
        let update = refresh_installed.clone();
        move |_| {
            let Some(parent) = weak.upgrade() else { return };
            let change = on_change.clone();
            let update = update.clone();
            glib::spawn_future_local(async move {
                let dialog = gtk::FileDialog::builder()
                    .title("Import tool definition")
                    .modal(true)
                    .build();
                let filter = gtk::FileFilter::new();
                filter.set_name(Some("Tool definitions (*.json)"));
                filter.add_pattern("*.json");
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&filter);
                dialog.set_filters(Some(&filters));
                let Ok(file) = dialog.open_future(Some(&parent)).await else {
                    return;
                };
                let Some(path) = file.path() else { return };
                let result = std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|text| ToolDefinition::from_json(&text).map_err(|e| e.to_string()));
                let tool = match result {
                    Ok(tool) => tool,
                    Err(e) => {
                        crate::show_error(&parent, &e);
                        return;
                    }
                };
                let item = CatalogItem {
                    path: format!("tools/{}.json", tool.id),
                    repository: String::new(),
                    revision: String::new(),
                    asset: Asset::Tool(tool),
                };
                match install(&parent, &item).await {
                    Ok(true) => {
                        update();
                        change();
                    }
                    Ok(false) => {}
                    Err(e) => crate::show_error(&parent, &e),
                }
            });
        }
    });
    refresh.connect_clicked({
        let weak = weak.clone();
        let query = search.clone();
        let rows = remote_rows.clone();
        let list = remote_list.clone();
        let repository = repository.clone();
        let reference = reference.clone();
        let spinner = spinner.clone();
        let status = status.clone();
        let update = refresh_installed.clone();
        let on_change = on_change.clone();
        move |refresh| {
            let repository_text = repository.text().trim().to_owned();
            let reference_text = reference.text().trim().to_owned();
            if let Err(e) = catalog::validate_repository(&repository_text) {
                status.set_text(&e);
                return;
            }
            refresh.set_sensitive(false);
            spinner.start();
            clear(&list, &rows);
            status.set_text("Loading catalog…");
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(catalog::fetch(&repository_text, &reference_text));
            });
            let weak = weak.clone();
            let query = query.clone();
            let rows = rows.clone();
            let list = list.clone();
            let refresh = refresh.clone();
            let spinner = spinner.clone();
            let status = status.clone();
            let update = update.clone();
            let change = on_change.clone();
            glib::timeout_add_local(Duration::from_millis(50), move || {
                let Some(parent) = weak.upgrade().filter(|w| w.is_visible()) else {
                    return glib::ControlFlow::Break;
                };
                let result = match receiver.try_recv() {
                    Ok(result) => result,
                    Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(_) => Err("Catalog request stopped unexpectedly".to_owned()),
                };
                refresh.set_sensitive(true);
                spinner.stop();
                match result {
                    Err(e) => {
                        status.set_text(&format!("{e}\nYour installed tools are still available."))
                    }
                    Ok(items) => {
                        status.set_text(&format!("{} definitions available", items.len()));
                        for item in items {
                            let version = match &item.asset {
                                Asset::Tool(tool) => {
                                    format!("Tool {} · program {}", tool.version, tool.executable)
                                }
                                Asset::Pipeline(_) => "Pipeline".to_owned(),
                            };
                            let detail = format!(
                                "{version} · {} · {}",
                                item.repository,
                                &item.revision[..12]
                            );
                            let (row, body) = item_row(item.name(), item.description(), &detail);
                            let exists = match &item.asset {
                                Asset::Tool(tool) => tool_library()
                                    .root()
                                    .join(format!("{}.json", tool.id))
                                    .exists(),
                                Asset::Pipeline(_) => crate::project::library_root()
                                    .join(format!("{}.pipeline", item.stem()))
                                    .exists(),
                            };
                            let button = gtk::Button::with_label(if exists {
                                "Update"
                            } else {
                                "Add to OPM"
                            });
                            let weak = parent.downgrade();
                            let update = update.clone();
                            let change = change.clone();
                            let chosen = item.clone();
                            button.connect_clicked(move |button| {
                                let Some(parent) = weak.upgrade() else { return };
                                let item = chosen.clone();
                                let update = update.clone();
                                let change = change.clone();
                                let button = button.downgrade();
                                glib::spawn_future_local(async move {
                                    match install(&parent, &item).await {
                                        Ok(true) => {
                                            update();
                                            change();
                                            if let Some(button) = button.upgrade() {
                                                button.set_label("Installed");
                                                button.set_sensitive(false);
                                            }
                                        }
                                        Ok(false) => {}
                                        Err(e) => crate::show_error(&parent, &e),
                                    }
                                });
                            });
                            body.append(&button);
                            let text = format!(
                                "{} {} {} {}",
                                item.name(),
                                item.description(),
                                item.kind(),
                                item.path
                            )
                            .to_lowercase();
                            row.set_visible(text.contains(&query.text().to_lowercase()));
                            list.append(&row);
                            rows.borrow_mut().push((row, text));
                        }
                    }
                }
                glib::ControlFlow::Break
            });
        }
    });
    stack.set_visible_child_name("catalog");
    window.present();
    refresh.emit_clicked();
}
