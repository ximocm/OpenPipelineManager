// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Explorador de archivos: panel lateral con árbol perezoso (ISSUE-022/023).
//!
//! - §14.3: panel lateral ocultable (`Ctrl+B`), ancho redimensionable y
//!   estado restaurable (la persistencia vive en [`state`] y delega en
//!   [`crate::appstate`], §11 A13).
//! - §14.4/§14.5: `Show All Files` y `Show Hidden Files` por ventana.
//! - §14.7: los symlinks se listan; las carpetas enlazadas no se expanden.
//! - §14.8: menú contextual con New File, New Folder, Import Files,
//!   `Rename`, Move, Delete y Copy Relative Path; el nodo del click es el
//!   destinatario (con click en vacío → raíz del proyecto). La ejecución de
//!   las operaciones vive en `main.rs` sobre las ops de `opm-fs`.
//! - P2-11: el listado de cada nivel lo da `opm_fs::list_tree_children`, LA
//!   política del árbol (carpetas primero case-insensitive, `IGNORED_NAMES`
//!   fuera, tope de profundidad 5) — la misma que `build_tree`, sin
//!   duplicarla aquí.
//! - P2-12 (MANUAL §2, A11): arrastre y soltado en las filas — mover paths
//!   del proyecto (contenido propio `application/x-opm-path`) o importar
//!   ficheros soltados desde el SO; la confirmación y la re-objetivación
//!   viven en `main.rs` vía `on_drop`.
//! - View owns the per-window file filters and Refresh command.
//! - §47.1/§47.5: los directorios se leen en una tarea aparte (un hilo por
//!   lectura) y el resultado se integra en el bucle principal con
//!   `glib::idle_add_once`: la UI jamás hace E/S y el árbol crece a demanda,
//!   directorio a directorio.
//! - Activar un archivo lo abre como pestaña vía `OpenTarget` (ISSUE-021);
//!   activar una carpeta la expande o colapsa.

mod model;
mod state;

pub use state::{load as load_state, save as save_state, ExplorerState};
// Formato `clave=valor` del explorador para el fichero único de
// `crate::appstate` (§11 A13: un solo dueño del archivo).
pub(crate) use state::{parse as parse_explorer_text, serialize as serialize_explorer_text};

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use opm_fs::{list_tree_children, TreeEntry};
use model::{ExplorerFilters, Row, TreeModel};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};

/// Indento por nivel de profundidad en el árbol.
const INDENT_PX: i32 = 16;

/// MIME del contenido propio de los arrastres internos del explorador
/// (P2-12): el path absoluto del nodo arrastrado, ofrecido además como texto
/// para que el `DropTarget` de la misma app lo resuelva por `G_TYPE_STRING`.
const OPM_PATH_MIME: &str = "application/x-opm-path";

/// Operaciones del menú contextual §14.8 (ISSUE-023).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplorerAction {
    NewFile,
    NewFolder,
    Rename,
    Move,
    Delete,
    CopyRelativePath,
    /// Import Files (§15, ISSUE-024): el nodo del click es la carpeta
    /// destino dentro del proyecto.
    ImportFiles,
}

/// Soltado en una fila del explorador (P2-12): `Move` mueve un path del
/// proyecto dentro de la carpeta destino; `Import` copia al proyecto los
/// ficheros soltados desde el SO.
#[derive(Debug, Clone)]
pub enum ExplorerDrop {
    Move { source: PathBuf },
    Import { sources: Vec<PathBuf> },
}

/// Contenido del arrastre interno: `application/x-opm-path` (bytes del path)
/// unido a una copia en texto, para que el `DropTarget` de la misma app lo
/// lea como `G_TYPE_STRING` sin registro de serializadores.
fn path_content_provider(path: &Path) -> gdk::ContentProvider {
    let text = path.to_string_lossy().into_owned();
    let bytes = glib::Bytes::from(text.as_bytes());
    gdk::ContentProvider::new_union(&[
        gdk::ContentProvider::for_bytes(OPM_PATH_MIME, &bytes),
        gdk::ContentProvider::for_value(&text.to_value()),
    ])
}

