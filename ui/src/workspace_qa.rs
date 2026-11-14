//! Offline running-window review harness; all project/library state is isolated.
use super::*;
use gtk4::gsk::prelude::*;
use std::time::Instant;

pub(super) fn settle() {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(350) {
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn capture(window: &impl IsA<gtk4::Window>, path: &Path) {
    capture_widget(window.as_ref(), path);
}

pub(super) fn capture_widget(widget: &impl IsA<Widget>, path: &Path) {
    settle();
    let window = widget.as_ref();
    assert!(window.is_mapped());
    let snapshot = gtk4::Snapshot::new();
    gtk4::WidgetPaintable::new(Some(window)).snapshot(
        &snapshot,
        window.width() as f64,
        window.height() as f64,
    );
    let renderer = gtk4::gsk::Renderer::for_surface(&window.native().unwrap().surface().unwrap()).unwrap();
    renderer
        .render_texture(&snapshot.to_node().expect("window frame"), None)
        .save_to_png(path)
        .unwrap();
    renderer.unrealize();
}

pub(super) fn widgets(root: &Widget) -> Vec<Widget> {
    let mut result = vec![root.clone()];
    let mut next = root.first_child();
    while let Some(child) = next {
        next = child.next_sibling();
        result.extend(widgets(&child));
    }
    result
}

pub(super) fn key(canvas: &PipelineCanvas, key: gtk4::gdk::Key) {
    use glib::translate::IntoGlib;
    let controllers = canvas.widget().observe_controllers();
    let keyboard = (0..controllers.n_items())
        .filter_map(|i| controllers.item(i))
        .find_map(|item| item.downcast::<gtk4::EventControllerKey>().ok())
        .unwrap();
    assert!(keyboard.emit_by_name::<bool>(
        "key-pressed",
        &[&key.into_glib(), &0u32, &gtk4::gdk::ModifierType::empty()]
    ));
}

fn drag(canvas: &PipelineCanvas, world: (f64, f64), delta: (f64, f64)) {
    let controllers = canvas.widget().observe_controllers();
    let gesture = (0..controllers.n_items())
        .filter_map(|i| controllers.item(i))
        .find_map(|item| item.downcast::<gtk4::GestureDrag>().ok())
        .unwrap();
    let screen = canvas.transform().to_screen(world);
    gesture.emit_by_name::<()>("drag-begin", &[&screen.0, &screen.1]);
    gesture.emit_by_name::<()>("drag-update", &[&delta.0, &delta.1]);
    gesture.emit_by_name::<()>("drag-end", &[&delta.0, &delta.1]);
}

#[test]
#[ignore = "requires a display and isolated OPM_QA_DIR, XDG_CONFIG_HOME, OPM_PIPELINES_DIR, OPM_TOOLS_DIR"]
fn workspace_visual_review() {
    let output = PathBuf::from(std::env::var_os("OPM_QA_DIR").expect("OPM_QA_DIR"));
    for key in [
        "OPM_QA_DIR",
        "XDG_CONFIG_HOME",
        "OPM_PIPELINES_DIR",
        "OPM_TOOLS_DIR",
    ] {
        assert!(PathBuf::from(std::env::var_os(key).expect(key)).starts_with(std::env::temp_dir()));
    }
    std::fs::create_dir_all(&output).unwrap();
    gtk4::init().unwrap();
    if std::env::var_os("OPM_QA_LARGE_TEXT").is_some() {
        gtk4::Settings::default()
            .unwrap()
            .set_gtk_font_name(Some("Sans 22"));
    }
    eprintln!(
        "GTK theme: {:?}; dark palette: {}",
        gtk4::Settings::default().unwrap().gtk_theme_name(),
        theme::is_dark()
    );
    let app = Application::builder()
        .application_id("app.opm.workspace.review")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let config: PipelineConfig =
        serde_json::from_str(include_str!("../../examples/first-run.json")).unwrap();
    let doc = PipelineDoc::new("Message analysis", config);
    let design = build_window(&app, WindowScope::Design, &[]);
    design.set_design(doc.clone(), None);
    design.select_step(Some("write_report".into()));
    capture(&design.window, &output.join("design.png"));
    let mut horizontal = doc.clone();
    horizontal.layout.insert(
        "create_message".into(),
        opm_store::Position { x: 30.0, y: 100.0 },
    );
    horizontal.layout.insert(
        "write_report".into(),
        opm_store::Position { x: 390.0, y: 100.0 },
    );
    design.set_design(horizontal, None);
    design.select_step(Some("write_report".into()));
    settle();
    key(design.canvas.as_ref().unwrap(), gtk4::gdk::Key::Home);
    capture(&design.window, &output.join("design-horizontal.png"));
    design.set_design(doc.clone(), None);
    design.select_step(Some("write_report".into()));
    design.window.set_default_size(800, 600);
    settle();
    key(design.canvas.as_ref().unwrap(), gtk4::gdk::Key::Home);
    capture(&design.window, &output.join("design-small.png"));
    eprintln!(
        "Small design allocation: {}×{}; canvas {}×{}",
        design.window.width(),
        design.window.height(),
        design.canvas.as_ref().unwrap().widget().width(),
        design.canvas.as_ref().unwrap().widget().height()
    );
    pipeline::step_dialog::open_step_dialog(
        &design.window,
        StepDraft::for_edit(&doc.pipeline.steps[1]),
        doc.pipeline.steps.clone(),
        None,
        Rc::new(|_, _| {}),
    );
    let dialog = gtk4::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk4::Window>().ok())
        .find(|w| w.title().as_deref() == Some("Edit step"))
        .unwrap();
    dialog.set_default_size(760, 560);
    capture(&dialog, &output.join("step-dialog.png"));
    let scroll = widgets(dialog.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk4::ScrolledWindow>().ok())
        .find(|w| {
            w.child().is_some_and(|child| {
                widgets(&child)
                    .iter()
                    .any(|w| w.has_css_class("opm-dialog-form"))
            })
        })
        .unwrap_or_else(|| {
            widgets(dialog.upcast_ref())
                .into_iter()
                .find_map(|w| w.downcast::<gtk4::ScrolledWindow>().ok())
                .unwrap()
        });
    scroll.vadjustment().set_value(350.0);
    capture(&dialog, &output.join("step-dialog-fields.png"));
    // Submission must preserve the command and declaration model after
    // the layout changes; detailed default/type contracts live in unit tests.
    dialog.destroy();
    let canvas = design.canvas.as_ref().unwrap();
    let zoom = canvas.transform().zoom;
    drag(canvas, (120.0, 160.0), (20.0, 10.0));
    let moved = design.design().unwrap().borrow().doc().layout["create_message"];
    assert!((moved.x - (80.0 + 20.0 / zoom)).abs() < 0.01);
    assert!((moved.y - (120.0 + 10.0 / zoom)).abs() < 0.01);
    let before = canvas.transform();
    drag(canvas, (0.0, 0.0), (15.0, 20.0));
    assert_eq!(canvas.transform().pan_x, before.pan_x + 15.0);
    disconnect_window_actions(&design.window);
    design.window.destroy();
    let work_root = output.join("work");
    std::fs::create_dir_all(&work_root).unwrap();
    ProjectStore::new().create_work(&work_root, &doc).unwrap();
    let work = build_window(&app, WindowScope::Work(work_root), &[]);
    work.select_step(Some("write_report".into()));
    work.logs.as_ref().unwrap().show_step(Some("write_report"),
        "[review] Waiting for execution\nOutput from the selected step appears here.\nLong lines remain selectable and can be wrapped for reading.");
    work.canvas.as_ref().unwrap().widget().grab_focus();
    work.window.set_focus_visible(true);
    capture(&work.window, &output.join("work.png"));
    work.window.set_default_size(800, 600);
    settle();
    key(work.canvas.as_ref().unwrap(), gtk4::gdk::Key::Home);
    capture(&work.window, &output.join("work-small.png"));
    // Real controller dispatch must select and toggle through the same
    // coordinator/store path used by mouse interaction.
    key(work.canvas.as_ref().unwrap(), gtk4::gdk::Key::Left);
    assert_eq!(
        work.selected_step.borrow().as_deref(),
        Some("create_message")
    );
    let selected = work.store.state_of("create_message").selected;
    key(work.canvas.as_ref().unwrap(), gtk4::gdk::Key::space);
    assert_ne!(work.store.state_of("create_message").selected, selected);
    let before = work.canvas.as_ref().unwrap().transform().zoom;
    key(work.canvas.as_ref().unwrap(), gtk4::gdk::Key::plus);
    assert!(work.canvas.as_ref().unwrap().transform().zoom > before);
    let logs = work.logs.as_ref().unwrap();
    let wrap = widgets(&logs.widget())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk4::CheckButton>().ok())
        .find(|w| w.label().as_deref() == Some("Wrap"))
        .unwrap();
    wrap.set_active(true);
    let view = widgets(&logs.widget())
        .into_iter()
        .find_map(|w| w.downcast::<gtk4::TextView>().ok())
        .unwrap();
    assert_eq!(view.wrap_mode(), gtk4::WrapMode::WordChar);
    // Execute the repository's self-contained example in the temporary work
    // folder, through the same coordinator used by the inspector Run button.
    work.select_step(Some("write_report".into()));
    work.run_step("write_report".into());
    let deadline = Instant::now() + Duration::from_secs(10);
    while work.exec.status().running {
        assert!(Instant::now() < deadline, "example execution timed out");
        settle();
    }
    work.pump_tick();
    assert_eq!(
        work.store.state_of("write_report").status,
        opm_store::StepStatus::Ok
    );
    assert_eq!(
        std::fs::read_to_string(
            work.store
                .project_root()
                .unwrap()
                .join("outputs/report.txt")
        )
        .unwrap(),
        "Hello from Open Pipeline Manager\nmode=demo\n"
    );
    work.window.set_default_size(1280, 800);
    settle();
    key(work.canvas.as_ref().unwrap(), gtk4::gdk::Key::Home);
    capture(&work.window, &output.join("work-completed.png"));
    let mut params = work.store.params_for("write_report");
    params.insert("mode".into(), Value::Str("invalid-choice".into()));
    work.store.set_params("write_report", params).unwrap();
    work.refresh_domain();
    let inspector = work
        .step_panel
        .as_ref()
        .unwrap()
        .widget()
        .downcast::<gtk4::ScrolledWindow>()
        .unwrap();
    inspector
        .vadjustment()
        .set_value(inspector.vadjustment().upper());
    capture(&work.window, &output.join("work-validation.png"));
    if std::env::var_os("GTK_THEME").is_none() {
        let run_file = work
            .store
            .project_root()
            .unwrap()
            .join(".pipeline-manager/run.json");
        let persisted = std::fs::read(&run_file).unwrap();
        let transform = work.canvas.as_ref().unwrap().transform();
        let selection = work.selected_step.borrow().clone();
        let text = work
            .tabs
            .borrow()
            .active()
            .editor
            .document()
            .content()
            .to_owned();
        let settings = gtk4::Settings::default().unwrap();
        settings.set_gtk_theme_name(Some("Adwaita"));
        settings.set_gtk_application_prefer_dark_theme(true);
        assert!(theme::is_dark());
        capture(&work.window, &output.join("work-theme-switch.png"));
        assert_eq!(std::fs::read(&run_file).unwrap(), persisted);
        assert_eq!(work.canvas.as_ref().unwrap().transform(), transform);
        assert_eq!(*work.selected_step.borrow(), selection);
        assert_eq!(
            work.tabs
                .borrow()
                .active()
                .editor
                .document()
                .content()
                .to_owned(),
            text
        );
    }
    disconnect_window_actions(&work.window);
    work.window.destroy();
}
