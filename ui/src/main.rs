//! opm — aplicación de escritorio GTK4 de Open Pipeline Manager.
//!
//! Integra el workspace `core/` con la shell de la aplicación (menús,
//! editor, pestañas y explorador):
//! - menús §36 (File, Edit, View, Help) y atajos §39 (clon en `menus`);
//! - editor con Markdown Mode + modo Visual §8.2 y despacho de highlighting
//!   por extensión (§5: Markdown → `opm_parser`, YAML/JSON/shell/FASTA →
//!   `crate::syntax`, resto sin resaltado);
//! - pestañas §37 con indicador `●` de modificado y guardado explícito
//!   (§3.6, §12.7);
//! - explorador de archivos con árbol perezoso (§14.3–§14.7) y operaciones
//!   del proyecto delegadas en `opm-fs` (§14.8);
//! - barra de búsqueda/reemplazo del archivo actual (§40.1);
//! - diálogos GTK4 y confirmaciones explícitas (§3.7, §12.7).
//!
//! Wiring (§5): `AppCmds` en `Rc` como coordinador de ventana, `CmdSlot`
//! (`Weak<AppCmds>`) para la contrapuesta tardía de los widgets, `EditSink`
//! como puente TextView → `TabManager`, UN `EditorPane` compartido por
//! ventana re-`load()`eado en cada cambio de tab, y guardas de cierre con
//! referencias débiles (§11 A12).
//!
//! Los dominios de proyecto, pipeline y ejecución viven en `project/`,
//! `pipeline/` y `runs/`, cableados desde este módulo.

mod appstate;
mod editor;
mod explorer;
mod find;
mod imageview;
mod list_scroll;
mod menus;
mod paths;
mod pipeline;
mod project;
mod runs;
mod syntax;
mod tabs;
mod theme;

use editor::{EditSink, EditorPane, EditMode};
use explorer::{
    load_state, save_state, ExplorerAction, ExplorerDrop, ExplorerPane, ExplorerState,
};
use find::{FindAction, FindBar};
use menus::{build_menubar, dialogs, install_actions, ACCELERATORS, Commands};
use pipeline::{CanvasCallbacks, CanvasData, PipelineCanvas, StepCallbacks, StepDraft, StepPanel};
use runs::{drain, format_blocked, Counters, LogsPanel, RunModel};
use tabs::{OpenTarget, TabBar, TabManager, WindowScope};

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    gio, Application, ApplicationWindow, Box as GtkBox, CssProvider, Orientation, Paned,
    PopoverMenuBar, Stack, Widget,
};
use opm_document::{Position, Selection};
use opm_exec::{ExecError, ExecutionManager};
use opm_fs::FsError;
use opm_pipeline::{PipelineStep, Value};
use opm_store::ProjectStore;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

/// Retardo del refresco de highlighting tras teclear (§47.1: las tareas
/// secundarias nunca bloquean una pulsación de teclado).
const REFRESH_DEBOUNCE_MS: u64 = 120;

/// Contrapuesta tardía: los widgets construidos ANTES de `AppCmds` (pane,
/// tab bar, find bar, explorador) resuelven el coordinador en el momento del
/// callback sin retención fuerte, sin ciclos de `Rc` (§11 A12).
type CmdSlot = RefCell<Option<Weak<AppCmds>>>;

/// Puente `EditSink` (TextView) → modelo (`TabManager` → core `Editor`).
struct Sink {
    tabs: Rc<RefCell<TabManager>>,
}

impl EditSink for Sink {
    fn insert_at(&self, at: Position, text: &str) {
        self.tabs.borrow_mut().insert_active(at, text);
    }

    fn delete_range(&self, from: Position, to: Position) {
        self.tabs.borrow_mut().delete_active(from, to);
    }

    fn set_selection(&self, selection: Selection) {
        self.tabs.borrow_mut().set_selection_active(selection);
    }
}

/// Página visible del stack central (§11 A11): la pestaña «Pipeline» es
/// PERMANENTE en ventanas de proyecto, el editor comparte una sola página
/// con el tab activo y cada imagen vive en su propia página (nunca en
/// `TabManager`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum CenterView {
    Pipeline,
    Editor,
    Image(PathBuf),
}

/// Estado de la ventana: widgets + modelo compartido.
struct AppCmds {
    app: Application,
    window: ApplicationWindow,
    /// Ámbito de esta ventana (ISSUE-021): standalone o con explorador.
    scope: WindowScope,
    tabs: Rc<RefCell<TabManager>>,
    /// Único panel de edición por ventana (§5): re-`load()` en cada cambio.
    editor_pane: Rc<EditorPane>,
    tab_bar: Rc<TabBar>,
    /// Barra de búsqueda/reemplazo del archivo actual (§40.1).
    find_bar: Rc<FindBar>,
    /// Explorador (solo ventanas de proyecto), su caja-panel enmarcada y el
    /// `Paned` cuyo hijo inicial es esa caja (`toggle_explorer`).
    explorer: Option<Rc<ExplorerPane>>,
    explorer_panel: Option<GtkBox>,
    explorer_paned: Option<Paned>,
    explorer_visible: Cell<bool>,
    explorer_state: RefCell<ExplorerState>,
    /// Store del proyecto de ESTA ventana (compartido con el motor, A6).
    store: Arc<ProjectStore>,
    /// Motor de ejecución de esta ventana (§4 opm-exec, A6: `Arc` store).
    exec: Rc<ExecutionManager>,
    /// Reducer puro de los eventos del motor (§11 A12: tick de 250 ms).
    run_model: RefCell<RunModel>,
    /// Ejecución en curso vista por la UI (la confirma `exec.status()`).
    running: Cell<bool>,
    /// Paso seleccionado (lienzo/panel/resumen de validación).
    selected_step: RefCell<Option<String>>,
    /// Stack central: página «Pipeline» permanente + editor + imágenes (A11).
    stack: Stack,
    /// Página visible actual del stack central.
    center_view: RefCell<CenterView>,
    /// Botón permanente «Pipeline» (solo ventanas de proyecto).
    pipeline_btn: Option<gtk4::Button>,
    /// Pestañas de imagen abiertas, en orden de la franja (A11).
    image_tabs: RefCell<Vec<PathBuf>>,
    /// Contenedor de las pestañas de imagen en la franja.
    image_strip: GtkBox,
    /// Vistas del dominio (solo ventanas de proyecto con proyecto abierto).
    canvas: Option<Rc<PipelineCanvas>>,
    step_panel: Option<Rc<StepPanel>>,
    counters: Option<Rc<Counters>>,
    logs: Option<Rc<LogsPanel>>,
    /// Botones del toolbar §6 (su sensibilidad la fija `apply_running_ui`).
    run_btn: Option<gtk4::Button>,
    stop_btn: Option<gtk4::Button>,
    /// Grupo «win» insertado por `install_actions`, en referencia DÉBIL: sus
    /// acciones capturan `Rc<dyn Commands>` de esta ventana, así que una
    /// referencia fuerte aquí cerraría el ciclo `AppCmds → grupo → acciones
    /// → AppCmds` (§11 A12). La referencia fuerte la retiene la propia
    /// ventana (`insert_action_group`) hasta el cierre, donde se desconecta.
    action_group: RefCell<Option<glib::WeakRef<gio::SimpleActionGroup>>>,
    refresh_scheduled: Rc<Cell<bool>>,
    slot: Rc<CmdSlot>,
}

impl AppCmds {
    fn slot(&self) -> Option<Rc<AppCmds>> {
        self.slot.borrow().as_ref().and_then(Weak::upgrade)
    }

    /// Sincroniza vistas y título con el tab activo (cambio de tab, open,
    /// undo/redo, save) y repinta el highlighting.
    fn reload_active(&self) {
        {
            let tabs = self.tabs.borrow();
            let tab = tabs.active();
            self.editor_pane.load(&tab.editor, tab.mode);
        }
        self.refresh_now();
    }

    /// Refresco completo del tab activo: highlighting, barra de pestañas,
    /// título, marcado de búsqueda y sensibilidad de Markdown/Visual.
    fn refresh_now(&self) {
        let text = self
            .tabs
            .borrow()
            .active()
            .editor
            .document()
            .content()
            .to_owned();
        self.editor_pane.refresh(&text);
        let editor_focused = matches!(*self.center_view.borrow(), CenterView::Editor);
        self.tab_bar.sync(&self.tabs.borrow(), editor_focused);
        self.sync_strip();
        self.update_title();
        // §40.1: con la barra abierta, el marcado sigue al contenido; el
        // ancla pasa a ser el cursor (edición, undo/redo o cambio de tab).
        let cursor = self.tabs.borrow().active().editor.selection().head;
        self.find_refresh(Some(cursor), false);
        // §5: el modo Visual solo existe para Markdown.
        let markdown = active_is_markdown(&self.tabs);
        self.set_action_enabled("toggle-mode", markdown);
    }

