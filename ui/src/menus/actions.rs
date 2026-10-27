// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Comandos, barra de menú §36 y atajos §39.
//!
//! El modelo de menú vive en datos puros ([`MENUBAR_SPEC`]) para poder
//! verificar la estructura §36 sin display ni main loop de GTK;
//! [`build_menubar`] lo materializa en `gio::Menu` (GMenu es GObject puro de
//! GIO y no necesita `gtk_init`). [`install_actions`] registra las acciones
//! `win.*` y fija los aceleradores §39.
//!
//! El conjunto de comandos es el del shell: File/Project/Edit/Pipeline/
//! View/Help sobre las pestañas de texto, el explorador y el dominio de
//! proyecto (§6: comandos de proyecto/pipeline/run sin stubs).

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::gio;
use gtk4::prelude::*;

/// Comandos del shell (§36).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    New,
    Open,
    /// Abrir carpeta como raíz del explorador (ISSUE-021).
    OpenFolder,
    Save,
    SaveAs,
    CloseTab,
    Undo,
    Redo,
    /// Búsqueda en el archivo actual (§40.1, §39: `Ctrl+F`).
    Find,
    /// Reemplazo en el archivo actual (§40.1, §39: `Ctrl+H`).
    Replace,
    /// Alterna Markdown/Visual en el tab activo (solo Markdown, §5).
    ToggleMode,
    /// Muestra/oculta el explorador de archivos (§14.3, §39: `Ctrl+B`).
    ToggleExplorer,
    /// Crea un proyecto y abre su ventana (§6, Project ▸ New project).
    NewProject,
    /// Abre un proyecto existente validando `project.config.json` (§6).
    OpenProject,
    /// Importa un fichero `.yml`/`.yaml`/`.json` al proyecto (§6).
    ImportPipeline,
    /// Relee el árbol del explorador y marca los ficheros abiertos /// cambiados en disco (P2-13, §12.7).
    Refresh,
    /// Exporta el pipeline del proyecto a JSON (§6).
    ExportPipeline,
    /// Abre el diálogo de creación de paso (§6, Pipeline ▸ Create step).
    CreateStep,
    /// Ejecuta los pasos seleccionados (§6, A10: habilitado en reposo).
    RunSelected,
    /// Detiene la ejecución en curso (§6).
    Stop,
    About,
    Shortcuts,
}

impl Command {
    /// Identificador de acción GTK («win.…») con el que se registra el
    /// comando y al que apuntan menús y aceleradores (§36).
    pub fn action_id(&self) -> &'static str {
        match self {
            Command::New => "win.new",
            Command::Open => "win.open",
            Command::OpenFolder => "win.open-folder",
            Command::Save => "win.save",
            Command::SaveAs => "win.save-as",
            Command::CloseTab => "win.close-tab",
            Command::Undo => "win.undo",
            Command::Redo => "win.redo",
            Command::Find => "win.find",
            Command::Replace => "win.replace",
            Command::ToggleMode => "win.toggle-mode",
            Command::ToggleExplorer => "win.toggle-explorer",
            Command::NewProject => "win.new-project",
            Command::OpenProject => "win.open-project",
            Command::ImportPipeline => "win.import-pipeline",
            Command::Refresh => "win.refresh",
            Command::ExportPipeline => "win.export-pipeline",
            Command::CreateStep => "win.create-step",
            Command::RunSelected => "win.run-selected",
            Command::Stop => "win.stop",
            Command::About => "win.about",
            Command::Shortcuts => "win.shortcuts",
        }
    }

    /// Etiqueta de menú en inglés (§36: File, Edit, …). El ítem de atajos
    /// conserva «Atajos», el nombre con el que §36 lo lista en Help.
    pub fn label(&self) -> &'static str {
        match self {
            Command::New => "New",
            Command::Open => "Open",
            Command::OpenFolder => "Open Folder",
            Command::Save => "Save",
            Command::SaveAs => "Save As",
            Command::CloseTab => "Close Tab",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Find => "Find",
            Command::Replace => "Replace",
            Command::ToggleMode => "Markdown/Visual",
            Command::ToggleExplorer => "Toggle Explorer",
            Command::NewProject => "New project",
            Command::OpenProject => "Open project",
            Command::ImportPipeline => "Import pipeline",
            Command::Refresh => "Refresh",
            Command::ExportPipeline => "Export pipeline",
            Command::CreateStep => "Create step",
            Command::RunSelected => "Run selected",
            Command::Stop => "Stop",
            Command::About => "About",
            Command::Shortcuts => "Atajos",
        }
    }
}