/// Acción de un soltado de TEXTO (P2-12): un path existente DENTRO del
/// proyecto es un move (el contenido propio arrastra paths absolutos del
/// árbol); uno fuera del proyecto es un fichero suelto del SO que se importa;
/// cualquier otra cosa (varias líneas, texto sin path, directorio externo —
/// `import_files` solo importa ficheros) se rechaza y el soltado no hace nada.
fn drop_text_action(text: &str, root: &Path) -> Option<ExplorerDrop> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }
    let raw = PathBuf::from(trimmed);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        root.join(raw)
    };
    let canon = opm_fs::canonicalize(&candidate).ok()?;
    if canon.starts_with(root) {
        // Dentro del proyecto: mover (archivo o carpeta).
        Some(ExplorerDrop::Move { source: canon })
    } else if canon.is_file() {
        // Fichero suelto del SO: se importa (el lote solo admite ficheros).
        Some(ExplorerDrop::Import {
            sources: vec![canon],
        })
    } else {
        // Directorio externo: `import_files` no lo admite → soltado nulo.
        None
    }
}

/// Menú contextual §14.8 en datos puros: `(acción, etiqueta)`; `Rename`
/// conserva su nombre inglés tal y como lo lista la SRS.
const CONTEXT_MENU_SPEC: &[(ExplorerAction, &str)] = &[
    (ExplorerAction::NewFile, "New File"),
    (ExplorerAction::NewFolder, "New Folder"),
    (ExplorerAction::ImportFiles, "Import Files"),
    (ExplorerAction::Rename, "Rename"),
    (ExplorerAction::Move, "Move"),
    (ExplorerAction::Delete, "Delete"),
    (ExplorerAction::CopyRelativePath, "Copy Relative Path"),
];

/// Nombre de la acción en el grupo `explorer.*`.
fn action_name(action: ExplorerAction) -> &'static str {
    match action {
        ExplorerAction::NewFile => "new-file",
        ExplorerAction::NewFolder => "new-folder",
        ExplorerAction::Rename => "rename",
        ExplorerAction::Move => "move",
        ExplorerAction::Delete => "delete",
        ExplorerAction::CopyRelativePath => "copy-relative-path",
        ExplorerAction::ImportFiles => "import-files",
    }
}

/// Native file navigator with a compact folder header and lazy tree rows.
pub struct ExplorerPane {
    widget: gtk::Box,
    header: gtk::Box,
    model: Arc<Mutex<TreeModel>>,
    store: gtk::gio::ListStore,
    selection: gtk::SingleSelection,
    active_path: Rc<RefCell<Option<PathBuf>>>,
    root: PathBuf,
}