    /// Refresco diferido tras cada edición real (§47.1).
    fn schedule_refresh(self: &Rc<Self>) {
        if self.refresh_scheduled.get() {
            return;
        }
        self.refresh_scheduled.set(true);
        let me = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(REFRESH_DEBOUNCE_MS), move || {
            me.refresh_scheduled.set(false);
            me.refresh_now();
        });
    }

    /// Habilita/deshabilita una acción `win.*` del grupo instalado.
    fn set_action_enabled(&self, name: &str, enabled: bool) {
        let group = self
            .action_group
            .borrow()
            .as_ref()
            .and_then(|weak| weak.upgrade());
        let Some(group) = group else { return };
        if let Some(action) = group
            .lookup_action(name)
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
        {
            action.set_enabled(enabled);
        }
    }

    // --- Búsqueda del archivo actual (§40.1) -------------------------------

    /// Pinta el marcado de búsqueda y la barra desde [`find::FindState`]
    /// (sin recalcular). `focus` lleva la coincidencia activa a la vista
    /// (scroll + cursor, §8.6).
    fn find_repaint(&self, focus: bool) {
        if !self.find_bar.is_open() {
            return;
        }
        let (matches, active_index, active, counter, error) = {
            let state = self.find_bar.state().borrow();
            (
                state.matches().to_vec(),
                state.active_index(),
                state.active_match(),
                state.counter_text(),
                state.error().map(str::to_owned),
            )
        };
        self.editor_pane.set_find_marks(&matches, active_index);
        self.find_bar.set_counter(&counter);
        self.find_bar.set_error(error.as_deref());
        if focus {
            if let Some(range) = active {
                self.editor_pane.focus_find_match(range);
                // Tras asentar la selección, el cursor a la vista (§40.1);
                // en idle, como el resto de scrolls del editor (§47.1).
                let pane = self.editor_pane.clone();
                glib::idle_add_local_once(move || pane.scroll_to_cursor());
            }
        }
    }

    /// Recalcula la búsqueda sobre el tab activo (§40.1) y pinta.
    ///
    /// `anchor` fija el ancla (cursor al abrir o editar, o el final de un
    /// reemplazo); con `None` el ancla se conserva, de modo que teclear la
    /// consulta no la mueve.
    fn find_refresh(&self, anchor: Option<Position>, focus: bool) {
        if !self.find_bar.is_open() {
            return;
        }
        {
            let mut state = self.find_bar.state().borrow_mut();
            if let Some(position) = anchor {
                state.set_anchor(position);
            }
            let tabs = self.tabs.borrow();
            state.refresh(&tabs.active().editor);
        }
        self.find_repaint(focus);
    }

    /// Abre la barra de búsqueda (§39: `Ctrl+F`) o de reemplazo (`Ctrl+H`)
    /// sobre el tab activo y busca desde el cursor.
    fn find_open(&self, show_replace: bool) {
        self.find_bar.open(show_replace);
        let cursor = self.tabs.borrow().active().editor.selection().head;
        self.find_refresh(Some(cursor), true);
        self.find_bar.focus_search();
    }

    /// Acciones de la barra de búsqueda (§40.1).
    fn find_action(&self, action: FindAction) {
        match action {
            FindAction::Search => {
                // El ancla no se mueve al teclear la consulta.
                self.find_refresh(None, true);
            }
            FindAction::Next | FindAction::Previous => {
                {
                    let mut state = self.find_bar.state().borrow_mut();
                    if action == FindAction::Next {
                        state.next();
                    } else {
                        state.previous();
                    }
                }
                // La activa ya avanzó (con wrap §40.1): solo re-marcar y
                // llevarla a la vista.
                self.find_repaint(true);
            }
            FindAction::Replace => {
                let edit = self.find_bar.state().borrow().replace_edit();
                let Some((range, text)) = edit else { return };
                self.tabs.borrow_mut().replace_active(range, &text);
                // «Reemplazar» avanza a la siguiente: `reload_active` fija el
                // ancla en el cursor, que quedó al final del texto insertado
                // (§8.6), y recalcula.
                self.reload_active();
                self.find_repaint(true);
            }
            FindAction::ReplaceAll => {
                let edits = self.find_bar.state().borrow().replace_all_edits();
                if edits.is_empty() {
                    return;
                }
                // §8.5: una sola transacción; un solo undo la revierte entera.
                self.tabs.borrow_mut().replace_all_active(&edits);
                self.reload_active();
                self.find_repaint(true);
            }
            FindAction::Close => {
                self.find_bar.close();
                self.find_bar.state().borrow_mut().clear_results();
                self.editor_pane.clear_find_marks();
            }
        }
    }

    // --- Título, ventanas y pestañas --------------------------------------

    /// Título observable de la ventana (§3.7): tab activo + ámbito (ISSUE-021)
    /// + modo Visual cuando el tab activo lo usa.
    fn update_title(&self) {
        let (title, mode) = {
            let tabs = self.tabs.borrow();
            let index = tabs.active_index();
            (tabs.title(index), tabs.active().mode)
        };
        let text = match &self.scope {
            WindowScope::Standalone => format!("{title} — OPM"),
            WindowScope::Project(root) => {
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.display().to_string());
                format!("{title} — OPM · {name}")
            }
        };
        let text = match mode {
            EditMode::Markdown => text,
            EditMode::Visual => format!("{text} · {}", mode.label()),
        };
        self.window.set_title(Some(&text));
    }

    /// Abre una ventana nueva con su propio ámbito y sus pestañas (ISSUE-021).
    fn spawn_window(&self, scope: WindowScope, paths: &[PathBuf]) {
        build_window(&self.app, scope, paths);
    }

    /// Abre `path` según el ámbito de la ventana (ISSUE-021, `OpenTarget`):
    /// dentro del ámbito, pestaña aquí; fuera del ámbito, ventana standalone
    /// nueva — nunca se rechaza un archivo.
    fn open_path(&self, path: &Path) {
        // §11 A11: las imágenes abren en su página dedicada, jamás a través
        // de la carga UTF-8 del editor.
        if imageview::is_image_path(path) {
            self.open_image(path.to_path_buf());
            return;
        }
        if self.scope.target_for(path) == OpenTarget::NewStandaloneWindow {
            self.spawn_window(WindowScope::Standalone, &[path.to_path_buf()]);
            return;
        }
        let opened = self.tabs.borrow_mut().open(path);
        match opened {
            Ok(index) => self.select_tab(index),
            Err(err) => show_error(&self.window, &err.to_string()),
        }
    }

    /// Abre `path` en una pestaña o activa la que ya lo tiene abierto
    /// (§5: «abrir (o activar) la pestaña del fichero»). La comparación de
    /// rutas admite grafías distintas del mismo fichero.
    fn activate_or_open(&self, path: &Path) {
        if imageview::is_image_path(path) {
            self.open_image(path.to_path_buf());
            return;
        }
        let objetivo = ruta_comparable(path);
        let abierta = {
            let tabs = self.tabs.borrow();
            tabs.tabs().iter().position(|tab| {
                tab.path
                    .as_deref()
                    .is_some_and(|p| ruta_comparable(p) == objetivo)
            })
        };
        match abierta {
            Some(index) => self.select_tab(index),
            None => self.open_path(path),
        }
    }

    fn select_tab(&self, index: usize) {
        self.tabs.borrow_mut().set_active(index);
        // §11 A11: activar una pestaña de texto lleva al editor al frente.
        self.show_view(CenterView::Editor);
        self.reload_active();
    }

    // --- Explorador (§14.3/§14.8) -----------------------------------------

    /// Persiste visibilidad y ancho del explorador (§14.3: estado
    /// restaurable) vía `crate::appstate` (§11 A13).
    fn persist_explorer_state(&self) {
        let (Some(paned), Some(_)) = (&self.explorer_paned, &self.explorer) else {
            return;
        };
        if self.explorer_visible.get() {
            let width = paned.position();
            if width > 0 {
                self.explorer_state.borrow_mut().width = ExplorerState::clamp_width(width);
            }
        }
        let state = *self.explorer_state.borrow();
        if let Err(err) = save_state(&state) {
            eprintln!("opm: no se pudo guardar el estado del explorador: {err}");
        }
    }

    fn refresh_explorer(&self) {
        if let Some(explorer) = &self.explorer {
            explorer.refresh();
        }
    }

    /// Operaciones §14.8 del explorador sobre el nodo `target`
    /// (`None` = raíz del proyecto), delegadas en las ops de `opm-fs`.
    fn explorer_op(&self, action: ExplorerAction, target: Option<PathBuf>) {
        let Some(me) = self.slot() else { return };
        let Some(root) = self.scope.project_root() else {
            return;
        };
        match action {
            ExplorerAction::CopyRelativePath => {
                let Some(target) = target else { return };
                // §14.8/§17: ruta relativa al proyecto (separadores `/`).
                let rel = rel_to(&root, &target)
                    .filter(|rel| !rel.is_empty())
                    .unwrap_or_else(|| ".".to_owned());
                if let Some(display) = gtk4::gdk::Display::default() {
                    display.clipboard().set_text(&rel);
                }
            }
            ExplorerAction::Delete => {
                // §14.9: borrado permanente con aviso de recursividad y
                // confirmación explícita (§3.7). A11/DECISIONS: borrar una
                // ruta con pestañas de texto SUCIAS dentro pide confirmación
                // explícita antes de descartar sus cambios (paridad SPA
                // `handleDeletePath`).
                let Some(target) = target else { return };
                let window = self.window.clone();
                glib::spawn_future_local(async move {
                    let name = file_name_of(&target);
                    let dirty = dirty_tabs_under(&me.tabs.borrow(), &target);
                    if !dirty.is_empty() {
                        let files = dirty.join("\n");
                        let detail = format!(
                            "Deleting {} will discard unsaved changes in:\n{files}\n\n\
                             Discard these changes and delete?",
                            target.display()
                        );
                        if !dialogs::confirm_message(&window, "Delete?", &detail, "Delete").await
                        {
                            return;
                        }
                    } else if !dialogs::confirm_trash(&window, &name).await {
                        return;
                    }
                    match rel_to(&root, &target) {
                        Some(rel) => match opm_fs::delete_project_path(&root, &rel) {
                            Ok(()) => {
                                // A11: las pestañas de texto afectadas se
                                // cierran (§3.7 las conserva desasociadas si
                                // estaban sucias) y las de imagen se retiran.
                                me.tabs.borrow_mut().path_deleted(&target);
                                me.close_images_under(&target);
                                me.reload_active();
                                me.refresh_explorer();
                            }
                            Err(err) => show_error(&window, &err.to_string()),
                        },
                        None => show_error(&window, "la ruta sale del proyecto"),
                    }
                });
            }
            ExplorerAction::NewFile | ExplorerAction::NewFolder => {
                // §14.8: crear dentro del nodo seleccionado (archivo → su
                // carpeta; vacío → raíz).
                let parent = parent_dir_of(&target, &root);
                let (title, initial) = match action {
                    ExplorerAction::NewFile => ("New File", "new.txt"),
                    _ => ("New Folder", "new-folder"),
                };
                let window = self.window.clone();
                let prompt_window = self.window.clone();
                dialogs::prompt_text(
                    &window,
                    title,
                    initial,
                    Rc::new(move |answer| {
                        let Some(name) = answer.filter(|n| !n.trim().is_empty()) else {
                            return;
                        };
                        let Some(requested) = requested_under(&root, &parent, &name) else {
                            return;
                        };
                        let directory = action == ExplorerAction::NewFolder;
                        match opm_fs::create_project_path(&root, &requested, "", directory) {
                            Ok(_) => me.refresh_explorer(),
                            Err(err) => show_error(&prompt_window, &err.to_string()),
                        }
                    }),
                );
            }
            ExplorerAction::Rename => {
                let Some(target) = target else { return };
                let initial = file_name_of(&target);
                let window = self.window.clone();
                dialogs::prompt_text(
                    &window,
                    "Rename",
                    &initial,
                    Rc::new(move |answer| {
                        let Some(name) = answer.filter(|n| !n.trim().is_empty()) else {
                            return;
                        };
                        let Some(rel) = rel_to(&root, &target) else {
                            return;
                        };
                        match opm_fs::rename_project_path(&root, &rel, &name) {
                            Ok(new_path) => me.retarget_after_fs_move(&target, &new_path),
                            Err(err) => show_error(&me.window, &err.to_string()),
                        }
                    }),
                );
            }
            ExplorerAction::Move => {
                let Some(target) = target else { return };
                // El prompt propone la carpeta del propio nodo (editable
                // hacia el destino, relativo a la raíz).
                let initial = target
                    .parent()
                    .and_then(|parent| rel_to(&root, parent))
                    .filter(|rel| !rel.is_empty())
                    .unwrap_or_else(|| ".".to_owned());
                let window = self.window.clone();
                dialogs::prompt_text(
                    &window,
                    "Move",
                    &initial,
                    Rc::new(move |answer| {
                        let Some(dest) = answer.filter(|n| !n.trim().is_empty()) else {
                            return;
                        };
                        let Some(rel) = rel_to(&root, &target) else {
                            return;
                        };
                        match opm_fs::move_project_path(&root, &rel, &dest) {
                            Ok(new_path) => me.retarget_after_fs_move(&target, &new_path),
                            Err(err) => show_error(&me.window, &err.to_string()),
                        }
                    }),
                );
            }
            ExplorerAction::ImportFiles => {
                // §15: el nodo del click derecho es la carpeta destino; el
                // lote se pré-valida entero en `opm_fs::import_files`.
                let dest = parent_dir_of(&target, &root);
                let window = self.window.clone();
                glib::spawn_future_local(async move {
                    let dialog = gtk4::FileDialog::new();
                    dialog.set_title("Import Files");
                    dialog.set_modal(true);
                    let model = match dialog.open_multiple_future(Some(&window)).await {
                        Ok(model) => model,
                        Err(_) => return, // cancelado
                    };
                    let mut sources = Vec::new();
                    for index in 0..model.n_items() {
                        let Some(file) = model.item(index).and_downcast::<gio::File>() else {
                            continue;
                        };
                        let Some(path) = file.path() else { continue };
                        let name = file_name_of(&path);
                        if !name.is_empty() {
                            sources.push(opm_fs::ImportSource { source: path, name });
                        }
                    }
                    if sources.is_empty() {
                        return;
                    }
                    let dest_rel = rel_to(&root, &dest).unwrap_or_default();
                    match opm_fs::import_files(&root, &dest_rel, &sources) {
                        Ok(_) => me.refresh_explorer(),
                        Err(err) => show_error(&window, &err.to_string()),
                    }
                });
            }
        }
    }

    /// Tras un rename/move propio en disco (menú contextual o arrastre P2-12):
    /// re-objetiva las pestañas de texto —huellas de guardado incluidas,
    /// P2-9—, la página de imagen, repinta y relee el árbol (§11 A11).
    fn retarget_after_fs_move(&self, source: &Path, new_path: &Path) {
        self.tabs.borrow_mut().retarget_path(source, new_path);
        self.retarget_image(source, new_path);
        self.reload_active();
        self.refresh_explorer();
    }

    /// Soltado en una fila del explorador (P2-12, MANUAL §2/A11): mover un
    /// path del proyecto dentro del destino con la misma re-objetivación que
    /// los flujos del menú contextual, o importar ficheros arrastrados desde
    /// el SO (lote pré-validado en `opm_fs::import_files`, §15).
    fn explorer_drop(&self, drop: ExplorerDrop, target_dir: PathBuf) {
        let Some(me) = self.slot() else { return };
        let Some(root) = self.scope.project_root() else { return };
        let window = self.window.clone();
        match drop {
            ExplorerDrop::Import { sources } => {
                let sources: Vec<opm_fs::ImportSource> = sources
                    .into_iter()
                    .filter_map(|path| {
                        let name = file_name_of(&path);
                        (!name.is_empty()).then(|| opm_fs::ImportSource { source: path, name })
                    })
                    .collect();
                if sources.is_empty() {
                    return;
                }
                glib::spawn_future_local(async move {
                    let dest_rel = rel_to(&root, &target_dir).unwrap_or_else(|| ".".to_owned());
                    match opm_fs::import_files(&root, &dest_rel, &sources) {
                        Ok(_) => me.refresh_explorer(),
                        Err(err) => show_error(&window, &err.to_string()),
                    }
                });
            }
            ExplorerDrop::Move { source } => {
                // Soltar en el propio directorio es un no-op (sin error 409
                // del `move_project_path`).
                if source == target_dir || source.parent() == Some(target_dir.as_path()) {
                    return;
                }
                let Some(rel) = rel_to(&root, &source) else { return };
                let dest_rel = rel_to(&root, &target_dir).unwrap_or_else(|| ".".to_owned());
                // Aviso de descendientes sucios con la MISMA estructura que
                // el borrado (P2-14): mover REUBICA los buffers abiertos en
                // sus rutas nuevas — el verbo del aviso no promete descartar.
                let dirty = dirty_tabs_under(&me.tabs.borrow(), &source);
                if dirty.is_empty() {
                    me.move_project_entry(root, rel, dest_rel, source);
                    return;
                }
                glib::spawn_future_local(async move {
                    let files = dirty.join("\n");
                    let detail = format!(
                        "Moving {} will relocate unsaved changes in:\n{files}\n\n\
                         Move these files and folders?",
                        source.display()
                    );
                    if !dialogs::confirm_message(&window, "Move?", &detail, "Move").await {
                        return;
                    }
                    me.move_project_entry(root, rel, dest_rel, source);
                });
            }
        }
    }

    /// Ejecuta el move del explorador (menú y arrastre comparten este
    /// camino): `opm_fs::move_project_path` + re-objetivación (A11) o error.
    fn move_project_entry(&self, root: PathBuf, rel: String, dest_rel: String, source: PathBuf) {
        match opm_fs::move_project_path(&root, &rel, &dest_rel) {
            Ok(new_path) => self.retarget_after_fs_move(&source, &new_path),
            Err(err) => show_error(&self.window, &err.to_string()),
        }
    }
}