/// Receptor de comandos: lo implementa el integrador (main.rs).
pub trait Commands {
    fn new(&self);
    fn open(&self);
    /// Abre una carpeta como raíz del explorador (ISSUE-021).
    fn open_folder(&self);
    fn save(&self);
    fn save_as(&self);
    fn close_tab(&self);
    fn undo(&self);
    fn redo(&self);
    /// Abre la barra de búsqueda del archivo actual (§40.1).
    fn find(&self);
    /// Abre la barra de búsqueda con la fila de reemplazo (§40.1).
    fn replace(&self);
    fn toggle_mode(&self);
    /// Muestra/oculta el explorador (§14.3).
    fn toggle_explorer(&self);
    /// Crea un proyecto (nombre + carpeta padre) y abre su ventana (§6).
    fn new_project(&self);
    /// Abre un proyecto existente; carpeta sin `project.config.json` →
    /// error claro (§6).
    fn open_project(&self);
    /// Importa un fichero de pipeline al proyecto abierto (§6).
    fn import_pipeline(&self);
    /// Refresco manual (P2-13): árbol del explorador + banderín ⚠ de los /// ficheros abiertos cambiados en disco.
    fn refresh(&self);
    /// Exporta el pipeline abierto a JSON (§6).
    fn export_pipeline(&self);
    /// Abre el diálogo de creación de paso (§6).
    fn create_step(&self);
    /// Ejecuta los pasos seleccionados — o los no `ok` si no hay selección
    /// (§4 opm-exec, A10).
    fn run_selected(&self);
    /// Detiene la ejecución en curso (§6).
    fn stop(&self);
    fn about(&self);
    fn shortcuts(&self);
}

/// Atajos §39 (identificador de acción → acelerador GTK).
pub const ACCELERATORS: &[(&str, &str)] = &[
    ("win.new", "<Control>n"),
    ("win.open", "<Control>o"),
    ("win.open-folder", "<Control><Shift>o"),
    ("win.save", "<Control>s"),
    ("win.save-as", "<Control><Shift>s"),
    ("win.close-tab", "<Control>w"),
    ("win.undo", "<Control>z"),
    ("win.redo", "<Control><Shift>z"),
    ("win.find", "<Control>f"),
    ("win.replace", "<Control>h"),
    ("win.toggle-mode", "<Control><Alt>m"),
    ("win.toggle-explorer", "<Control>b"),
];

/// Todos los comandos, en orden de aparición en los menús §36.
pub const ALL_COMMANDS: &[Command] = &[
    Command::New,
    Command::Open,
    Command::OpenFolder,
    Command::Save,
    Command::SaveAs,
    Command::CloseTab,
    Command::Undo,
    Command::Redo,
    Command::Find,
    Command::Replace,
    Command::ToggleMode,
    Command::ToggleExplorer,
    Command::NewProject,
    Command::OpenProject,
    Command::ImportPipeline,
    Command::Refresh,
    Command::ExportPipeline,
    Command::CreateStep,
    Command::RunSelected,
    Command::Stop,
    Command::About,
    Command::Shortcuts,
];

/// Ítem puro del modelo de menú §36: `(etiqueta, action_id)`.
type EntrySpec = (&'static str, &'static str);
/// Sección pura del modelo de menú §36: ítems contiguos con separador.
type SectionSpec = &'static [EntrySpec];
/// Submenú puro §36: `(título, secciones)`.
type SubmenuSpec = (&'static str, &'static [SectionSpec]);

