//! Native launch page. Actions reuse the window's existing library/work flows.
use gtk::prelude::*;
use gtk4 as gtk;
use std::path::PathBuf;
use std::rc::Rc;

use crate::project::Restore;

fn text(value: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(value));
    label.add_css_class(class);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label
}

fn action(label: &str, name: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.set_action_name(Some(name));
    button
}

fn card(icon: &str, title: &str, description: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    card.add_css_class("opm-welcome-card");
    card.set_size_request(264, -1);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(28);
    image.set_halign(gtk::Align::Start);
    card.append(&image);
    card.append(&text(title, "opm-step-name"));
    let description = text(description, "opm-dim");
    description.set_max_width_chars(30);
    card.append(&description);
    card
}

pub fn build(recent: Restore, on_recent: Rc<dyn Fn(PathBuf)>) -> gtk::ScrolledWindow {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    content.set_halign(gtk::Align::Center);
    content.set_valign(gtk::Align::Center);
    content.set_margin_top(40);
    content.set_margin_bottom(40);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.add_css_class("opm-welcome");

    let heading = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let name = text("Open Pipeline Manager", "opm-welcome-title");
    name.set_xalign(0.5);
    heading.append(&name);
    let subtitle = text("From commands to connected workflows.", "opm-dim");
    subtitle.set_xalign(0.5);
    heading.append(&subtitle);
    content.append(&heading);

    let cards = gtk::FlowBox::new();
    cards.set_selection_mode(gtk::SelectionMode::None);
    cards.set_min_children_per_line(1);
    cards.set_max_children_per_line(2);
    cards.set_column_spacing(16);
    cards.set_row_spacing(16);
    cards.set_homogeneous(true);
    let pipeline = card(
        "view-grid-symbolic",
        "Pipeline design",
        "Build reusable steps, inputs and dependencies.",
    );
    let open = action("Open pipeline…", "win.open-pipeline");
    open.add_css_class("suggested-action");
    pipeline.append(&open);
    pipeline.append(&action("New pipeline…", "win.new-pipeline"));
    let import = action("Import pipeline…", "win.import-pipeline");
    import.add_css_class("flat");
    pipeline.append(&import);
    cards.insert(&pipeline, -1);

    let work = card(
        "folder-open-symbolic",
        "Work folders",
        "Run a pipeline with your project files and values.",
    );
    work.append(&action("Open work folder…", "win.open-work"));
    work.append(&action("New work…", "win.new-work"));
    let folder = action("Browse a folder…", "win.open-folder");
    folder.add_css_class("flat");
    work.append(&folder);
    cards.insert(&work, -1);
    content.append(&cards);

    match recent {
        Restore::Work(root) => {
            let recent = gtk::Box::new(gtk::Orientation::Vertical, 8);
            recent.append(&text("Recent work", "opm-section-title"));
            let button = gtk::Button::new();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.set_margin_top(8);
            row.set_margin_bottom(8);
            row.set_margin_start(8);
            row.set_margin_end(8);
            row.append(&gtk::Image::from_icon_name("document-open-recent-symbolic"));
            let name = root
                .file_name()
                .unwrap_or(root.as_os_str())
                .to_string_lossy();
            let label = text(&name, "heading");
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            row.append(&label);
            row.append(&gtk::Image::from_icon_name("go-next-symbolic"));
            button.set_child(Some(&row));
            button.set_tooltip_text(Some(&root.display().to_string()));
            button.connect_clicked(move |_| on_recent(root.clone()));
            recent.append(&button);
            content.append(&recent);
        }
        Restore::Failed(message) => {
            let notice = text(
                &format!("The last work folder could not be reopened.\n{message}"),
                "opm-issue-warning",
            );
            notice.set_max_width_chars(64);
            content.append(&notice);
        }
        Restore::None => {}
    }

    let files = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    files.set_halign(gtk::Align::Center);
    for (label, name) in [("Open file…", "win.open"), ("New text file", "win.new")] {
        let button = action(label, name);
        button.add_css_class("flat");
        files.append(&button);
    }
    content.append(&files);
    let version = text(
        &format!(
            "Native GTK4 · Local execution · v{}",
            env!("CARGO_PKG_VERSION")
        ),
        "opm-dim",
    );
    version.set_xalign(0.5);
    content.append(&version);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&content));
    scroll
}