impl AppCmds {
    // --- Dominio de proyecto (§6) -----------------------------------------

    /// ¿El store tiene un proyecto abierto? Si no, muestra el error claro
    /// del §6 y devuelve `false` (los comandos de proyecto se niegan).
    fn require_project(&self) -> bool {
        if self.store.project_root().is_ok() {
            return true;
        }
        show_error(
            &self.window,
            "No project open — use Project ▸ New project or Open project",
        );
        false
    }

    /// Refresco del dominio (canvas, panel, contadores) SIN tocar los
    /// editores (§11 A12: el event pump jamás recarga el editor activo).
    fn refresh_domain(&self) {
        let snapshot = self.store.snapshot();
        let selected = self.selected_step.borrow().clone();
        if let Some(canvas) = &self.canvas {
            canvas.update(CanvasData::new(&snapshot, selected.as_deref()));
        }
        if let Some(counters) = &self.counters {
            counters.set_issues(&snapshot.validation);
        }
        if let Some(panel) = &self.step_panel {
            // El panel refresca EL PASO QUE MUESTRA (fuente: `current_step`).
            let detail = panel
                .current_step()
                .and_then(|id| self.store.step_detail(&id).ok());
            panel.refresh(detail.as_ref());
        }
    }

    /// Selección de paso (lienzo, resumen de validación, importación):
    /// detalle + log del paso en el panel derecho e inferior (paridad SPA
    /// `handleSelectStep`). Un id que ya no existe limpia la selección.
    fn select_step(&self, step: Option<String>) {
        let detail = match step.as_deref() {
            Some(id) => self.store.step_detail(id).ok(),
            None => None,
        };
        let shown = detail.as_ref().map(|detail| detail.step.id.clone());
        *self.selected_step.borrow_mut() = shown.clone();
        if let Some(panel) = &self.step_panel {
            panel.select(detail.as_ref());
        }
        if let Some(logs) = &self.logs {
            let content = shown
                .as_deref()
                .map(|id| self.store.log_content(id))
                .unwrap_or_default();
            logs.show_step(shown.as_deref(), &content);
        }
        if let Some(canvas) = &self.canvas {
            let snapshot = self.store.snapshot();
            canvas.update(CanvasData::new(&snapshot, shown.as_deref()));
        }
    }

    /// Añade `source` a `target.dependencies` (unión, drag nodo→nodo del
    /// lienzo, A9) y refresca.
    fn add_dependency(&self, target: &str, source: &str) {
        let Some(step) = self.store.step(target) else {
            show_error(&self.window, &format!("Unknown step: {target}"));
            return;
        };
        if step.dependencies.iter().any(|dep| dep == source) {
            return;
        }
        let mut dependencies = step.dependencies;
        dependencies.push(source.to_owned());
        if let Err(err) = self.store.set_dependencies(target, dependencies) {
            show_error(&self.window, &err.to_string());
            return;
        }
        self.refresh_domain();
    }

    /// Elimina `source` de `target.dependencies` (borrado de arista, A9).
    fn remove_dependency(&self, target: &str, source: &str) {
        let Some(step) = self.store.step(target) else {
            return;
        };
        let previous = step.dependencies.len();
        let dependencies: Vec<String> = step
            .dependencies
            .into_iter()
            .filter(|dep| dep != source)
            .collect();
        if dependencies.len() == previous {
            return;
        }
        if let Err(err) = self.store.set_dependencies(target, dependencies) {
            show_error(&self.window, &err.to_string());
            return;
        }
        self.refresh_domain();
    }

    /// Arranque de ejecución (§6, A10): `Ok` → UI «en curso» + logs limpios
    /// (paridad SPA); `Blocked` → `message + "step.field: msg; …"` con el
    /// preflight del motor como autoridad.
    fn start_run(&self, result: Result<(), ExecError>) {
        match result {
            Ok(()) => {
                self.running.set(true);
                self.apply_running_ui();
                if let Some(logs) = &self.logs {
                    logs.clear();
                }
            }
            Err(ExecError::Blocked { message, blockers }) => {
                show_error(&self.window, &format_blocked(&message, &blockers));
            }
            // `ExecError` puede crecer (variantes nuevas): el Display del
            // motor es la fuente del texto.
            Err(err) => show_error(&self.window, &err.to_string()),
        }
    }

    /// Ejecuta UN paso (botón Run del panel derecho, A10: habilitado en
    /// reposo).
    fn run_step(&self, step_id: String) {
        if self.running.get() {
            return;
        }
        self.start_run(self.exec.run_steps(vec![step_id]));
    }

    /// Abre el diálogo crear/editar paso (§6); al enviarlo se persiste con
    /// `store.create_step` / `store.update_step` (la propagación de rename
    /// la trae el core).
    fn open_step_dialog(&self, draft: StepDraft, steps: Vec<PipelineStep>) {
        let Some(me) = self.slot() else { return };
        let sink = Rc::clone(&me);
        // P2-15: la raíz alimenta las opciones «From output» con los mismos
        // helpers de rutas que el store.
        let root = me.store.project_root().ok();
        pipeline::step_dialog::open_step_dialog(
            &self.window,
            draft,
            steps,
            root,
            Rc::new(move |original_id, step| sink.submit_step(original_id, step)),
        );
    }

    /// Persiste el paso armado por el diálogo (crear o edición) y lo
    /// selecciona (el usuario ve el resultado de inmediato).
    fn submit_step(&self, original_id: Option<String>, step: PipelineStep) {
        let result = match &original_id {
            Some(old_id) => self.store.update_step(old_id, step.clone()),
            None => self.store.create_step(step.clone()),
        };
        match result {
            Ok(()) => {
                self.select_step(Some(step.id.clone()));
                self.refresh_domain();
                self.refresh_explorer();
            }
            Err(err) => show_error(&self.window, &err.to_string()),
        }
    }