/// Modelo de menú §36 en datos puros: File, Project, Edit, Pipeline, View y
/// Help.
const MENUBAR_SPEC: &[SubmenuSpec] = &[
    (
        "File",
        &[&[
            ("New", "win.new"),
            ("Open", "win.open"),
            ("Open Folder", "win.open-folder"),
            ("Save", "win.save"),
            ("Save As", "win.save-as"),
            ("Close Tab", "win.close-tab"),
        ]],
    ),
    (
        "Project",
        &[&[
            ("New project", "win.new-project"),
            ("Open project", "win.open-project"),
            ("Import pipeline", "win.import-pipeline"),
            ("Export pipeline", "win.export-pipeline"),
            ("Refresh", "win.refresh"),
        ]],
    ),
    (
        "Edit",
        &[&[
            ("Undo", "win.undo"),
            ("Redo", "win.redo"),
            ("Find", "win.find"),
            ("Replace", "win.replace"),
        ]],
    ),
    (
        "Pipeline",
        &[&[
            ("Create step", "win.create-step"),
            ("Run selected", "win.run-selected"),
            ("Stop", "win.stop"),
        ]],
    ),
    (
        "View",
        &[&[
            ("Markdown/Visual", "win.toggle-mode"),
            ("Toggle Explorer", "win.toggle-explorer"),
        ]],
    ),
    (
        "Help",
        &[&[("Atajos", "win.shortcuts"), ("About", "win.about")]],
    ),
];

/// Modelo de menú §36: File, Project, Edit, Pipeline, View, Help.
pub fn build_menubar() -> gio::Menu {
    let menubar = gio::Menu::new();
    for &(title, sections) in MENUBAR_SPEC {
        let submenu = gio::Menu::new();
        for entries in sections {
            let section = gio::Menu::new();
            for &(label, action_id) in *entries {
                section.append_item(&gio::MenuItem::new(Some(label), Some(action_id)));
            }
            submenu.append_section(None::<&str>, &section);
        }
        menubar.append_submenu(Some(title), &submenu);
    }
    menubar
}

/// Enruta un comando activado al receptor [`Commands`] (§3.7: toda acción de
/// la UI es explícita y observable en el host).
fn dispatch(cmd: Command, host: &dyn Commands) {
    match cmd {
        Command::New => host.new(),
        Command::Open => host.open(),
        Command::OpenFolder => host.open_folder(),
        Command::Save => host.save(),
        Command::SaveAs => host.save_as(),
        Command::CloseTab => host.close_tab(),
        Command::Undo => host.undo(),
        Command::Redo => host.redo(),
        Command::Find => host.find(),
        Command::Replace => host.replace(),
        Command::ToggleMode => host.toggle_mode(),
        Command::ToggleExplorer => host.toggle_explorer(),
        Command::NewProject => host.new_project(),
        Command::OpenProject => host.open_project(),
        Command::ImportPipeline => host.import_pipeline(),
        Command::Refresh => host.refresh(),
        Command::ExportPipeline => host.export_pipeline(),
        Command::CreateStep => host.create_step(),
        Command::RunSelected => host.run_selected(),
        Command::Stop => host.stop(),
        Command::About => host.about(),
        Command::Shortcuts => host.shortcuts(),
    }
}

/// Construye el grupo de acciones `win.*` de la ventana: cada comando se
/// registra sin parámetro en un [`gio::SimpleActionGroup`] insertado con el
/// prefijo «win» (los `action_id` §36 son nombres de acción detallados
/// `win.*`).
///
/// Aislado de la ventana para poder construirlo y verificarlo sin display
/// (patrón [`build_menubar`]); [`install_actions`] lo inserta en la ventana.
pub fn action_group(host: &Rc<dyn Commands>) -> gio::SimpleActionGroup {
    let group = gio::SimpleActionGroup::new();
    for &cmd in ALL_COMMANDS {
        let name = cmd
            .action_id()
            .strip_prefix("win.")
            .expect("los action_id usan el prefijo «win.» (§36)");
        let action = gio::SimpleAction::new(name, None);
        let host = Rc::clone(host);
        action.connect_activate(move |_, _| dispatch(cmd, &*host));
        group.add_action(&action);
    }
    group
}

