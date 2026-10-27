//! Flujos de proyecto (§6): crear, abrir, importar/exportar pipeline y
//! restauración de sesión con TTL de 1 h (paridad con el SPA).
//!
//! - [`create_dialog`]: diálogo modal con nombre + preview saneado
//!   ([`opm_fs::safe_segment`]) y carpeta padre navegable; al aceptar crea
//!   el scaffold con `opm_fs::ensure_project_scaffold` y entrega la raíz al
//!   integrador (que abre la ventana de proyecto con `store.open_project`).
//! - [`pick_project`]: selector de carpeta que valida `project.config.json`
//!   y muestra el error claro del §6 si la carpeta no es un proyecto.
//! - [`restore`]/[`restore_from`]: sesión con TTL de 1 h sobre el hueco de
//!   `crate::appstate` (§11 A13). Caducada → se limpia en silencio (paridad
//!   SPA); proyecto ausente/inválido → se limpia Y se devuelve el error para
//!   mostrarlo.
//!
//! La lógica pura (TTL, restore) vive separada de los diálogos GTK para
//! poder testearla sin display.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;

use crate::appstate;

/// TTL de la sesión de proyecto: 1 hora en segundos (paridad con el
/// `SESSION_TTL_MS` del SPA, `App.tsx`).
pub const SESSION_TTL_SECS: i64 = 60 * 60;

/// Resultado de la restauración de sesión al arrancar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// Sin sesión guardada o caducada (ya limpiada).
    None,
    /// Sesión válida: proyecto listo para abrir.
    Open(PathBuf),
    /// Sesión apuntaba a algo inválido: limpiada y con el error a mostrar.
    Failed(String),
}

/// ¿La sesión guardada en `created_at` sigue vigente en `now` (unix secs)?
///
/// Paridad con `loadStoredSession` del SPA: caduca cuando
/// `created_at + TTL < now`; un `now` anterior al guardado (cambio de reloj)
/// la mantiene viva.
pub fn session_fresh(created_at: i64, now: i64) -> bool {
    created_at.saturating_add(SESSION_TTL_SECS) >= now
}

/// Timestamp unix actual (§11 A13: sello de la sesión al abrir un proyecto
/// y control de TTL al restaurar).
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// [`restore_from`] sobre el fichero de estado real del SO (§11 A13): lee
/// el hueco con [`appstate::load_last_project`] y purga lo caducado.
pub fn restore() -> Restore {
    let now = now_secs();
    restore_loaded(&appstate::state_file(), appstate::load_last_project(), now)
}

/// [`restore`] sobre una ruta explícita (solo pruebas: la lógica pura del
/// TTL y del purgado sin tocar el fichero real del SO).
#[cfg(test)]
pub fn restore_from(state_path: &Path, now: i64) -> Restore {
    restore_loaded(
        state_path,
        appstate::load_last_project_from(state_path),
        now,
    )
}

/// Restauración pura sobre el hueco ya leído: caducada → limpia en silencio
/// (paridad SPA); proyecto ausente/inválido → limpia Y devuelve el error.
fn restore_loaded(
    state_path: &Path,
    loaded: Option<(String, i64)>,
    now: i64,
) -> Restore {
    let Some((path, created_at)) = loaded else {
        return Restore::None;
    };
    if !session_fresh(created_at, now) {
        // El SPA borra la sesión caducada sin molestar.
        let _ = appstate::save_last_project_to(state_path, &None);
        return Restore::None;
    }
    let root = PathBuf::from(path);
    if let Err(err) = opm_fs::validate_project_folder(&root) {
        let _ = appstate::save_last_project_to(state_path, &None);
        return Restore::Failed(err.to_string());
    }
    // Paridad SPA: `openProject` puede fallar por estado interno corrupto,
    // no solo por la carpeta; en ese caso también se limpia y se muestra.
    let probe = opm_store::ProjectStore::new();
    match probe.open_project(&root) {
        Ok(()) => Restore::Open(root),
        Err(err) => {
            let _ = appstate::save_last_project_to(state_path, &None);
            Restore::Failed(err.to_string())
        }
    }
}