    /// Sensibilidad de acciones y botones según la ejecución (A10: las
    /// acciones de run están habilitadas en reposo y deshabilitadas SOLO
    /// mientras corre; A12: cambio de proyecto y mutaciones estructurales
    /// deshabilitados mientras corre).
    fn apply_running_ui(&self) {
        let running = self.running.get();
        for action in [
            "new-project",
            "open-project",
            "open-folder",
            "import-pipeline",
            "create-step",
            "run-selected",
        ] {
            self.set_action_enabled(action, !running);
        }
        self.set_action_enabled("stop", running);
        if let Some(panel) = &self.step_panel {
            panel.set_running(running);
        }
        if let Some(button) = &self.run_btn {
            button.set_sensitive(!running);
            button.set_label(if running { "Running" } else { "Run selected" });
        }
        if let Some(button) = &self.stop_btn {
            button.set_sensitive(running);
        }
        if let Some(logs) = &self.logs {
            logs.stop_button().set_sensitive(running);
        }
    }

    /// Tick del event pump (§11 A12): drenado NO bloqueante con lote
    /// acotado, anexos de log agrupados y refresco de dominio una sola vez
    /// por lote — NUNCA se recarga el editor activo.
    fn pump_tick(&self) {
        drain(&self.exec, &mut self.run_model.borrow_mut());
        // Líneas del lote: se anexan ANTES de cualquier reconciliación (los
        // `StepLog` van DELANTE de `ExecutionFinished` en la cola, FIFO).
        let pending = self.run_model.borrow_mut().take_logs();
        if !pending.is_empty() {
            if let Some(logs) = &self.logs {
                for (step_id, line) in &pending {
                    logs.append_line(step_id, line);
                }
            }
        }
        let was_running = self.running.get();
        let running = self.exec.status().running;
        if running != was_running {
            self.running.set(running);
            self.apply_running_ui();
        }
        let changed = self.run_model.borrow_mut().take_changed();
        if changed {
            self.refresh_domain();
        }
        // P2-5: la reconciliación FINAL del log se ancla al consumo de
        // `ExecutionFinished` (todos los `StepLog` encolados ya se anexaron
        // en este lote o en anteriores), NUNCA a la observación
        // independiente de `status().running` — así un lote tardío no pega
        // líneas sobre el log completo ya sustituido.
        if self.run_model.borrow_mut().take_finished() {
            if let Some(logs) = &self.logs {
                let content = logs
                    .selected()
                    .as_deref()
                    .map(|id| self.store.log_content(id))
                    .unwrap_or_default();
                logs.set_content(&content);
            }
            // P2-13: al terminar, los outputs generados se vuelven visibles
            // en el árbol del explorador.
            self.refresh_explorer();
        }
    }

    // --- Páginas del stack central (§11 A11) ------------------------------

    /// Clave de página del stack para cada tipo de vista.
    fn page_name(view: &CenterView) -> String {
        match view {
            CenterView::Pipeline => "pipeline".to_owned(),
            CenterView::Editor => "editor".to_owned(),
            CenterView::Image(path) => imageview::page_id(path),
        }
    }

    /// Cambia la página visible del centro y repinta la franja de pestañas.
    /// Si la página pedida ya no existe, el fallback es la pestaña
    /// «Pipeline» (paridad SPA `CenterTabs`) o, en su defecto, el editor.
    fn show_view(&self, view: CenterView) {
        let chosen = if self.stack.child_by_name(&Self::page_name(&view)).is_some()
        {
            view
        } else if self.pipeline_btn.is_some() {
            CenterView::Pipeline
        } else {
            CenterView::Editor
        };
        *self.center_view.borrow_mut() = chosen.clone();
        self.stack
            .set_visible_child_name(&Self::page_name(&chosen));
        self.tab_bar
            .sync(&self.tabs.borrow(), chosen == CenterView::Editor);
        self.sync_strip();
    }

    /// Repinta la franja: estado del botón «Pipeline» + pestañas de imagen
    /// (etiqueta activa con estilo de pestaña seleccionada + cierre ✕).
    fn sync_strip(&self) {
        let view = self.center_view.borrow().clone();
        if let Some(button) = &self.pipeline_btn {
            if view == CenterView::Pipeline {
                button.add_css_class("suggested-action");
            } else {
                button.remove_css_class("suggested-action");
            }
        }
        while let Some(child) = self.image_strip.first_child() {
            self.image_strip.remove(&child);
        }
        for path in self.image_tabs.borrow().iter() {
            let row = GtkBox::new(Orientation::Horizontal, 0);
            let open = gtk4::Button::with_label(&imageview::page_title(path));
            open.set_tooltip_text(Some(&path.display().to_string()));
            if view == CenterView::Image(path.clone()) {
                open.add_css_class("suggested-action");
            }
            {
                let slot = self.slot.clone();
                let target = path.clone();
                open.connect_clicked(move |_| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        me.show_view(CenterView::Image(target.clone()));
                    }
                });
            }
            let close = gtk4::Button::with_label("✕");
            close.add_css_class("flat");
            close.set_tooltip_text(Some("Close image"));
            {
                let slot = self.slot.clone();
                let target = path.clone();
                close.connect_clicked(move |_| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        me.close_image(&target);
                    }
                });
            }
            row.append(&open);
            row.append(&close);
            self.image_strip.append(&row);
        }
    }

    /// Abre `path` en su página dedicada de imagen (A11: `gtk::Picture`,
    /// nunca a través de la carga UTF-8 de `TabManager`).
    fn open_image(&self, path: PathBuf) {
        let id = imageview::page_id(&path);
        let known = self
            .image_tabs
            .borrow()
            .iter()
            .any(|other| imageview::page_id_matches(&id, other));
        if !known {
            let page = imageview::build_page(&path);
            self.stack.add_named(&page, Some(&id));
            self.image_tabs.borrow_mut().push(path.clone());
        }
        self.show_view(CenterView::Image(path));
    }

    /// Cierra la página de imagen (✕ de su pestaña). La vista activa vuelve
    /// a «Pipeline» o al editor (paridad SPA).
    fn close_image(&self, path: &Path) {
        let id = imageview::page_id(path);
        if let Some(child) = self.stack.child_by_name(&id) {
            self.stack.remove(&child);
        }
        self.image_tabs.borrow_mut().retain(|other| other != path);
        let active = *self.center_view.borrow() == CenterView::Image(path.to_path_buf());
        if active {
            let fallback = if self.pipeline_btn.is_some() {
                CenterView::Pipeline
            } else {
                CenterView::Editor
            };
            self.show_view(fallback);
        } else {
            self.sync_strip();
        }
    }

    /// Retira las páginas de imagen bajo `path` (o el propio `path`) tras
    /// un borrado (paridad SPA `handleDeletePath`). Por segmentos con
    /// separadores normalizados (P1).
    fn close_images_under(&self, path: &Path) {
        let affected: Vec<PathBuf> = self
            .image_tabs
            .borrow()
            .iter()
            .filter(|other| paths::is_descendant(other, path))
            .cloned()
            .collect();
        for target in affected {
            self.close_image(&target);
        }
    }

    /// Re-objetiva las páginas de imagen de `from` (o sus descendientes) a
    /// `to` tras un rename/move (A11: lo mismo que `TabManager::retarget_path`
    /// con las pestañas de texto). Comparación y sufijo por segmentos con
    /// separadores normalizados (P1).
    fn retarget_image(&self, from: &Path, to: &Path) {
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        {
            let mut image_tabs = self.image_tabs.borrow_mut();
            for path in image_tabs.iter_mut() {
                // Sin sufijo el destino es `to` (`retargeted` no hace
                // `join("")`, que añadiría «.png/»).
                let Some(new_path) = paths::retargeted(path, from, to) else {
                    continue;
                };
                let old_path = path.clone();
                let old_id = imageview::page_id(&old_path);
                if let Some(child) = self.stack.child_by_name(&old_id) {
                    self.stack.remove(&child);
                    self.stack
                        .add_named(&child, Some(&imageview::page_id(&new_path)));
                }
                *path = new_path.clone();
                moved.push((old_path, new_path));
            }
        }
        if moved.is_empty() {
            return;
        }
        // Si la vista activa era una imagen re-objetivada, sigue siéndolo.
        let current = self.center_view.borrow().clone();
        if let CenterView::Image(path) = current {
            if let Some((_, new_path)) = moved.iter().find(|(old, _)| *old == path) {
                *self.center_view.borrow_mut() = CenterView::Image(new_path.clone());
                self.stack
                    .set_visible_child_name(&imageview::page_id(new_path));
            }
        }
        self.sync_strip();
    }
}

impl Commands for AppCmds {
    fn new(&self) {
        self.tabs.borrow_mut().new_tab();
        // P2-10: tras crear la pestaña, la página visible vuelve al editor
        // (Ctrl+N en la página «Pipeline»/de imagen debe verse el resultado).
        self.show_view(CenterView::Editor);
        self.reload_active();
    }