/// Conecta `gio::SimpleAction`s de la ventana con `host` y fija los aceleradores.
///
/// Los aceleradores §39 se fijan con `gtk::Application::set_accels_for_action`.
/// Devuelve el grupo insertado para que `AppCmds` pueda consultar/refrescar
/// el `enabled` de acciones (p. ej. `toggle-mode` según el tipo de tab)
/// guardándolo en referencia débil: las acciones capturan `host` y una
/// retención fuerte cerraría un ciclo con `AppCmds`.
pub fn install_actions<W: IsA<gtk::Window>>(
    window: &W,
    app: &gtk::Application,
    host: Rc<dyn Commands>,
) -> gio::SimpleActionGroup {
    let group = action_group(&host);
    window
        .upcast_ref::<gtk::Window>()
        .insert_action_group("win", Some(&group));
    for &(action_id, accel) in ACCELERATORS {
        app.set_accels_for_action(action_id, &[accel]);
    }
    group
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    /// §39: los atajos exigidos están registrados y no hay aceleradores
    /// duplicados ni acciones sin registrar.
    #[test]
    fn aceleradores_39_sin_duplicados() {
        for (id, accel) in [
            ("win.new", "<Control>n"),
            ("win.open", "<Control>o"),
            ("win.open-folder", "<Control><Shift>o"),
            ("win.save", "<Control>s"),
            ("win.save-as", "<Control><Shift>s"),
            ("win.close-tab", "<Control>w"),
            ("win.undo", "<Control>z"),
            ("win.redo", "<Control><Shift>z"),
            ("win.find", "<Control>f"),
            ("win.replace", "<Control>h"),
            ("win.toggle-explorer", "<Control>b"),
        ] {
            assert!(
                ACCELERATORS.contains(&(id, accel)),
                "falta el atajo §39 {id} → {accel}"
            );
        }

        let registrados: HashSet<&str> = ALL_COMMANDS.iter().map(|c| c.action_id()).collect();
        let mut acciones = HashSet::new();
        let mut aceleradores = HashSet::new();
        for &(id, accel) in ACCELERATORS {
            assert!(registrados.contains(id), "acción sin registrar: {id}");
            assert!(
                acciones.insert(id),
                "acción duplicada en ACCELERATORS: {id}"
            );
            assert!(
                aceleradores.insert(accel),
                "acelerador duplicado en ACCELERATORS: {accel}"
            );
        }
    }

    /// §36: cada comando tiene un `action_id` único con el prefijo «win.».
    #[test]
    fn action_ids_unicos_con_prefijo_win() {
        let mut vistos = HashSet::new();
        for &cmd in ALL_COMMANDS {
            let id = cmd.action_id();
            assert!(id.starts_with("win."), "{id} sin prefijo «win.»");
            assert!(vistos.insert(id), "action_id duplicado: {id}");
        }
        assert_eq!(vistos.len(), ALL_COMMANDS.len());
    }

    /// §36: estructura exacta del menú — File, Project, Edit, Pipeline, View
    /// y Help, con los ítems de proyecto/pipeline del §6.
    #[test]
    fn estructura_menu_36() {
        let esperado: &[(&str, &[&[(&str, &str)]])] = &[
            (
                "File",
                &[&[
                    ("New", "win.new"),
                    ("Open", "win.open"),
                    ("Open Folder", "win.open-folder"),
                    ("Save", "win.save"),
                    ("Save As", "win.save-as"),
                    ("Close Tab", "win.close-tab"),
                ]],
            ),
            (
                "Project",
                &[&[
                    ("New project", "win.new-project"),
                    ("Open project", "win.open-project"),
                    ("Import pipeline", "win.import-pipeline"),
                    ("Export pipeline", "win.export-pipeline"),
                    ("Refresh", "win.refresh"),
                ]],
            ),
            (
                "Edit",
                &[&[
                    ("Undo", "win.undo"),
                    ("Redo", "win.redo"),
                    ("Find", "win.find"),
                    ("Replace", "win.replace"),
                ]],
            ),
            (
                "Pipeline",
                &[&[
                    ("Create step", "win.create-step"),
                    ("Run selected", "win.run-selected"),
                    ("Stop", "win.stop"),
                ]],
            ),
            (
                "View",
                &[&[
                    ("Markdown/Visual", "win.toggle-mode"),
                    ("Toggle Explorer", "win.toggle-explorer"),
                ]],
            ),
            (
                "Help",
                &[&[("Atajos", "win.shortcuts"), ("About", "win.about")]],
            ),
        ];
        assert_eq!(MENUBAR_SPEC, esperado);
    }

    /// `build_menubar()` materializa [`MENUBAR_SPEC`] en `gio::Menu` con los
    /// mismos títulos, secciones, ítems y `action_id` (GMenu no necesita
    /// display; la introspección de `gio::MenuModel` tampoco).
    #[test]
    fn build_menubar_materializa_estructura_36() {
        let menubar = build_menubar();
        assert_eq!(menubar.n_items(), MENUBAR_SPEC.len() as i32);
        for (i, &(title, sections)) in MENUBAR_SPEC.iter().enumerate() {
            let i = i as i32;
            let titulo = menubar
                .item_attribute_value(i, "label", None)
                .expect("título del submenú");
            assert_eq!(titulo.str(), Some(title));
            let submenu = menubar.item_link(i, "submenu").expect("submenú");
            assert_eq!(submenu.n_items(), sections.len() as i32);
            for (j, entries) in sections.iter().enumerate() {
                let j = j as i32;
                let section = submenu.item_link(j, "section").expect("sección");
                assert_eq!(section.n_items(), entries.len() as i32);
                for (k, &(label, action_id)) in entries.iter().enumerate() {
                    let k = k as i32;
                    let etiqueta = section
                        .item_attribute_value(k, "label", None)
                        .expect("etiqueta del ítem");
                    let accion = section
                        .item_attribute_value(k, "action", None)
                        .expect("acción del ítem");
                    assert_eq!(etiqueta.str(), Some(label));
                    assert_eq!(accion.str(), Some(action_id));
                }
            }
        }
    }

    /// Coherencia §36/§37: todo ítem del menú corresponde a un comando
    /// registrado con su etiqueta y todo comando es alcanzable desde el menú.
    #[test]
    fn menu_y_comandos_coherentes() {
        let mut en_menu = HashSet::new();
        for &(_, sections) in MENUBAR_SPEC {
            for entries in sections {
                for &(label, action_id) in *entries {
                    let cmd = ALL_COMMANDS
                        .iter()
                        .find(|c| c.action_id() == action_id)
                        .unwrap_or_else(|| panic!("ítem sin comando: {action_id}"));
                    assert_eq!(cmd.label(), label, "etiqueta desalineada en {action_id}");
                    en_menu.insert(action_id);
                }
            }
        }
        for &cmd in ALL_COMMANDS {
            assert!(
                en_menu.contains(cmd.action_id()),
                "comando sin ítem de menú §36: {}",
                cmd.action_id()
            );
        }
    }

    /// Gramática de aceleradores de GTK: grupos `<Modificador>` seguidos de
    /// una tecla. Un literal inválido lo ignora GTK en silencio (el atajo
    /// queda muerto), así que la tabla §39 se valida contra la gramática.
    fn accel_es_valido(accel: &str) -> bool {
        let mut rest = accel;
        while let Some(start) = rest.find('<') {
            if start != 0 {
                return false; // texto suelto antes de un grupo
            }
            let Some(end) = rest.find('>') else {
                return false; // grupo sin cerrar
            };
            let modifier = &rest[1..end];
            if !matches!(
                modifier,
                "Control" | "Shift" | "Alt" | "Meta" | "Super" | "Primary"
            ) {
                return false;
            }
            rest = &rest[end + 1..];
        }
        // Tecla final: sin restos de grupo ni espacios.
        !rest.is_empty() && !rest.contains('>') && !rest.contains('<') && !rest.contains(' ')
    }

    #[test]
    fn aceleradores_siguen_la_gramatica_de_gtk() {
        for (action, accel) in ACCELERATORS {
            assert!(
                accel_es_valido(accel),
                "acelerador {accel:?} inválido para {action} (§39)"
            );
        }
        // El validador no es vacío: conoce los fallos reales.
        for malo in [
            "<Control>Alt>m",
            "<Controll>n",
            "<Control>",
            "Control>n",
            "",
        ] {
            assert!(!accel_es_valido(malo), "{malo:?} no debería validar");
        }
    }
}
