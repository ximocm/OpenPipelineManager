//! Capture the real application for repository documentation.
use super::*;
use crate::workspace_qa::{capture, key, settle, widgets};
use std::time::Instant;

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(45);
    while !ready() {
        assert!(Instant::now() < deadline, "capture setup timed out");
        settle();
    }
    settle();
}

fn frame(cmds: &AppCmds, width: i32, height: i32) {
    cmds.window.set_default_size(width, height);
    settle();
    if let Some(pane) = &cmds.explorer_paned {
        pane.set_position(220);
    }
    if let Some(canvas) = &cmds.canvas {
        let canvas_widget = canvas.widget();
        let inspector = cmds.step_panel.as_ref().unwrap().widget();
        for pane in widgets(cmds.window.upcast_ref())
            .into_iter()
            .filter_map(|w| w.downcast::<Paned>().ok())
        {
            if pane.orientation() == Orientation::Horizontal
                && pane
                    .start_child()
                    .is_some_and(|w| widgets(&w).contains(&canvas_widget))
                && pane
                    .end_child()
                    .is_some_and(|w| widgets(&w).contains(&inspector))
            {
                pane.set_position(pane.width() - 320);
            }
        }
        settle();
        key(canvas, gtk4::gdk::Key::Home);
    }
    cmds.window.set_focus_visible(false);
    settle();
}

fn destroy(cmds: &AppCmds) {
    disconnect_window_actions(&cmds.window);
    cmds.window.destroy();
}

fn open_document(cmds: &AppCmds, path: &Path) {
    cmds.open_path(path);
    if cmds.tabs.borrow().tabs()[0].path.is_none() && cmds.tabs.borrow().len() > 1 {
        cmds.select_tab(0);
        cmds.close_tab();
    }
}