    fn open(&self) {
        let Some(me) = self.slot() else { return };
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            let Some(path) = dialogs::open_file(&window).await else {
                return;
            };
            me.open_path(&path);
        });
    }

    fn open_folder(&self) {
        let Some(_me) = self.slot() else { return };
        let window = self.window.clone();
        let app = self.app.clone();
        glib::spawn_future_local(async move {
            let Some(folder) = dialogs::open_folder(&window).await else {
                return;
            };
            // ISSUE-021: cada carpeta se abre en su propia ventana con su
            // explorador.
            build_window(&app, WindowScope::Project(folder), &[]);
        });
    }

    fn save(&self) {
        let result = self.tabs.borrow_mut().save_active();
        match result {
            Ok(()) => self.reload_active(),
            // §12.4: sin archivo asignado, Save As.
            Err(FsError::MissingPath) => self.save_as(),
            // §12.7: nunca sobrescritura silenciosa; solo con confirmación
            // explícita del usuario (§3.7) se sobrescribe el cambio externo,
            // o bien se recarga desde disco si el buffer está limpio.
            Err(FsError::ExternalModification { path }) => {
                let Some(me) = self.slot() else { return };
                let window = self.window.clone();
                glib::spawn_future_local(async move {
                    // «Recargar» solo se ofrece con el buffer limpio: con
                    // cambios sin guardar el usuario decide entre
                    // «Sobrescribir» y seguir editando (`reload_from_disk`
                    // nunca toca un buffer sucio, §12.7).
                    let dirty = me
                        .tabs
                        .borrow()
                        .active()
                        .editor
                        .document()
                        .is_modified();
                    match dialogs::confirm_overwrite_external(&window, &path, dirty).await {
                        dialogs::ExternalChange::Overwrite => {
                            if let Err(err) =
                                save_confirmed_and_reload(&me.tabs, || me.reload_active())
                            {
                                show_error(&window, &err.to_string());
                            }
                        }
                        dialogs::ExternalChange::Reload => {
                            match me.tabs.borrow_mut().reload_from_disk(&path) {
                                Ok(()) => me.reload_active(),
                                Err(err) => show_error(&window, &err.to_string()),
                            }
                        }
                        dialogs::ExternalChange::Cancel => {}
                    }
                });
            }
            Err(err) => show_error(&self.window, &err.to_string()),
        }
    }

    fn save_as(&self) {
        let Some(me) = self.slot() else { return };
        let window = self.window.clone();
        let suggested = {
            let tabs = self.tabs.borrow();
            let tab = tabs.active();
            match &tab.path {
                Some(path) => path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "untitled.txt".to_owned()),
                None => format!("untitled-{}.txt", tab.untitled_number),
            }
        };
        glib::spawn_future_local(async move {
            let Some(path) = dialogs::save_file_as(&window, &suggested).await else {
                return;
            };
            let saved = me.tabs.borrow_mut().save_active_as(&path);
            match saved {
                Ok(()) => me.reload_active(),
                Err(err) => show_error(&window, &err.to_string()),
            }
        });
    }

    fn close_tab(&self) {
        // P2-10: Ctrl+W despacha por la página visible (A11): la imagen se
        // cierra en su página, «Pipeline» es permanente (no se cierra) y el
        // editor mantiene el flujo clásico con su confirmación §3.7.
        match self.center_view.borrow().clone() {
            CenterView::Image(path) => {
                self.close_image(&path);
                return;
            }
            CenterView::Pipeline => return,
            CenterView::Editor => {}
        }
        let (modified, title) = {
            let tabs = self.tabs.borrow();
            let index = tabs.active_index();
            (tabs.is_modified(index), tabs.title(index))
        };
        if !modified {
            self.tabs.borrow_mut().close_active();
            self.reload_active();
            return;
        }
        // §3.7: descartar cambios solo con confirmación explícita.
        let Some(me) = self.slot() else { return };
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            if dialogs::confirm_discard(&window, &title).await {
                me.tabs.borrow_mut().close_active();
                me.reload_active();
            }
        });
    }

    fn undo(&self) {
        self.tabs.borrow_mut().undo_active();
        self.reload_active();
    }

    fn redo(&self) {
        self.tabs.borrow_mut().redo_active();
        self.reload_active();
    }

    fn find(&self) {
        // §40.1/§39: Ctrl+F, modo búsqueda.
        self.find_open(false);
    }

    fn replace(&self) {
        // §40.1/§39: Ctrl+H, con la fila de reemplazo visible.
        self.find_open(true);
    }

    fn toggle_mode(&self) {
        // §5: solo Markdown tiene modo Visual; la acción además está
        // deshabilitada para el resto de tipos, esta guarda es la defensa.
        if !active_is_markdown(&self.tabs) {
            return;
        }
        self.tabs.borrow_mut().toggle_mode_active();
        self.reload_active();
    }

    fn toggle_explorer(&self) {
        // §14.3: el explorador solo existe en ventanas de proyecto; en
        // standalone Ctrl+B es un no-op documentado.
        let (Some(paned), Some(_)) = (&self.explorer_paned, &self.explorer) else {
            return;
        };
        if self.explorer_visible.get() {
            // El ancho actual se guarda antes de ocultar (§14.3).
            self.persist_explorer_state();
            paned.set_start_child(Option::<&Widget>::None);
            self.explorer_visible.set(false);
            self.explorer_state.borrow_mut().visible = false;
        } else {
            let width = self.explorer_state.borrow().width;
            if let Some(panel) = &self.explorer_panel {
                paned.set_start_child(Some(panel));
            }
            paned.set_position(width);
            self.explorer_visible.set(true);
            self.explorer_state.borrow_mut().visible = true;
        }
        self.persist_explorer_state();
    }

    /// Refresh (P2-13, Project ▸ Refresh y botón del explorador): marca con
    /// ⚠ los ficheros abiertos que cambiaron en disco (huella del LocalFs,
    /// §12.7 — sin recargar ningún buffer) y relee el árbol (los outputs
    /// generados aparecen sin reiniciar).
    fn refresh(&self) {
        self.tabs.borrow_mut().mark_external_changes();
        self.refresh_explorer();
    }

    /// Crea un proyecto (§6, Project ▸ New project) y abre SU ventana con
    /// el scaffold ya en disco.
    fn new_project(&self) {
        let app = self.app.clone();
        let window = self.window.clone();
        project::create_dialog(&window, Rc::new(move |root| {
            build_window(&app, WindowScope::Project(root), &[]);
        }));
    }

    /// Abre un proyecto existente (§6): la carpeta se valida contra
    /// `project.config.json` y un falto claro nunca abre una ventana
    /// degradada.
    fn open_project(&self) {
        let app = self.app.clone();
        let window = self.window.clone();
        glib::spawn_future_local(async move {
            let Some(folder) = project::pick_project(&window).await else {
                return;
            };
            build_window(&app, WindowScope::Project(folder), &[]);
        });
    }

    /// Importa un `.yml`/`.yaml`/`.json` al proyecto abierto (§6). La
    /// selección de paso se conserva si el paso sigue existiendo (paridad
    /// SPA `refresh`).
    fn import_pipeline(&self) {
        let Some(me) = self.slot() else { return };
        if me.running.get() || !me.require_project() {
            return;
        }
        let window = me.window.clone();
        glib::spawn_future_local(async move {
            let Some(file) = dialogs::open_pipeline(&window).await else {
                return;
            };
            match me.store.import_pipeline(&file) {
                Ok(_) => {
                    let keep = me.selected_step.borrow().clone();
                    me.refresh_domain();
                    me.select_step(keep);
                    me.refresh_explorer();
                }
                Err(err) => show_error(&window, &err.to_string()),
            }
        });
    }

    /// Exporta el pipeline del proyecto a JSON (§6, Project ▸ Export
    /// pipeline): lectura, así sigue habilitado mientras corre.
    fn export_pipeline(&self) {
        let Some(me) = self.slot() else { return };
        if !me.require_project() {
            return;
        }
        let window = me.window.clone();
        glib::spawn_future_local(async move {
            let Some(dest) = dialogs::save_pipeline(&window, "pipeline.json").await else {
                return;
            };
            if let Err(err) = me.store.export_pipeline(&dest) {
                show_error(&window, &err.to_string());
            }
        });
    }

    /// Diálogo de creación de paso (§6, Pipeline ▸ Create step): id auto
    /// `step_<n+1>` y, al guardar, `store.create_step` / `update_step`.
    fn create_step(&self) {
        let Some(me) = self.slot() else { return };
        if me.running.get() || !me.require_project() {
            return;
        }
        let steps = me.store.steps();
        let draft = StepDraft::for_create(&steps);
        me.open_step_dialog(draft, steps);
    }

    /// Ejecuta los pasos seleccionados — o los no `ok` si no hay selección
    /// (§4 opm-exec; A10: habilitado en reposo, el preflight del motor es
    /// autoridad sobre los blockers).
    fn run_selected(&self) {
        if self.running.get() {
            return;
        }
        if !self.require_project() {
            return;
        }
        if self.store.steps().is_empty() {
            show_error(
                &self.window,
                "No pipeline imported — use Project ▸ Import pipeline",
            );
            return;
        }
        self.start_run(self.exec.run_selected());
    }

    /// Detiene la ejecución en curso (§6): el tick de cierre/cierre de
    /// ventana recoge y espera al motor (A12).
    fn stop(&self) {
        if self.exec.status().running {
            self.exec.stop();
        }
    }

    fn about(&self) {
        let about = gtk4::AboutDialog::builder()
            .program_name("opm")
            .version(env!("CARGO_PKG_VERSION"))
            .comments(
                "Open Pipeline Manager — project tree, pipeline canvas and runs.\n\
                 License: LicenseRef-Attribution-NonCommercial (see LICENSE).",
            )
            .license("LicenseRef-Attribution-NonCommercial — see LICENSE")
            .modal(true)
            .transient_for(&self.window)
            .build();
        about.present();
    }

    fn shortcuts(&self) {
        let mut detail = String::new();
        for (action, accel) in ACCELERATORS {
            // La fila muestra la etiqueta humana del comando (§36) con su
            // identificador entre paréntesis para poder enlazarlo.
            let label = menus::actions::ALL_COMMANDS
                .iter()
                .find(|command| command.action_id() == *action)
                .map(|command| command.label())
                .unwrap_or(action);
            detail.push_str(&format!("{accel}\t{label} ({action})\n"));
        }
        let alert = gtk4::AlertDialog::builder()
            .message("Atajos (§39)")
            .detail(detail.trim_end())
            .buttons(["Cerrar"])
            .modal(true)
            .build();
        alert.choose(
            Some(&self.window),
            std::option::Option::<&gio::Cancellable>::None,
            |_| {},
        );
    }
}

// --- Helpers puros -----------------------------------------------------------

/// Ruta comparable: canónica si se resuelve y la propia si no (§12.7: dos
/// grafías de un mismo fichero son el mismo fichero).
fn ruta_comparable(path: &Path) -> PathBuf {
    opm_fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Ruta de `target` relativa a la raíz del proyecto con separadores `/`;
/// `None` si `target` queda fuera de `root`.
fn rel_to(root: &Path, target: &Path) -> Option<String> {
    let rel = target.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

/// `requested` root-relative de una entrada NUEVA bajo `parent` (§14.8):
/// `nombre` si `parent` es la raíz, `rel(parent)/nombre` en otro caso.
fn requested_under(root: &Path, parent: &Path, name: &str) -> Option<String> {
    let base = rel_to(root, parent)?;
    Some(if base.is_empty() {
        name.to_owned()
    } else {
        format!("{base}/{name}")
    })
}

/// Carpeta destino de una operación §14.8 sobre `target` (archivo → su
/// carpeta; carpeta → sí misma; raíz → `root`).
fn parent_dir_of(target: &Option<PathBuf>, root: &Path) -> PathBuf {
    match target {
        Some(path) if path.is_dir() => path.clone(),
        Some(path) => path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.to_path_buf()),
        None => root.to_path_buf(),
    }
}

/// Nombre mostrable de un path (último componente, o el path completo).
fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Títulos de las pestañas de texto MODIFICADAS cuyo fichero está dentro de
/// `target` (o es él mismo) — DECISIONS.md/A11: borrar la ruta pide
/// confirmación explícita antes de descartar esos cambios (paridad SPA
/// `handleDeletePath`). La comparación es por segmentos con separadores
/// normalizados (P1): una ruta serializada `folder\file.txt` también cuenta.
fn dirty_tabs_under(tabs: &TabManager, target: &Path) -> Vec<String> {
    (0..tabs.len())
        .filter(|&index| {
            let tab = &tabs.tabs()[index];
            tab.editor.document().is_modified()
                && tab
                    .path
                    .as_deref()
                    .is_some_and(|path| paths::is_descendant(path, target))
        })
        .map(|index| tabs.title(index))
        .collect()
}

/// Sobrescritura confirmada (§12.7 / P1-1): guarda el resultado del
/// guardado del tab activo en SU propia sentencia y solo entonces ejecuta
/// `after` (la recarga). Un `match tabs.borrow_mut().save_active_confirmed()`
/// con `after()` dentro del brazo mantendría el `RefMut` vivo mientras la
/// recarga vuelve a pedir el préstamo → pánico de doble préstamo. `after`
/// solo corre si el guardado pudo completarse.
fn save_confirmed_and_reload(
    tabs: &Rc<RefCell<TabManager>>,
    after: impl FnOnce(),
) -> Result<(), FsError> {
    let saved = tabs.borrow_mut().save_active_confirmed();
    if saved.is_ok() {
        after();
    }
    saved
}

/// ¿El tab activo es Markdown (§5: Visual solo para `.md`/`.markdown`)?
fn active_is_markdown(tabs: &RefCell<TabManager>) -> bool {
    let tabs = tabs.borrow();
    path_is_markdown(tabs.active().path.as_deref())
}

fn path_is_markdown(path: Option<&Path>) -> bool {
    path.and_then(|path| path.extension())
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            ext == "md" || ext == "markdown"
        })
        .unwrap_or(false)
}

