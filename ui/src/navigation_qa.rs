//! Running GTK checks for header navigation and explorer View actions.
use super::*;
use crate::workspace_qa::{capture, capture_widget, drag, settle, widgets};
use std::time::Instant;

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(Instant::now() < deadline, "navigator did not become ready");
        settle();
    }
    settle();
}

fn has_file(explorer: &ExplorerPane, name: &str) -> bool {
    widgets(&explorer.widget()).into_iter().any(|widget|
        widget.downcast::<gtk4::Label>().is_ok_and(|label| label.text() == name))
}

fn row_button(explorer: &ExplorerPane, name: &str) -> gtk4::Button {
    widgets(&explorer.widget()).into_iter()
        .filter_map(|w| w.downcast::<gtk4::Button>().ok())
        .find(|button| button.has_css_class("opm-explorer-row")
            && widgets(button.upcast_ref()).into_iter().any(|widget|
                widget.downcast::<gtk4::Label>().is_ok_and(|label| label.text() == name)))
        .unwrap()
}

fn group(cmds: &AppCmds) -> gio::SimpleActionGroup {
    cmds.action_group.borrow().as_ref().unwrap().upgrade().unwrap()
}

fn checked(group: &gio::SimpleActionGroup, action: &str) -> bool {
    group.action_state(action).unwrap().get::<bool>().unwrap()
}

fn pane_positions(cmds: &AppCmds) -> Vec<i32> {
    widgets(cmds.window.upcast_ref()).into_iter()
        .filter_map(|w| w.downcast::<Paned>().ok())
        .map(|pane| pane.position()).collect()
}