/// Carpeta inicial de los diálogos de proyecto: directorio personal si se
/// conoce, si no el directorio de trabajo.
pub fn default_parent() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Selector de carpeta de proyecto (§6, Project ▸ Open project / Open
/// Folder): `None` al cancelar; una carpeta sin `project.config.json` →
/// error claro y `None` (nunca una ventana degradada).
pub async fn pick_project(parent: &impl IsA<gtk::Window>) -> Option<PathBuf> {
    let folder = crate::menus::dialogs::open_folder(parent).await?;
    match opm_fs::validate_project_folder(&folder) {
        Ok(()) => Some(folder),
        Err(err) => {
            crate::show_error(parent.upcast_ref::<gtk::Window>(), &err.to_string());
            None
        }
    }
}

/// Diálogo «Create Project» (§6): nombre con preview saneado + carpeta
/// padre navegable.
///
/// `on_created(root)` recibe la raíz canonicalizada del proyecto recién
/// creado (el scaffold ya está en disco); el integrador abre su ventana.
pub fn create_dialog(
    parent: &impl IsA<gtk::Window>,
    on_created: Rc<dyn Fn(PathBuf)>,
) {
    let parent_window = parent.upcast_ref::<gtk::Window>().clone();

    let window = gtk::Window::builder()
        .title("Create Project")
        .modal(true)
        .transient_for(&parent_window)
        .default_width(460)
        .resizable(false)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let title = gtk::Label::new(Some("Create Project"));
    title.set_halign(gtk::Align::Start);
    title.add_css_class("title-2");
    vbox.append(&title);

    // Nombre + preview saneado (paridad con el `DirectoryPicker` del SPA).
    let name_grid = gtk::Grid::new();
    name_grid.set_row_spacing(4);
    name_grid.set_column_spacing(8);
    let name_label = gtk::Label::new(Some("Project name"));
    name_label.set_halign(gtk::Align::Start);
    let name_entry = gtk::Entry::new();
    name_entry.set_placeholder_text(Some("my-pipeline-project"));
    name_entry.set_hexpand(true);
    name_entry.set_activates_default(true);
    name_grid.attach(&name_label, 0, 0, 1, 1);
    name_grid.attach(&name_entry, 1, 0, 1, 1);
    let preview = gtk::Label::new(None);
    preview.set_halign(gtk::Align::Start);
    preview.add_css_class("opm-dim");
    name_grid.attach(&preview, 1, 1, 1, 1);
    vbox.append(&name_grid);

    // Carpeta padre navegable.
    let parent_grid = gtk::Grid::new();
    parent_grid.set_row_spacing(4);
    parent_grid.set_column_spacing(8);
    let parent_label = gtk::Label::new(Some("Parent folder"));
    parent_label.set_halign(gtk::Align::Start);
    let parent_entry = gtk::Entry::new();
    parent_entry.set_text(&default_parent().display().to_string());
    parent_entry.set_hexpand(true);
    let browse = gtk::Button::with_label("Browse…");
    parent_grid.attach(&parent_label, 0, 0, 1, 1);
    parent_grid.attach(&parent_entry, 1, 0, 1, 1);
    parent_grid.attach(&browse, 2, 0, 1, 1);
    vbox.append(&parent_grid);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let accept = gtk::Button::with_label("Create project");
    accept.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&accept);
    vbox.append(&buttons);

    window.set_default_widget(Some(&accept));
    window.set_child(Some(&vbox));

    // Preview del nombre saneado mientras se teclea.
    {
        let preview = preview.clone();
        name_entry.connect_changed(move |entry| {
            let name = entry.text().to_string();
            if name.trim().is_empty() {
                preview.set_text("");
            } else {
                preview.set_text(&format!("Folder: {}", opm_fs::safe_segment(&name)));
            }
        });
    }

    // Browse: selector de carpeta nativo que rellena la entrada del padre.
    {
        let window_for_dialog = window.clone();
        let parent_entry = parent_entry.clone();
        browse.connect_clicked(move |_| {
            let entry = parent_entry.clone();
            let window = window_for_dialog.clone();
            gtk::glib::spawn_future_local(async move {
                let dialog = gtk::FileDialog::new();
                dialog.set_title("Parent folder");
                dialog.set_modal(true);
                let current = PathBuf::from(entry.text().to_string());
                if current.is_dir() {
                    dialog.set_initial_folder(Some(&gtk::gio::File::for_path(current)));
                }
                if let Ok(folder) = dialog.select_folder_future(Some(&window)).await {
                    if let Some(path) = folder.path() {
                        entry.set_text(&path.display().to_string());
                    }
                }
            });
        });
    }

    // Cancelar / cerrar.
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.destroy());
    }
    window.connect_close_request({
        let window = window.clone();
        move |_| {
            window.destroy();
            gtk::glib::Propagation::Proceed
        }
    });

    // Crear: scaffold + entrega de la raíz al integrador.
    {
        let window = window.clone();
        let name_entry = name_entry.clone();
        let parent_entry = parent_entry.clone();
        let on_created = Rc::clone(&on_created);
        accept.connect_clicked(move |_| {
            let parent_text = parent_entry.text().to_string();
            let parent_path = PathBuf::from(parent_text.trim());
            let parent_canonical = match opm_fs::canonicalize(&parent_path) {
                Ok(path) => path,
                Err(err) => {
                    crate::show_error(
                        &window,
                        &format!("Parent folder is not usable: {err}"),
                    );
                    return;
                }
            };
            let name = name_entry.text().to_string();
            let safe_name = opm_fs::safe_segment(&name);
            let root = parent_canonical.join(&safe_name);
            if let Err(err) = opm_fs::ensure_project_scaffold(&root, &safe_name) {
                crate::show_error(&window, &err.to_string());
                return;
            }
            let root = opm_fs::canonicalize(&root).unwrap_or(root);
            window.destroy();
            on_created(root);
        });
    }

    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "opm-project-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    /// TTL de 1 h: vigente hasta `created_at + 3600` inclusive; más allá,
    /// caducada (paridad con el `expiresAt` del SPA).
    #[test]
    fn sesion_con_ttl_de_una_hora() {
        let base = 1_760_000_000_i64;
        assert!(session_fresh(base, base));
        assert!(session_fresh(base, base + SESSION_TTL_SECS));
        assert!(!session_fresh(base, base + SESSION_TTL_SECS + 1));
        // Reloj atrasado: la sesión se conserva (paridad SPA).
        assert!(session_fresh(base, base - 60));
    }

    /// Sin fichero → `Restore::None`; caducada → se limpia y `None`.
    #[test]
    fn restore_sin_sesion_o_caducada() {
        let missing = temp_path("missing");
        assert_eq!(restore_from(&missing, 1_000), Restore::None);

        let path = temp_path("caducada");
        let stale = Some(("/proy".to_owned(), 1_000_i64));
        appstate::save_last_project_to(&path, &stale).unwrap();
        assert_eq!(restore_from(&path, 1_000 + SESSION_TTL_SECS + 1), Restore::None);
        // Caducada ⇒ hueco limpio.
        assert_eq!(appstate::load_last_project_from(&path), None);
        let _ = std::fs::remove_file(&path);
    }

    /// Sesión fresca apuntando a una carpeta que no es proyecto →
    /// `Restore::Failed` con el mensaje del §6 y el hueco limpio.
    #[test]
    fn restore_proyecto_invalido_limpia_y_reporta() {
        let state = temp_path("estado");
        let folder = temp_path("carpeta");
        std::fs::create_dir_all(&folder).unwrap();
        let slot = Some((folder.display().to_string(), 1_000_i64));
        appstate::save_last_project_to(&state, &slot).unwrap();

        let now = 1_000 + 60;
        match restore_from(&state, now) {
            Restore::Failed(message) => {
                assert!(
                    message.contains("project.config.json"),
                    "mensaje claro del §6: {message}"
                );
            }
            other => panic!("esperaba Failed, llegó {other:?}"),
        }
        assert_eq!(appstate::load_last_project_from(&state), None);
        let _ = std::fs::remove_file(&state);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Sesión fresca y válida (carpeta con `project.config.json`) →
    /// `Restore::Open` con la raíz.
    #[test]
    fn restore_proyecto_valido_abre() {
        let state = temp_path("estado-ok");
        let folder = temp_path("proyecto-ok");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(opm_fs::PROJECT_CONFIG_FILENAME),
            r#"{"name":"ok","version":1}"#,
        )
        .unwrap();
        let slot = Some((folder.display().to_string(), 1_000_i64));
        appstate::save_last_project_to(&state, &slot).unwrap();

        let root = opm_fs::canonicalize(&folder).unwrap();
        assert_eq!(restore_from(&state, 1_000 + 60), Restore::Open(root));
        // La sesión sigue viva (paridad SPA: re-guarda al abrir).
        assert_eq!(appstate::load_last_project_from(&state), slot);
        let _ = std::fs::remove_file(&state);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// `default_parent` devuelve una ruta utilizable.
    #[test]
    fn carpeta_padre_por_defecto() {
        let parent = default_parent();
        assert!(!parent.as_os_str().is_empty());
    }
}