/// Diálogo de error modal (§3.7: acciones observables). Visible para los
/// módulos del crate (`crate::show_error`).
fn show_error(parent: &impl IsA<gtk4::Window>, message: &str) {
    let alert = gtk4::AlertDialog::builder()
        .message("Error")
        .detail(message)
        .buttons(["Aceptar"])
        .modal(true)
        .build();
    alert.choose(
        Some(parent),
        std::option::Option::<&gio::Cancellable>::None,
        |_| {},
    );
}

// --- Tema y ventana ----------------------------------------------------------

/// CSS de la UI GTK desde la [`theme::Palette`] del tema detectado (§23):
/// lógica pura y testeable. Estilo «borde + cabecera» (decisión del usuario):
/// cada región es una caja `opm-panel` con borde de1px y cabecera tintada,
/// elevación declarada UNA sola vez (borde, sin sombras, sin gradientes).
fn theme_css(palette: &theme::Palette) -> String {
    format!(
        "window {{ background-color: {bg}; color: {fg}; }}\n\
         textview, textview > text {{ font-family: monospace; background-color: {bg}; color: {fg}; }}\n\
         .opm-tab-bar {{ background-color: {code}; }}\n\
         .opm-tab-bar button {{ padding: 2px 10px; }}\n\
         .opm-tab-bar button.suggested-action {{ border-bottom: 2px solid {fg}; font-weight: bold; }}\n\
         .opm-explorer {{ background-color: {bg}; color: {fg}; }}\n\
         .opm-explorer listview {{ background-color: {bg}; color: {fg}; }}\n\
         .opm-explorer listview button {{ background-color: {bg}; color: {fg}; }}\n\
         .opm-explorer listview label {{ color: {fg}; }}\n\
         .opm-find-bar {{ background-color: {code}; }}\n\
         .opm-panel {{ border: 1px solid alpha({fg}, 0.32); border-radius: 6px; background-color: {bg}; }}\n\
         .opm-panel-header {{ background-color: {code}; border-bottom: 1px solid alpha({fg}, 0.32); border-radius: 5px 5px 0 0; padding: 3px 8px; }}\n\
         .opm-panel-header label {{ color: {fg}; font-weight: bold; }}\n\
         .opm-panel-body {{ padding: 6px; }}\n\
         .opm-console, .opm-console textview, .opm-console textview > text {{ background-color: {code}; color: {fg}; }}\n\
         paned > separator {{ background-color: alpha({fg}, 0.25); min-width: 5px; min-height: 5px; }}\n",
        bg = palette.bg,
        fg = palette.fg,
        code = palette.code_bg,
    )
}

/// Estiliza la UI GTK con la paleta del tema detectado (§23).
fn install_theme_css(palette: &theme::Palette) {
    let css = theme_css(palette);
    let provider = CssProvider::new();
    provider.load_from_data(&css);
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Desconecta el grupo `win.*` de la ventana: sus acciones retienen
/// `Rc<dyn Commands>` → `AppCmds` → ventana; sin este desconectar, cerrar
/// dejaría un ciclo de referencias vivo (§11 A12, «close-cycle trap»).
fn disconnect_window_actions(window: &ApplicationWindow) {
    window.insert_action_group("win", None::<&gio::SimpleActionGroup>);
}

/// Caja de panel «borde + cabecera» (§6): `opm-panel` con borde de1px y
/// radio ≤6px, cabecera `opm-panel-header` con el título, y cuerpo
/// `opm-panel-body` con el contenido (inset para que el canvas anuncie su
/// extensión dentro del marco).
fn panel_box(title: &str, content: &impl IsA<Widget>) -> GtkBox {
    let wrapper = GtkBox::new(Orientation::Vertical, 0);
    wrapper.add_css_class("opm-panel");
    let header = GtkBox::new(Orientation::Horizontal, 6);
    header.add_css_class("opm-panel-header");
    let label = gtk4::Label::new(Some(title));
    label.set_halign(gtk4::Align::Start);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    header.append(&label);
    let body = GtkBox::new(Orientation::Vertical, 0);
    body.add_css_class("opm-panel-body");
    body.set_vexpand(true);
    wrapper.append(&header);
    wrapper.append(&body);
    body.append(content);
    content.set_vexpand(true);
    wrapper
}

/// Intentos de «reap» del motor al cerrar la ventana (50 ms × 200 = 10 s
/// máximos por fase; §11 A12: detener y recoger ANTES de cerrar). Tras la
/// primera fase sin éxito se escala con [`ExecutionManager::force_stop`] y
/// se dan OTROS 200 intentos (P1-3).
const REAP_ATTEMPTS: u32 = 200;

/// Detiene → escala si hace falta → recoge el motor → cierra. Cada intento
/// drena el event pump y solo cuando `status().running` baja se pasa al
/// cierre normal (§11 A12). Con `escalated` agotado Y el motor aún vivo
/// (p. ej. un descendiente heredó la tubería de salida), la ventana queda
/// ABIERTA con un error claro: **nunca se destruye con una ejecución viva**
/// (P1-3).
fn schedule_reap(
    window: ApplicationWindow,
    slot: Rc<CmdSlot>,
    attempts: u32,
    escalated: bool,
) {
    glib::timeout_add_local_once(Duration::from_millis(50), move || {
        let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) else {
            return;
        };
        drain(&me.exec, &mut me.run_model.borrow_mut());
        if !me.exec.status().running {
            drop(me);
            glib::spawn_future_local(close_after_checks(window, slot));
            return;
        }
        if attempts > 0 {
            drop(me);
            schedule_reap(window, slot, attempts - 1, escalated);
            return;
        }
        if escalated {
            // SIGTERM + SIGKILL + 20 s no han bastado: error visible y la
            // ventana SIGUE ABIERTA (el reap normal del pump sigue vivo y un
            // cierre posterior vuelve a intentarlo).
            show_error(
                &window,
                "The running execution could not be stopped, not even with a \
                 forced kill. The window stays open — use Stop to try again.",
            );
            drop(me);
            return;
        }
        // Escalada P1-3: SIGKILL al hijo inmediato + otra fase de reap.
        me.exec.force_stop();
        drop(me);
        schedule_reap(window, slot, REAP_ATTEMPTS, true);
    });
}

/// Cierre normal de la ventana: confirmación de descartes si hay cambios
/// sin guardar (§3.7), desconexión de las acciones y destrucción.
async fn close_after_checks(window: ApplicationWindow, slot: Rc<CmdSlot>) {
    let any_modified = slot
        .borrow()
        .as_ref()
        .and_then(Weak::upgrade)
        .map(|me| {
            me.tabs
                .borrow()
                .tabs()
                .iter()
                .any(|tab| tab.editor.document().is_modified())
        })
        .unwrap_or(false);
    if any_modified
        && !dialogs::confirm_discard(&window, "los documentos abiertos").await
    {
        return;
    }
    // P1-3: NUNCA destruir con el motor vivo (una carrera entre el reap y
    // los diálogos podría reactivarlo): si sigue corriendo, se para y vuelve
    // a la cadena de reap con su escalada.
    let running = slot
        .borrow()
        .as_ref()
        .and_then(Weak::upgrade)
        .is_some_and(|me| {
            if me.exec.status().running {
                me.exec.stop();
                true
            } else {
                false
            }
        });
    if running {
        schedule_reap(window, slot, REAP_ATTEMPTS, false);
        return;
    }
    disconnect_window_actions(&window);
    window.destroy();
}