impl ExplorerPane {
    /// Crea el panel para la raíz `root` (ventana de proyecto) y comienza su
    /// carga incremental (§47.5). `on_open(path)` abre un archivo activado;
    /// `on_action(acción, nodo)` ejecuta una operación §14.8 sobre el nodo
    /// del menú contextual (`None` = raíz del proyecto); `on_drop(drop,
    /// carpeta)` resuelve los arrastres sobre una fila (P2-12).
    pub fn new(
        root: &Path,
        on_open: Rc<dyn Fn(PathBuf)>,
        on_action: Rc<dyn Fn(ExplorerAction, Option<PathBuf>)>,
        on_drop: Rc<dyn Fn(ExplorerDrop, PathBuf)>,
    ) -> Self {
        let root = opm_fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let model: Arc<Mutex<TreeModel>> = Arc::new(Mutex::new(TreeModel::new(root.clone())));
        let store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("opm-explorer");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        header.add_css_class("opm-explorer-header");
        let title = gtk::Label::new(Some(
            root.file_name().and_then(|s| s.to_str()).unwrap_or("Files"),
        ));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_tooltip_text(Some(&root.display().to_string()));
        header.append(&title);
        for (icon, label, action) in [
            ("document-new-symbolic", "New file", ExplorerAction::NewFile),
            ("folder-new-symbolic", "New folder", ExplorerAction::NewFolder),
        ] {
            let button = gtk::Button::from_icon_name(icon);
            button.add_css_class("flat");
            button.set_tooltip_text(Some(label));
            button.update_property(&[gtk::accessible::Property::Label(label)]);
            let on_action = on_action.clone();
            button.connect_clicked(move |_| on_action(action, None));
            header.append(&button);
        }

        let selection = gtk::SingleSelection::new(Some(store.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let active_path: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
        // Rebuilding a lazy directory replaces row objects. Restore the active
        // document when its row reappears, including after filtering/refresh.
        store.connect_items_changed({
            let weak = selection.downgrade();
            let active_path = active_path.clone();
            move |store, position, _, added| {
                let Some(selection) = weak.upgrade() else { return; };
                let active = active_path.borrow();
                let Some(path) = active.as_deref() else { return; };
                for index in position..position + added {
                    if row_matches(store, index, path) {
                        selection.set_selected(index);
                        break;
                    }
                }
            }
        });

        // Menú contextual §14.8 sobre el nodo del click (None = raíz).
        let context_target: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
        let popover = build_context_menu(&widget, &context_target, &on_action);

        // Lista de filas: un botón plano por fila, indentado por profundidad.
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup({
            let model = model.clone();
            let store = store.clone();
            let on_open = Rc::clone(&on_open);
            let on_drop = Rc::clone(&on_drop);
            let root = root.clone();
            let host = widget.clone();
            let popover = popover.clone();
            let context_target = Rc::clone(&context_target);
            move |_, object| {
                let list_item = object
                    .downcast_ref::<gtk::ListItem>()
                    .expect("el factory de lista recibe gtk::ListItem");
                let label = gtk::Label::new(None);
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_hexpand(true);
                let arrow = gtk::Image::new();
                arrow.set_pixel_size(12);
                let icon = gtk::Image::new();
                icon.set_pixel_size(16);
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                row.append(&arrow);
                row.append(&icon);
                row.append(&label);
                let button = gtk::Button::new();
                button.set_child(Some(&row));
                button.set_has_frame(false);
                button.set_halign(gtk::Align::Fill);
                button.add_css_class("opm-explorer-row");

                // Un solo handler por widget (ListView recicla widgets): lee
                // la fila vigente del ListItem en el momento del click.
                let weak_item = list_item.downgrade();
                let model = model.clone();
                let store = store.clone();
                let on_open = Rc::clone(&on_open);
                button.connect_clicked(move |_| {
                    let Some(list_item) = weak_item.upgrade() else {
                        return;
                    };
                    let Some(row) = current_row(&list_item) else {
                        return;
                    };
                    if row.expandable {
                        toggle_dir(&model, &store, &row.path);
                    } else if !row.is_dir {
                        // §14.7: las carpetas enlazadas (no expandibles) no
                        // abren nada; los archivos van a `OpenTarget`.
                        on_open(row.path.clone());
                    }
                });

                // Click derecho §14.8: el nodo de la fila es el destinatario.
                let gesture = gtk::GestureClick::new();
                gesture.set_button(3);
                {
                    let weak_item = list_item.downgrade();
                    let host = host.clone();
                    let popover = popover.clone();
                    let context_target = Rc::clone(&context_target);
                    gesture.connect_pressed(move |gesture, _, x, y| {
                        let target = weak_item
                            .upgrade()
                            .and_then(|list_item| current_row(&list_item))
                            .map(|row| row.path.clone());
                        *context_target.borrow_mut() = target;
                        popup_at(&popover, &host, &gesture, x, y);
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                    });
                }
                button.add_controller(gesture);

                // P2-12: arrastre de la fila — contenido propio
                // `application/x-opm-path` (+ texto) ofrecido en `prepare`
                // (el widget reciclado resuelve su fila vigente AHÍ).
                let drag = gtk::DragSource::new();
                drag.set_actions(gdk::DragAction::MOVE);
                drag.connect_prepare({
                    let weak_item = list_item.downgrade();
                    move |_, _, _| {
                        let row = weak_item
                            .upgrade()
                            .and_then(|item| current_row(&item))?;
                        Some(path_content_provider(&row.path))
                    }
                });
                button.add_controller(drag);

                // P2-12: soltado sobre la fila — `FileList` (ficheros del
                // SO, preferido cuando el origen ofrece `text/uri-list`) o
                // el path en texto; solo las carpetas son destino.
                let drop = gtk::DropTarget::new(
                    gdk::FileList::static_type(),
                    gdk::DragAction::COPY | gdk::DragAction::MOVE,
                );
                drop.set_types(&[
                    gdk::FileList::static_type(),
                    <String as glib::types::StaticType>::static_type(),
                ]);
                drop.connect_accept({
                    let weak_item = list_item.downgrade();
                    move |_, _| {
                        weak_item
                            .upgrade()
                            .and_then(|item| current_row(&item))
                            .is_some_and(|row| row.is_dir)
                    }
                });
                drop.connect_drop({
                    let weak_item = list_item.downgrade();
                    let on_drop = Rc::clone(&on_drop);
                    let root = root.clone();
                    move |_, value, _, _| {
                        let Some(row) = weak_item
                            .upgrade()
                            .and_then(|item| current_row(&item))
                        else {
                            return false;
                        };
                        if !row.is_dir {
                            return false;
                        }
                        let target_dir = row.path.clone();
                        if value.is::<gdk::FileList>() {
                            let Ok(files) = value.get::<gdk::FileList>() else {
                                return false;
                            };
                            let sources: Vec<PathBuf> = files
                                .files()
                                .iter()
                                .filter_map(|file| file.path())
                                .collect();
                            if sources.is_empty() {
                                return false;
                            }
                            on_drop(ExplorerDrop::Import { sources }, target_dir);
                            return true;
                        }
                        if value.is::<String>() {
                            let Ok(text) = value.get::<String>() else {
                                return false;
                            };
                            let Some(action) = drop_text_action(&text, &root) else {
                                return false;
                            };
                            on_drop(action, target_dir);
                            return true;
                        }
                        false
                    }
                });
                button.add_controller(drop);
                list_item.set_child(Some(&button));
            }
        });
        factory.connect_bind(|_, object| {
            let list_item = object
                .downcast_ref::<gtk::ListItem>()
                .expect("el factory de lista recibe gtk::ListItem");
            let Some(row) = current_row(list_item) else {
                return;
            };
            let Some(button) = list_item.child().and_downcast::<gtk::Button>() else {
                return;
            };
            let content = button.child().and_downcast::<gtk::Box>().unwrap();
            let arrow = content.first_child().and_downcast::<gtk::Image>().unwrap();
            let icon = arrow.next_sibling().and_downcast::<gtk::Image>().unwrap();
            let label = icon.next_sibling().and_downcast::<gtk::Label>().unwrap();
            arrow.set_icon_name(Some(if row.expanded { "pan-down-symbolic" } else { "pan-end-symbolic" }));
            arrow.set_opacity(if row.expandable { 1.0 } else { 0.0 });
            icon.set_icon_name(Some(if row.is_dir {
                if row.is_symlink { "folder-symlink-symbolic" }
                else if row.expanded { "folder-open-symbolic" }
                else { "folder-symbolic" }
            } else if crate::imageview::is_image_path(&row.path) {
                "image-x-generic-symbolic"
            } else { "text-x-generic-symbolic" }));
            label.set_text(&row.name);
            button.update_property(&[gtk::accessible::Property::Label(&row.name)]);
            button.update_state(&[gtk::accessible::State::Expanded(
                row.expandable.then_some(row.expanded),
            )]);
            button.set_tooltip_text(Some(&row.path.display().to_string()));
            button.set_margin_start(row.depth as i32 * INDENT_PX);
        });

        let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&list));
        scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);