#[test]
#[ignore = "requires a display and isolated OPM_QA_DIR/config/libraries"]
fn explorer_navigation_review() {
    for key in ["OPM_QA_DIR", "XDG_CONFIG_HOME", "OPM_PIPELINES_DIR", "OPM_TOOLS_DIR"] {
        assert!(PathBuf::from(std::env::var_os(key).expect(key)).starts_with(std::env::temp_dir()));
    }
    let output = PathBuf::from(std::env::var_os("OPM_QA_DIR").unwrap());
    let root = output.join("Sequence analysis");
    std::fs::create_dir_all(&root).unwrap();
    gtk4::init().unwrap();
    if std::env::var_os("OPM_QA_LARGE_TEXT").is_some() {
        gtk4::Settings::default().unwrap().set_gtk_font_name(Some("Sans 22"));
    }
    let app = Application::builder().application_id("app.opm.navigation.review")
        .flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let doc = PipelineDoc::new("Message analysis",
        serde_json::from_str(include_str!("../../examples/first-run.json")).unwrap());
    ProjectStore::new().create_work(&root, &doc).unwrap();
    let notes = root.join("Analysis notes.md");
    std::fs::write(&notes, "# Analysis notes\n\nFiles and workflows share one document header.\n").unwrap();
    std::fs::write(root.join(".session.md"), "hidden notes").unwrap();
    std::fs::write(root.join("payload.bin"), [0, 1, 2, 3]).unwrap();
    let image = root.join("preview.png");
    gtk4::gdk::MemoryTexture::new(2, 2, gtk4::gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(vec![120u8; 16]), 8).save_to_png(&image).unwrap();
    let work = build_window(&app, WindowScope::Work(root.clone()), &[]);
    let explorer = work.explorer.as_ref().unwrap();
    wait_for(|| has_file(explorer, "Analysis notes.md"));
    let positions = pane_positions(&work);
    let size = (work.window.width(), work.window.height());
    let actions = group(&work);
    assert!(work.tabs.borrow().is_placeholder());
    assert!(!work.tab_bar.widget().is_visible());
    assert!(!work.window.title().unwrap().contains("Sin título"));
    let canvas = work.canvas.as_ref().unwrap();
    let zoom = canvas.transform().zoom;
    drag(canvas, (180.0, 175.0), (40.0, 20.0));
    let saved = work.store.snapshot().visual_layout["create_message"];
    assert!((saved.x - (80.0 + 40.0 / zoom)).abs() < 0.01);
    let reopened = ProjectStore::new();
    reopened.open_work(&root).unwrap();
    assert_eq!(reopened.snapshot().visual_layout["create_message"], saved);

    let console = widgets(work.window.upcast_ref()).into_iter()
        .filter_map(|w| w.downcast::<gtk4::ToggleButton>().ok())
        .find(|button| button.action_name().as_deref() == Some("win.show-logs")).unwrap();
    assert!(console.is_active());
    let logs = work.logs.as_ref().unwrap();
    logs.show_step(Some("create_message"), "before hide\n");
    let log_view = widgets(&logs.widget()).into_iter()
        .find_map(|w| w.downcast::<gtk4::TextView>().ok()).unwrap();
    let log_pane = work.logs_paned.as_ref().unwrap();
    let log_height = log_pane.height() - log_pane.position();
    console.emit_clicked();
    settle();
    assert!(!checked(&actions, "show-logs"));
    assert!(log_pane.end_child().is_none());
    logs.append_line("create_message", "while hidden\n");
    capture(&work.window, &output.join("console-hidden.png"));
    actions.activate_action("show-logs", None);
    settle();
    assert!(console.is_active());
    assert_eq!(log_pane.height() - log_pane.position(), log_height);
    let buffer = log_view.buffer();
    assert!(buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).contains("while hidden"));
    assert!(!has_file(explorer, ".session.md"));
    assert!(!has_file(explorer, "payload.bin"));
    assert!(!checked(&actions, "show-all-files"));
    assert!(!checked(&actions, "show-hidden-files"));
    actions.activate_action("show-hidden-files", None);
    wait_for(|| has_file(explorer, ".session.md"));
    assert!(checked(&actions, "show-hidden-files"));
    assert!(!has_file(explorer, "payload.bin"));
    actions.change_action_state("show-all-files", &true.to_variant());
    wait_for(|| has_file(explorer, "payload.bin"));
    assert!(checked(&actions, "show-all-files"));
    actions.activate_action("show-hidden-files", None);
    settle();
    assert!(!has_file(explorer, ".session.md"));
    assert!(has_file(explorer, "payload.bin"));
    actions.activate_action("refresh-files", None);
    wait_for(|| has_file(explorer, "payload.bin"));
    assert!(checked(&actions, "show-all-files"));
    actions.change_action_state("show-all-files", &false.to_variant());
    work.open_path(&notes);
    settle();
    assert_eq!(work.tabs.borrow().len(), 1);
    assert!(!work.tabs.borrow().is_placeholder());
    let selected = row_button(explorer, "Analysis notes.md");
    assert!(selected.parent().unwrap().state_flags().contains(gtk4::StateFlags::SELECTED));
    actions.activate_action("refresh-files", None);
    wait_for(|| has_file(explorer, "Analysis notes.md"));
    assert!(row_button(explorer, "Analysis notes.md").parent().unwrap()
        .state_flags().contains(gtk4::StateFlags::SELECTED));
    row_button(explorer, "Input").emit_clicked();
    settle();
    assert!(row_button(explorer, "Analysis notes.md").parent().unwrap()
        .state_flags().contains(gtk4::StateFlags::SELECTED));
    let pipeline = work.pipeline_btn.as_ref().unwrap();
    assert!(pipeline.ancestor(gtk4::ScrolledWindow::static_type()).unwrap()
        .has_css_class("opm-workspace-header"));
    assert_eq!(pane_positions(&work), positions);
    assert_eq!((work.window.width(), work.window.height()), size);
    capture(&work.window, &output.join("files.png"));
    pipeline.emit_clicked();
    assert_eq!(*work.center_view.borrow(), CenterView::Pipeline);
    assert_eq!(pane_positions(&work), positions);
    capture(&work.window, &output.join("pipeline.png"));
    work.open_image(image.clone());
    assert_eq!(*work.center_view.borrow(), CenterView::Image(image.clone()));
    assert!(widgets(&work.image_strip.clone().upcast()).into_iter().any(|w|
        w.downcast::<gtk4::Button>().is_ok_and(|b| b.label().as_deref() == Some("preview.png"))));
    work.close_image(&image);
    assert!(work.image_tabs.borrow().is_empty());
    let active = work.tabs.borrow().active_index();
    work.select_tab(active);
    actions.activate_action("show-hidden-files", None);
    wait_for(|| has_file(explorer, ".session.md"));
    // Open the actual menu bar item; capture GTK's native View popover.
    let menu = widgets(work.window.upcast_ref()).into_iter()
        .find(|w| w.type_().name() == "GtkPopoverMenuBarItem" && widgets(w).into_iter().any(|w|
            w.downcast::<gtk4::Label>().is_ok_and(|label| label.text() == "View"))).unwrap();
    assert!(menu.activate());
    settle();
    let popup = widgets(work.window.upcast_ref()).into_iter()
        .filter_map(|w| w.downcast::<gtk4::PopoverMenu>().ok())
        .find(|w| w.is_mapped()).unwrap();
    capture_widget(&popup, &output.join("view-menu.png"));
    popup.popdown();
    actions.activate_action("show-hidden-files", None);
    row_button(explorer, "Analysis notes.md").grab_focus();
    work.window.set_focus_visible(true);
    capture(&work.window, &output.join("focus.png"));
    work.window.set_default_size(800, 600);
    capture(&work.window, &output.join("small.png"));
    let scroll = pipeline.ancestor(gtk4::ScrolledWindow::static_type()).unwrap()
        .downcast::<gtk4::ScrolledWindow>().unwrap();
    let active_button = widgets(scroll.upcast_ref()).into_iter()
        .find(|w| w.has_css_class("opm-active-tab")).unwrap();
    let bounds = active_button.compute_bounds(scroll.upcast_ref::<Widget>()).unwrap();
    assert!(bounds.x() >= -1.0, "active tab starts outside the header: {bounds:?}");
    if bounds.width() <= scroll.width() as f32 {
        assert!(bounds.x() + bounds.width() <= scroll.width() as f32 + 1.0,
            "active tab ends outside the header: {bounds:?}");
    }
    assert!(bounds.y() >= -1.0 && bounds.y() + bounds.height() <= scroll.height() as f32 + 1.0,
        "active tab is vertically clipped: {bounds:?}");
    let folder = build_window(&app, WindowScope::Folder(root), &[]);
    assert!(!checked(&group(&folder), "show-all-files"));
    assert!(!checked(&group(&folder), "show-hidden-files"));
    let standalone = build_window(&app, WindowScope::Standalone, &[]);
    for action in ["show-all-files", "show-hidden-files", "refresh-files"] {
        assert!(!group(&standalone).is_action_enabled(action));
    }
    assert!(!group(&standalone).is_action_enabled("show-logs"));
    assert!(!group(&folder).is_action_enabled("show-logs"));
    work.close_tab();
    assert!(work.tabs.borrow().is_placeholder());
    assert_eq!(*work.center_view.borrow(), CenterView::Pipeline);
    work.new();
    assert!(!work.tabs.borrow().is_placeholder());
    assert_eq!(*work.center_view.borrow(), CenterView::Editor);
    assert!(work.tab_bar.widget().is_visible());
    for cmds in [&standalone, &folder, &work] {
        disconnect_window_actions(&cmds.window);
        cmds.window.destroy();
    }
}