#[test]
#[ignore = "requires a display, isolated OPM_QA_DIR/config/libraries, and POSIX awk"]
fn readme_screenshots() {
    for key in [
        "OPM_QA_DIR",
        "XDG_CONFIG_HOME",
        "OPM_PIPELINES_DIR",
        "OPM_TOOLS_DIR",
    ] {
        assert!(PathBuf::from(std::env::var_os(key).expect(key)).starts_with(std::env::temp_dir()));
    }
    let output = PathBuf::from(std::env::var_os("OPM_QA_DIR").unwrap());
    std::fs::create_dir_all(&output).unwrap();
    gtk4::init().unwrap();
    let app = Application::builder()
        .application_id("app.opm.readme.capture")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();

    let welcome = build_window(&app, WindowScope::Standalone, &[]);
    welcome.window.set_default_size(1040, 800);
    assert_eq!(*welcome.center_view.borrow(), CenterView::Welcome);
    welcome.save();
    assert_eq!(*welcome.center_view.borrow(), CenterView::Welcome);
    capture(&welcome.window, &output.join("welcome.png"));
    welcome.window.set_default_size(800, 600);
    capture(&welcome.window, &output.join("welcome-small.png"));
    welcome.new();
    assert_eq!(*welcome.center_view.borrow(), CenterView::Editor);
    assert_eq!(
        welcome.tabs.borrow().len(),
        1,
        "start page must not create extra text tabs"
    );
    destroy(&welcome);

    for json in [
        include_str!("../../core/opm-store/tests/fixtures/fastqc.json"),
        include_str!("../../core/opm-store/tests/fixtures/cutadapt-single.json"),
    ] {
        let tool = opm_store::ToolDefinition::from_json(json).unwrap();
        marketplace::tool_library()
            .save(&tool, SaveMode::CreateNew)
            .unwrap();
    }
    let pipeline: PipelineConfig =
        serde_json::from_str(include_str!("../../scripts/fixtures/readme-pipeline.json")).unwrap();
    let mut doc = PipelineDoc::new("Sequence analysis", pipeline);
    for (id, x, y) in [
        ("prepare_reads", 30.0, 50.0),
        ("filter_reads", 390.0, 50.0),
        ("compute_metrics", 390.0, 270.0),
        ("write_report", 30.0, 270.0),
    ] {
        doc.layout.insert(id.into(), opm_store::Position { x, y });
    }
    project::library()
        .save(&doc.name, &doc, SaveMode::CreateNew)
        .unwrap();
    let design = build_window(&app, WindowScope::Design, &[]);
    design.set_design(doc.clone(), Some(doc.name.clone()));
    let native = std::fs::read_dir(project::library_root())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "pipeline"))
        .unwrap();
    open_document(&design, &native);
    design.show_view(CenterView::Pipeline);
    design.select_step(Some("filter_reads".into()));
    frame(&design, 1440, 900);
    capture(&design.window, &output.join("design.png"));
    design.open_step_dialog(
        StepDraft::for_edit(&doc.pipeline.steps[1]),
        doc.pipeline.steps.clone(),
    );
    let dialog = gtk4::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk4::Window>().ok())
        .find(|w| w.title().as_deref() == Some("Edit step"))
        .unwrap();
    dialog.set_default_size(900, 820);
    capture(&dialog, &output.join("configure.png"));
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
        .unwrap();
    scroll.vadjustment().set_value(380.0);
    capture(&dialog, &output.join("configure-fields.png"));
    dialog.destroy();
    if std::env::var_os("OPM_CAPTURE_MARKETPLACE").is_some() {
        design.marketplace();
        let market = gtk4::Window::list_toplevels()
            .into_iter()
            .filter_map(|w| w.downcast::<gtk4::Window>().ok())
            .find(|w| w.title().as_deref() == Some("Tools & Pipeline Marketplace"))
            .unwrap();
        wait_for(|| {
            widgets(market.upcast_ref())
                .iter()
                .filter_map(|w| w.downcast_ref::<gtk4::Label>())
                .any(|label| label.text().ends_with("definitions available"))
        });
        market.set_default_size(1060, 820);
        capture(&market, &output.join("marketplace.png"));
        market.destroy();
    }
    destroy(&design);

    let root = output.join("Sequence analysis");
    std::fs::create_dir_all(&root).unwrap();
    ProjectStore::new().create_work(&root, &doc).unwrap();
    let work = build_window(&app, WindowScope::Work(root.clone()), &[]);
    work.select_step(Some("write_report".into()));
    work.run_step("write_report".into());
    wait_for(|| !work.exec.status().running);
    work.pump_tick();
    assert!(work
        .store
        .steps()
        .iter()
        .all(|step| work.store.state_of(&step.id).status == opm_store::StepStatus::Ok));
    assert_eq!(
        std::fs::read_to_string(root.join("outputs/report.txt")).unwrap(),
        "reads\tbases\n4\t46\n"
    );
    frame(&work, 1440, 900);
    open_document(&work, &root.join("outputs/report.txt"));
    work.show_view(CenterView::Pipeline);
    for folder in ["Input", "outputs"] {
        wait_for(|| {
            widgets(&work.explorer.as_ref().unwrap().widget())
                .iter()
                .any(|w| {
                    w.downcast_ref::<gtk4::Button>()
                        .and_then(|b| b.child())
                        .and_downcast::<gtk4::Label>()
                        .is_some_and(|l| l.text() == format!("▸ {folder}"))
                })
        });
        let button = widgets(&work.explorer.as_ref().unwrap().widget())
            .into_iter()
            .filter_map(|w| w.downcast::<gtk4::Button>().ok())
            .find(|b| {
                b.child()
                    .and_downcast::<gtk4::Label>()
                    .is_some_and(|l| l.text() == format!("▸ {folder}"))
            })
            .unwrap();
        button.emit_clicked();
        settle();
    }
    capture(&work.window, &output.join("workspace.png"));
    work.open_path(&root.join("outputs/filtered.fasta"));
    work.select_step(Some("filter_reads".into()));
    capture(&work.window, &output.join("fasta.png"));
    std::fs::write(root.join("analysis.md"), "# Sequence analysis\n\nA reproducible local workflow, from input sequences to a compact report.\n\n## Results\n\n- Input: **6 example reads**\n- Retained: **4 reads**\n- Total length: **46 bases**\n- Minimum read length: **8 bases**\n\n## Workflow\n\n1. Prepare the example FASTA dataset.\n2. Filter sequences by their minimum length.\n3. Count reads and total bases.\n4. Write the analysis report.\n\n## Project files\n\n- `Input/reads.fasta` — original sequences\n- `outputs/filtered.fasta` — retained sequences\n- `outputs/metrics.tsv` — computed metrics\n- `outputs/report.txt` — final report\n\n## Reproducibility\n\nPipeline definitions, parameter values and run history stay with this work folder.\nInspect the step logs to see the commands used for this run.\n").unwrap();
    work.open_path(&root.join("analysis.md"));
    work.toggle_mode();
    capture(&work.window, &output.join("editor.png"));
    destroy(&work);

    let recent = build_window(&app, WindowScope::Standalone, &[]);
    recent.window.set_default_size(1040, 900);
    capture(&recent.window, &output.join("welcome-recent.png"));
    let button = widgets(recent.window.upcast_ref())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk4::Button>().ok())
        .find(|b| {
            b.tooltip_text()
                .is_some_and(|t| t == root.display().to_string())
        })
        .unwrap();
    button.emit_clicked();
    settle();
    let reopened = gtk4::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<ApplicationWindow>().ok())
        .find(|w| w != &recent.window)
        .expect("recent action opens the work window");
    assert!(reopened.title().unwrap().contains("Sequence analysis"));
    disconnect_window_actions(&reopened);
    reopened.destroy();
    destroy(&recent);

    let direct = build_window(
        &app,
        WindowScope::Standalone,
        &[root.join("outputs/report.txt")],
    );
    assert_eq!(
        *direct.center_view.borrow(),
        CenterView::Editor,
        "explicit file paths bypass the start page"
    );
    destroy(&direct);

    // Invalid recent work remains recoverable from the start page and clears
    // the stale session slot, using the existing restore validation.
    let missing = output.join("missing-work");
    appstate::save_last_work(&Some((missing.display().to_string(), project::now_secs()))).unwrap();
    let unavailable = build_window(&app, WindowScope::Standalone, &[]);
    assert!(widgets(unavailable.window.upcast_ref())
        .iter()
        .filter_map(|w| w.downcast_ref::<gtk4::Label>())
        .any(|label| label
            .text()
            .contains("last work folder could not be reopened")));
    assert!(appstate::load_last_work().is_none());
    unavailable.window.set_default_size(1040, 800);
    capture(&unavailable.window, &output.join("welcome-unavailable.png"));
    destroy(&unavailable);

    gtk4::Settings::default()
        .unwrap()
        .set_gtk_font_name(Some("Sans 22"));
    let large = build_window(&app, WindowScope::Standalone, &[]);
    large.window.set_default_size(800, 600);
    capture(&large.window, &output.join("welcome-large-text.png"));
    assert!(
        large.window.width() <= 800,
        "large text must not force the start page wider"
    );
    destroy(&large);
}