        // Click derecho en vacío: operaciones sobre la raíz del proyecto.
        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        {
            let host = widget.clone();
            let popover = popover.clone();
            let context_target = Rc::clone(&context_target);
            gesture.connect_pressed(move |gesture, _, x, y| {
                *context_target.borrow_mut() = None;
                popup_at(&popover, &host, &gesture, x, y);
                gesture.set_state(gtk::EventSequenceState::Claimed);
            });
        }
        scroll.add_controller(gesture);
        widget.append(&scroll);

        // Raíz del proyecto: primera lectura a demanda (§47.5).
        request_load(&model, &store, root.clone());

        Self {
            widget,
            header,
            model,
            store,
            selection,
            active_path,
            root,
        }
    }

    /// Widget raíz del panel, para insertarlo en el `Paned` de la ventana.
    pub fn widget(&self) -> gtk::Widget {
        self.widget.clone().upcast()
    }

    pub fn header(&self) -> gtk::Widget {
        self.header.clone().upcast()
    }

    pub fn set_show_all(&self, visible: bool) {
        self.set_filter(|filters| filters.show_all = visible);
    }

    pub fn set_show_hidden(&self, visible: bool) {
        self.set_filter(|filters| filters.show_hidden = visible);
    }

    fn set_filter(&self, apply: impl FnOnce(&mut ExplorerFilters)) {
        let mut model = lock(&self.model);
        let mut filters = model.filters();
        apply(&mut filters);
        model.set_filters(filters);
        rebuild(&self.store, &model);
    }

    pub fn set_active_path(&self, path: Option<&Path>) {
        if self.active_path.borrow().as_deref() == path { return; }
        *self.active_path.borrow_mut() = path.map(Path::to_path_buf);
        let index = path.and_then(|path|
            (0..self.store.n_items()).find(|&i| row_matches(&self.store, i, path)));
        self.selection.set_selected(index.unwrap_or(gtk::INVALID_LIST_POSITION));
    }

    /// Recarga el árbol tras operaciones §14.8. Conserva los filtros.
    pub fn refresh(&self) {
        self.model.lock().unwrap_or_else(|p| p.into_inner()).reset();
        rebuild(&self.store, &lock(&self.model));
        request_load(&self.model, &self.store, self.root.clone());
    }
}