fn build_window(app: &Application, scope: WindowScope, open_paths: &[PathBuf]) -> Rc<AppCmds> {
    let palette = theme::palette();
    install_theme_css(&palette);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("OPM")
        .default_width(1280)
        .default_height(800)
        .build();

    // Canoniza la raíz del árbol una vez: las filas del explorador cuelgan
    // de ella y las ops de `opm-fs` reciben rutas relativas a esta raíz.
    let scope = match scope {
        WindowScope::Project(root) => {
            WindowScope::Project(opm_fs::canonicalize(&root).unwrap_or(root))
        }
        WindowScope::Standalone => WindowScope::Standalone,
    };
    // Store + motor de ESTA ventana (§4 opm-store/opm-exec, A6: el store es
    // `Arc` porque el motor cruza hilos). En ventanas de proyecto se intenta
    // abrir; una carpeta que no es proyecto sigue siendo ventana con
    // explorador y el lienzo explica el motivo (§6, mensaje claro).
    let store = Arc::new(ProjectStore::new());
    let exec = Rc::new(ExecutionManager::new(Arc::clone(&store)));
    let is_project = matches!(scope, WindowScope::Project(_));
    if let WindowScope::Project(root) = &scope {
        if let Err(err) = store.open_project(root) {
            eprintln!("opm: no se pudo abrir el proyecto {}: {err}", root.display());
        }
    }
    // §11 A13: el hueco de sesión registra el último proyecto abierto con su
    // timestamp; `project::restore` lo restaura al arrancar con TTL de 1 h
    // y lo limpia si está caducado o inválido (§6).
    if let WindowScope::Project(root) = &scope {
        if store.project_root().is_ok() {
            if let Err(err) =
                appstate::save_last_project(&Some((root.display().to_string(), project::now_secs())))
            {
                eprintln!("opm: no se pudo guardar el último proyecto: {err}");
            }
        }
    }
    let slot: Rc<CmdSlot> = Rc::new(RefCell::new(None));
    let tabs = Rc::new(RefCell::new(TabManager::new()));
    tabs.borrow_mut().set_scope(scope.clone());
    for path in open_paths {
        // Las imágenes se abren tras construir `cmds` (página dedicada A11);
        // `TabManager` es solo texto y jamás carga binarios por UTF-8.
        if imageview::is_image_path(path) {
            continue;
        }
        let opened = tabs.borrow_mut().open(path);
        match opened {
            Ok(index) => tabs.borrow_mut().set_active(index),
            Err(err) => eprintln!("opm: no se pudo abrir {}: {err}", path.display()),
        }
    }

    // Puente TextView → modelo + refresco diferido (§47.1) en tardío.
    let sink = Rc::new(Sink { tabs: tabs.clone() });
    let on_edited = {
        let slot = slot.clone();
        Rc::new(move || {
            if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                me.schedule_refresh();
            }
        })
    };
    let editor_pane = Rc::new(EditorPane::new(sink, on_edited, &palette));

    // Barra de pestañas (§37) con su clase de tema; acciones en tardío.
    let tab_bar = Rc::new(TabBar::new({
        let slot = slot.clone();
        Rc::new(move |index| {
            if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                me.select_tab(index);
            }
        })
    }));
    // A11: en ventanas de proyecto la franja es permanente (el editor sigue
    // alcanzable junto a la pestaña «Pipeline» con un solo documento).
    tab_bar.set_persistent(matches!(scope, WindowScope::Project(_)));
    let tab_bar_widget = tab_bar.widget();

    // Barra de búsqueda/reemplazo del archivo actual (§40.1): acciones en
    // tardío por [`CmdSlot`], como el resto de widgets.
    let find_bar = Rc::new(FindBar::new({
        let slot = slot.clone();
        Rc::new(move |action| {
            if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                me.find_action(action);
            }
        })
    }));

    // Explorador (§14.3): solo en ventanas de proyecto, con el estado
    // restaurado desde `appstate` (§11 A13).
    let explorer_state = load_state();
    let explorer = match &scope {
        WindowScope::Project(root) => {
            let on_open: Rc<dyn Fn(PathBuf)> = {
                let slot = slot.clone();
                Rc::new(move |path: PathBuf| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        me.activate_or_open(&path);
                    }
                })
            };
            let on_action: Rc<dyn Fn(ExplorerAction, Option<PathBuf>)> = {
                let slot = slot.clone();
                Rc::new(move |action, target| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        me.explorer_op(action, target);
                    }
                })
            };
            // Arrastres del explorador (P2-12): mover dentro del proyecto o
            // importar ficheros del SO, con la confirmación y la
            // re-objetivación en el coordinador.
            let on_drop: Rc<dyn Fn(ExplorerDrop, PathBuf)> = {
                let slot = slot.clone();
                Rc::new(move |drop, target_dir| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        me.explorer_drop(drop, target_dir);
                    }
                })
            };
            // Botón Refresh de la cabecera (P2-13): el MISMO flujo que
            // Project ▸ Refresh (banderín ⚠ + relectura del árbol).
            let on_refresh: Rc<dyn Fn()> = {
                let slot = slot.clone();
                Rc::new(move || {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        me.refresh();
                    }
                })
            };
            Some(Rc::new(ExplorerPane::new(
                root, on_open, on_action, on_drop, on_refresh,
            )))
        }
        WindowScope::Standalone => None,
    };

    // Layout §6: menú (popover) + toolbar + `explorador | centro | panel de
    // paso` + logs; el centro es un stack con la página «Pipeline»
    // PERMANENTE (A11), la página de editor y las páginas de imagen.
    let menubar = PopoverMenuBar::from_model(Some(&build_menubar()));
    menubar.set_hexpand(true);

    // Franja de pestañas del centro: «Pipeline» (solo proyecto) + pestañas
    // de texto (TabBar) + pestañas de imagen con su ✕ (A11).
    let strip = GtkBox::new(Orientation::Horizontal, 0);
    strip.add_css_class("opm-tab-bar");
    let pipeline_btn = is_project.then(|| {
        let button = gtk4::Button::with_label("Pipeline");
        let slot = slot.clone();
        button.connect_clicked(move |_| {
            if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                me.show_view(CenterView::Pipeline);
            }
        });
        button
    });
    if let Some(button) = &pipeline_btn {
        strip.append(button);
    }
    strip.append(&tab_bar_widget);
    let image_strip = GtkBox::new(Orientation::Horizontal, 0);
    strip.append(&image_strip);

    // Stack central permanente (§11 A11): editor SIEMPRE presente; la
    // página «Pipeline» y el panel de paso solo existen en ventanas de
    // proyecto.
    let stack = Stack::new();
    stack.set_hexpand(true);
    stack.add_named(&editor_pane.widget(), Some("editor"));

    let canvas = if is_project {
        let callbacks = CanvasCallbacks {
            on_select: {
                let slot = slot.clone();
                Rc::new(move |step_id| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        me.select_step(Some(step_id));
                    }
                })
            },
            on_toggle: {
                let slot = slot.clone();
                Rc::new(move |step_id| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        if let Err(err) = me.store.toggle_selected(&step_id) {
                            show_error(&me.window, &err.to_string());
                        }
                        me.refresh_domain();
                    }
                })
            },
            on_connect: {
                let slot = slot.clone();
                Rc::new(move |source, target| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        // A9: unión de `source` en `target.dependencies`.
                        me.add_dependency(&target, &source);
                    }
                })
            },
            on_disconnect: {
                let slot = slot.clone();
                Rc::new(move |source, target| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        me.remove_dependency(&target, &source);
                    }
                })
            },
            on_reconnect: {
                let slot = slot.clone();
                Rc::new(move |source, target, new_target| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        // A9: reconexión = quitar de `target` + añadir en
                        // `new_target`.
                        me.remove_dependency(&target, &source);
                        me.add_dependency(&new_target, &source);
                    }
                })
            },
            on_move: {
                let slot = slot.clone();
                Rc::new(move |step_id, x, y| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        // Persiste SOLO el nodo movido (set_layout fusiona).
                        let positions = HashMap::from([(step_id, (x, y))]);
                        if let Err(err) = me.store.set_layout(positions) {
                            show_error(&me.window, &err.to_string());
                        }
                    }
                })
            },
            edits_enabled: {
                let slot = slot.clone();
                Rc::new(move || {
                    slot.borrow()
                        .as_ref()
                        .and_then(Weak::upgrade)
                        .map(|me| !me.running.get())
                        .unwrap_or(false)
                })
            },
        };
        let canvas = PipelineCanvas::new(palette, callbacks);
        stack.add_named(&canvas.widget(), Some("pipeline"));
        Some(canvas)
    } else {
        None
    };

    // Panel derecho de detalle (§6): editar/ejecutar paso + valores.
    let step_panel = is_project.then(|| {
        StepPanel::new(StepCallbacks {
            on_edit: {
                let slot = slot.clone();
                Rc::new(move |step| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        let steps = me.store.steps();
                        me.open_step_dialog(StepDraft::for_edit(&step), steps);
                    }
                })
            },
            on_run: {
                let slot = slot.clone();
                Rc::new(move |step_id| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        me.run_step(step_id);
                    }
                })
            },
            on_param: {
                let slot = slot.clone();
                Rc::new(move |step_id, key, value: Value| {
                    if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                        // Bool/select se commitean al instante; texto/número
                        // al salir del campo (paridad SPA `handleParams`).
                        let mut params = me.store.params_for(&step_id);
                        params.insert(key, value);
                        if let Err(err) = me.store.set_params(&step_id, params) {
                            show_error(&me.window, &err.to_string());
                        }
                        me.refresh_domain();
                    }
                })
            },
        })
    });

    // Contadores de validación del toolbar (§6) con su popover-resumen.
    let counters = is_project.then(|| {
        Counters::new({
            let slot = slot.clone();
            Rc::new(move |step_id| {
                if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                    me.select_step(Some(step_id));
                }
            })
        })
    });

    // Panel de logs inferior (§6): Stop siempre visible, su sensibilidad la
    // fija `apply_running_ui`.
    let logs = is_project.then(|| {
        LogsPanel::new(&palette, {
            let slot = slot.clone();
            Rc::new(move || {
                if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                    me.stop();
                }
            })
        })
    });

    // Toolbar §6: nombre del proyecto + contadores + Run selected, Stop,
    // Import pipeline y Create step.
    let (toolbar, run_btn, stop_btn) = if is_project {
        let toolbar = GtkBox::new(Orientation::Horizontal, 8);
        toolbar.set_margin_start(8);
        toolbar.set_margin_end(8);
        toolbar.set_margin_top(4);
        toolbar.set_margin_bottom(4);
        toolbar.add_css_class("opm-toolbar");
        let project_label = gtk4::Label::new(
            scope
                .project_root()
                .and_then(|root| {
                    root.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .as_deref(),
        );
        project_label.add_css_class("heading");
        toolbar.append(&project_label);
        if let Some(counters) = &counters {
            toolbar.append(&counters.widget());
        }
        let spacer = gtk4::Label::new(None);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);
        let button = |label: &str| {
            let button = gtk4::Button::with_label(label);
            button.add_css_class("flat");
            button
        };
        let run_btn = button("Run selected");
        {
            let slot = slot.clone();
            run_btn.connect_clicked(move |_| {
                if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                    me.run_selected();
                }
            });
        }
        let stop_btn = button("Stop");
        {
            let slot = slot.clone();
            stop_btn.connect_clicked(move |_| {
                if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                    me.stop();
                }
            });
        }
        let import_btn = button("Import pipeline");
        {
            let slot = slot.clone();
            import_btn.connect_clicked(move |_| {
                if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                    me.import_pipeline();
                }
            });
        }
        let create_btn = button("Create step");
        {
            let slot = slot.clone();
            create_btn.connect_clicked(move |_| {
                if let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) {
                    me.create_step();
                }
            });
        }
        toolbar.append(&run_btn);
        toolbar.append(&stop_btn);
        toolbar.append(&import_btn);
        toolbar.append(&create_btn);
        (Some(toolbar), Some(run_btn), Some(stop_btn))
    } else {
        (None, None, None)
    };

    // Columna central: franja + find bar + stack de páginas (solo ventanas
    // standalone; en proyecto se envuelve en su caja-panel más abajo).
    let center_col = GtkBox::new(Orientation::Vertical, 0);
    center_col.append(&strip);
    center_col.append(&find_bar.widget());
    center_col.append(&stack);
    stack.set_vexpand(true);

    // Estilo «borde + cabecera» (§6): cada región es una caja `opm-panel`
    // con cabecera (`Explorer`, `Pipeline`, `Step`, `Logs`) y los `Paned`
    // separadores tienen agarre visible (≥5px, CSS `paned > separator`).
    let (content, explorer_paned, explorer_visible, explorer_panel) = match &explorer
    {
        Some(explorer) => {
            let explorer_panel = panel_box("Explorer", &explorer.widget());
            let center_panel = panel_box("Pipeline", &center_col);
            // `explorador | (centro | panel de paso)`: dos mangos para las
            // tres cajas redimensionables del §6.
            let step_width = 360;
            let inner = Paned::new(Orientation::Horizontal);
            inner.set_wide_handle(true);
            inner.set_start_child(Some(&center_panel));
            if let Some(step_panel) = &step_panel {
                let step_panel_box = panel_box("Step", &step_panel.widget());
                inner.set_end_child(Some(&step_panel_box));
            }
            let explorer_width = if explorer_state.visible {
                explorer_state.width
            } else {
                0
            };
            inner.set_position(
                (window.default_width() - explorer_width - step_width).max(240),
            );
            let main_paned = Paned::new(Orientation::Horizontal);
            main_paned.set_wide_handle(true);
            main_paned.set_end_child(Some(&inner));
            if explorer_state.visible {
                main_paned.set_start_child(Some(&explorer_panel));
                main_paned.set_position(explorer_state.width);
            }
            // Partición horizontal visible sobre la consola de logs (§6:
            // «bottom logs»), con la consola autosuficiente dentro de su
            // propia caja-panel.
            let vertical = Paned::new(Orientation::Vertical);
            vertical.set_wide_handle(true);
            vertical.set_start_child(Some(&main_paned));
            if let Some(logs) = &logs {
                vertical.set_end_child(Some(&logs.widget()));
            }
            vertical.set_position((window.default_height() - 300).max(240));
            (
                vertical.clone().upcast::<Widget>(),
                Some(main_paned),
                explorer_state.visible,
                Some(explorer_panel),
            )
        }
        None => (center_col.clone().upcast::<Widget>(), None, false, None),
    };

    let vbox = GtkBox::new(Orientation::Vertical, 0);
    vbox.append(&menubar);
    if let Some(toolbar) = &toolbar {
        vbox.append(toolbar);
    }
    vbox.append(&content);
    content.set_vexpand(true);
    vbox.set_vexpand(true);
    window.set_child(Some(&vbox));

    let cmds = Rc::new(AppCmds {
        app: app.clone(),
        window: window.clone(),
        scope,
        tabs: tabs.clone(),
        editor_pane,
        tab_bar,
        find_bar,
        explorer,
        explorer_panel,
        explorer_paned,
        explorer_visible: Cell::new(explorer_visible),
        explorer_state: RefCell::new(explorer_state),
        store,
        exec,
        run_model: RefCell::new(RunModel::new()),
        running: Cell::new(false),
        selected_step: RefCell::new(None),
        stack,
        center_view: RefCell::new(if is_project {
            CenterView::Pipeline
        } else {
            CenterView::Editor
        }),
        pipeline_btn,
        image_tabs: RefCell::new(Vec::new()),
        image_strip,
        canvas,
        step_panel,
        counters,
        logs,
        run_btn,
        stop_btn,
        action_group: RefCell::new(None),
        refresh_scheduled: Rc::new(Cell::new(false)),
        slot: slot.clone(),
    });
    *slot.borrow_mut() = Some(Rc::downgrade(&cmds));
    // El grupo devuelve un `Rc` DÉBIL en `AppCmds`; la referencia fuerte la
    // retiene la ventana (§11 A12) hasta [`disconnect_window_actions`].
    *cmds.action_group.borrow_mut() = Some(install_actions(&window, app, cmds.clone()).downgrade());

    // Event pump (§11 A12): 250 ms, `next_event(ZERO)` con lote acotado y
    // anexos de log agrupados; los refresh son de dominio, NUNCA del editor.
    glib::timeout_add_local(Duration::from_millis(250), {
        let slot = slot.clone();
        move || {
            let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) else {
                return glib::ControlFlow::Break;
            };
            me.pump_tick();
            glib::ControlFlow::Continue
        }
    });

    // §3.7 / A12: al cerrar la ventana, con ejecución en curso se DETIENE y
    // se recoge el motor ANTES de continuar; los descartes sin guardar se
    // confirman explícitamente. El handler solo captura la ranura débil.
    window.connect_close_request({
        let slot = slot.clone();
        move |window| {
            let Some(me) = slot.borrow().as_ref().and_then(Weak::upgrade) else {
                return glib::Propagation::Proceed;
            };
            // §14.3: al cerrar se persiste el estado del explorador.
            me.persist_explorer_state();
            // Los popovers del resumen mueren con la ventana: se cierran
            // antes (§6, doc de `Counters::popdown`).
            if let Some(counters) = &me.counters {
                counters.popdown();
            }
            if me.exec.status().running {
                me.exec.stop();
                let window = window.clone();
                let slot = slot.clone();
                drop(me);
                schedule_reap(window, slot, REAP_ATTEMPTS, false);
                return glib::Propagation::Stop;
            }
            drop(me);
            let window = window.clone();
            glib::spawn_future_local(close_after_checks(window, slot.clone()));
            glib::Propagation::Stop
        }
    });

    // Página inicial: «Pipeline» en proyecto (paridad SPA), editor en
    // standalone; después el contenido activo y el primer refresco de
    // dominio (canvas, contadores, panel).
    cmds.show_view(if is_project {
        CenterView::Pipeline
    } else {
        CenterView::Editor
    });
    cmds.apply_running_ui();
    cmds.reload_active();
    cmds.refresh_domain();
    for path in open_paths {
        if imageview::is_image_path(path) {
            cmds.open_image(path.clone());
        }
    }
    window.present();
    cmds
}