/// Construye el `PopoverMenu` §14.8 con las acciones `explorer.*`.
fn build_context_menu(
    host: &gtk::Box,
    target: &Rc<RefCell<Option<PathBuf>>>,
    on_action: &Rc<dyn Fn(ExplorerAction, Option<PathBuf>)>,
) -> gtk::PopoverMenu {
    let menu = gtk::gio::Menu::new();
    let group = gtk::gio::SimpleActionGroup::new();
    // Secciones §14.8: crear/importar / gestionar / eliminar / copiar ruta.
    const SECTIONS: &[&[usize]] = &[&[0, 1, 2], &[3, 4], &[5], &[6]];
    for section in SECTIONS {
        let submenu = gtk::gio::Menu::new();
        for &entry in *section {
            let (action, label) = CONTEXT_MENU_SPEC[entry];
            submenu.append_item(&gtk::gio::MenuItem::new(
                Some(label),
                Some(&format!("explorer.{}", action_name(action))),
            ));
            let simple = gtk::gio::SimpleAction::new(action_name(action), None);
            let on_action = Rc::clone(on_action);
            let target = Rc::clone(target);
            simple.connect_activate(move |_, _| {
                on_action(action, target.borrow().clone());
            });
            group.add_action(&simple);
        }
        menu.append_section(None::<&str>, &submenu);
    }
    host.insert_action_group("explorer", Some(&group));
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(host);
    popover
}