fn main() {
    // Argumentos: los ficheros abren pestañas en una ventana standalone y
    // cada carpeta fija la raíz del explorador de su ventana.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut folders: Vec<PathBuf> = Vec::new();
    for arg in std::env::args().skip(1) {
        let path = PathBuf::from(arg);
        if path.is_dir() {
            folders.push(path);
        } else {
            files.push(path);
        }
    }
    let app = Application::builder().application_id("app.opm").build();
    app.connect_activate(move |app| {
        if !files.is_empty() {
            build_window(app, WindowScope::Standalone, &files);
        }
        for folder in &folders {
            build_window(app, WindowScope::Project(folder.clone()), &[]);
        }
        if files.is_empty() && folders.is_empty() {
            // §11 A13/§6: restauración de la sesión con TTL de 1 h. Una
            // sesión caducada se limpia en silencio (paridad SPA) y una que
            // apunta a algo inválido se limpia MOSTRANDO el error; sin
            // sesión, la ventana standalone de siempre.
            match project::restore() {
                project::Restore::Open(root) => {
                    build_window(app, WindowScope::Project(root), &[]);
                }
                project::Restore::Failed(message) => {
                    let cmds = build_window(app, WindowScope::Standalone, &[]);
                    show_error(&cmds.window, &message);
                }
                project::Restore::None => {
                    build_window(app, WindowScope::Standalone, &[]);
                }
            }
        }
    });
    // Los ficheros de la línea de comandos se abren en `connect_activate`; no
    // se pasan como argumentos de GApplication (que exigiría HANDLES_OPEN).
    app.run_with_args::<&str>(&["opm"]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El CSS generado referencia SOLO clases OPM (`opm-*`) y usa la paleta
    /// completa (fondo, texto y fondo de código).
    #[test]
    fn css_del_tema_usa_clases_opm() {
        for palette in [theme::Palette::LIGHT, theme::Palette::DARK] {
            let css = theme_css(&palette);
            assert!(css.contains(".opm-tab-bar"));
            assert!(css.contains(".opm-explorer"));
            assert!(css.contains(".opm-find-bar"));
            assert!(css.contains(palette.bg));
            assert!(css.contains(palette.fg));
            assert!(css.contains(palette.code_bg));
            assert!(!css.contains("mm-"), "sin clases mm- heredadas");
        }
    }

    /// Despacho de modo Visual (§5): solo `.md`/`.markdown`, insensible a
    /// mayúsculas y sin extensión para los ficheros sin path.
    #[test]
    fn solo_markdown_tiene_modo_visual() {
        assert!(path_is_markdown(Some(Path::new("a.md"))));
        assert!(path_is_markdown(Some(Path::new("A.MARKDOWN"))));
        assert!(!path_is_markdown(Some(Path::new("a.yml"))));
        assert!(!path_is_markdown(Some(Path::new("sinext"))));
        assert!(!path_is_markdown(None));
    }

    /// Rutas relativas a la raíz del proyecto (§14.8/§17).
    #[test]
    fn rel_to_y_requested_under() {
        let root = Path::new("/proy");
        assert_eq!(rel_to(root, Path::new("/proy")).as_deref(), Some(""));
        assert_eq!(
            rel_to(root, Path::new("/proy/cap/doc.md")).as_deref(),
            Some("cap/doc.md")
        );
        assert_eq!(rel_to(root, Path::new("/otro/x.md")), None);
        assert_eq!(
            requested_under(root, root, "new.txt").as_deref(),
            Some("new.txt")
        );
        assert_eq!(
            requested_under(root, Path::new("/proy/cap"), "new.txt").as_deref(),
            Some("cap/new.txt")
        );
    }

    /// P1-1 (regresión): la sobrescritura confirmada materializa el
    /// resultado del guardado en SU propia sentencia y solo entonces ejecuta
    /// `after`, que vuelve a prestar el `RefCell` (como hace
    /// `AppCmds::reload_active`). Un `match tabs.borrow_mut()
    /// .save_active_confirmed()` con `after()` dentro del brazo mantendría el
    /// `RefMut` vivo y moriría aquí con pánico de doble préstamo.
    #[test]
    fn guardado_confirmado_recarga_sin_doble_prestamo() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("opm-p1-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.md");
        std::fs::write(&path, "original\n").unwrap();

        let tabs: Rc<RefCell<TabManager>> = Rc::new(RefCell::new(TabManager::new()));
        tabs.borrow_mut().open(&path).unwrap();
        tabs.borrow_mut().insert_active(Position::new(0, 8), "!"); // «original!\n»
        // Un cambio externo en disco: es el escenario en el que `save`
        // devuelve ExternalModification y la UI llega al tramo confirmado.
        std::fs::write(&path, "externo\n").unwrap();

        let reloaded = Cell::new(false);
        let result = save_confirmed_and_reload(&tabs, || {
            // Recarga real: re-presta el RefCell (mismo acceso que
            // `AppCmds::reload_active`).
            let _ = tabs.borrow().active_index();
            reloaded.set(true);
        });
        assert!(result.is_ok(), "{result:?}");
        assert!(reloaded.get(), "la recarga corre tras liberar el préstamo");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original!\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P1 (regresión): la confirmación de borrado/movimiento del explorador
    /// detecta pestañas SUCIAS cuya ruta está serializada con el otro
    /// separador (`folder\file.md` bajo `folder`) y NO confunde `folderx`
    /// con `folder` — el check antiguo solo comparaba `/`.
    #[test]
    fn dirty_tabs_under_detecta_formas_mixtas_de_separadores() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("opm-p1-dirty-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Nombre con `\` literal (POSIX): la ruta del tab queda serializada
        // como la de un estado persistido en Windows.
        let path = dir.join("folder\\file.md");
        std::fs::write(&path, "contenido\n").unwrap();

        let mut tabs = TabManager::new();
        tabs.open(&path).unwrap();
        tabs.insert_active(Position::new(0, 0), "x"); // «xcontenido» ●

        let bajo_folder = dirty_tabs_under(&tabs, &dir.join("folder"));
        assert_eq!(
            bajo_folder.len(),
            1,
            "P1: la pestaña sucia folder\\file.md cuenta bajo folder: {bajo_folder:?}"
        );
        assert!(
            bajo_folder[0].contains("folder\\file.md"),
            "el título lista el fichero: {bajo_folder:?}"
        );

        // Prefijo crudo ≠ segmento: `folderx` no arrastra la pestaña.
        assert!(dirty_tabs_under(&tabs, &dir.join("folderx")).is_empty());
        // Y la propia ruta (paridad con `==`) sigue contando.
        assert_eq!(dirty_tabs_under(&tabs, &path).len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