/// Muestra el menú en las coordenadas del gesto (relativas a `host`).
fn popup_at(
    popover: &gtk::PopoverMenu,
    host: &gtk::Box,
    gesture: &gtk::GestureClick,
    x: f64,
    y: f64,
) {
    let widget = gesture
        .widget()
        .and_then(|w| w.translate_coordinates(host, x, y));
    let (x, y) = widget.unwrap_or((x, y));
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.popup();
}

/// Fila vigente ([`Row`]) del `ListItem` reciclado.
fn current_row(list_item: &gtk::ListItem) -> Option<Row> {
    let item = list_item.item().and_downcast::<glib::BoxedAnyObject>()?;
    let row = (*item.borrow::<Row>()).clone();
    Some(row)
}

fn row_matches(store: &gtk::gio::ListStore, index: u32, path: &Path) -> bool {
    store.item(index).and_downcast::<glib::BoxedAnyObject>()
        .is_some_and(|item| item.borrow::<Row>().path == path)
}

/// Expande/colapsa `dir` y pide su lectura si hace falta (§47.5).
fn toggle_dir(model: &Arc<Mutex<TreeModel>>, store: &gtk::gio::ListStore, dir: &Path) {
    let needs_load = {
        let mut model = lock(model);
        let expanded = model.toggle(dir);
        expanded && model.begin_load(dir)
    };
    rebuild(store, &lock(model));
    if needs_load {
        spawn_load(model.clone(), store, dir.to_path_buf());
    }
}

/// Pide la lectura de `dir` (§47.5): `begin_load` + hilo de lectura.
fn request_load(model: &Arc<Mutex<TreeModel>>, store: &gtk::gio::ListStore, dir: PathBuf) {
    if lock(model).begin_load(&dir) {
        spawn_load(model.clone(), store, dir);
    }
}

/// Lee `dir` en un hilo aparte (§47.1: nunca bloquea la UI ni la escritura) y
/// vuelca el resultado en el bucle principal, que es el único que toca GTK.
/// El listado lo da `opm_fs::list_tree_children` (P2-11: LA política del
/// árbol — IGNORED_NAMES, carpetas primero case-insensitive, tope 5).
fn spawn_load(model: Arc<Mutex<TreeModel>>, store: &gtk::gio::ListStore, dir: PathBuf) {
    let root = lock(&model).root().to_path_buf();
    let store_weak = glib::SendWeakRef::from(store.downgrade());
    std::thread::spawn(move || {
        let entries: Result<Vec<TreeEntry>, String> =
            list_tree_children(&root, &dir).map_err(|err| err.to_string());
        glib::idle_add_once(move || {
            let mut model = lock(&model);
            model.finish_load(&dir, entries);
            if let Some(store) = store_weak.upgrade() {
                rebuild(&store, &model);
            }
        });
    });
}

/// Re-renderiza la lista completa desde el modelo (§47.5: sin releer).
fn rebuild(store: &gtk::gio::ListStore, model: &TreeModel) {
    store.remove_all();
    for row in model.rows() {
        store.append(&glib::BoxedAnyObject::new(row));
    }
}

/// Lock sin panics en la UI: un hilo envenenado no tumba la ventana.
fn lock(model: &Mutex<TreeModel>) -> MutexGuard<'_, TreeModel> {
    model
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §14.8/§15: el menú contextual cubre todas las operaciones exigidas,
    /// `Rename` conserva el nombre de la SRS e `Import Files` (§15) usa su
    /// etiqueta literal.
    #[test]
    fn menu_contextual_14_8_completo() {
        let etiquetas: Vec<&str> = CONTEXT_MENU_SPEC.iter().map(|(_, l)| *l).collect();
        assert_eq!(
            etiquetas,
            [
                "New File",
                "New Folder",
                "Import Files",
                "Rename",
                "Move",
                "Delete",
                "Copy Relative Path",
            ]
        );
        // Una acción registrada por ítem, sin duplicados.
        let mut nombres = std::collections::HashSet::new();
        for (action, _) in CONTEXT_MENU_SPEC {
            assert!(nombres.insert(action_name(*action)), "acción duplicada");
        }
    }
}
