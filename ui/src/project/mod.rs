//! Flujos de librería y trabajo (pipeline-storage §6): abrir/crear
//! trabajos, navegar la librería de pipelines y restaurar la sesión.
//!
//! - [`library_root`] / [`library`]: raíz de la librería `.pipeline`
//!   (decisión de almacenamiento #2): `OPM_PIPELINES_DIR` si está fijado,
//!   si no `<data_dir>/opm/pipelines` (mismo precedente `directories` que
//!   [`crate::appstate::state_file`]), si no el directorio temporal. NUNCA
//!   depende del cwd de lanzamiento ni de la carpeta de trabajo abierta, y
//!   la ubicación se muestra en la UI (diálogo de la librería y toolbar de
//!   diseño).
//! - [`pick_library`]: navegador de nombres de la librería (con la raíz
//!   visible) para Open pipeline / New work.
//! - [`pick_work`]: selector de carpeta de trabajo que exige
//!   `.pipeline-manager/run.json` y deja el error claro si no lo hay.
//! - [`restore`]/[`restore_from`]: sesión con TTL de 1 h sobre el hueco
//!   `last_work` de [`crate::appstate`] (§11 A13). Caducada → se limpia en
//!   silencio (paridad SPA); carpeta ausente/inválida → se limpia Y se
//!   devuelve el error para mostrarlo.
//!
//! La lógica pura (TTL, restore, raíz de librería) vive separada de los
//! diálogos GTK para poder testearla sin display.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;

use opm_store::PipelineLibrary;

use crate::appstate;
use crate::menus::dialogs;
use crate::tabs::WindowScope;

/// TTL de la sesión de trabajo: 1 hora en segundos (paridad con el
/// `SESSION_TTL_MS` del SPA, `App.tsx`).
pub const SESSION_TTL_SECS: i64 = 60 * 60;

/// Resultado de la restauración de sesión al arrancar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// Sin sesión guardada o caducada (ya limpiada).
    None,
    /// Sesión válida: carpeta de trabajo lista para abrir.
    Work(PathBuf),
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

/// Timestamp unix actual (§11 A13: sello de la sesión al abrir una carpeta
/// de trabajo y control de TTL al restaurar).
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// [`restore_from`] sobre el fichero de estado real del SO (§11 A13): lee
/// el hueco con [`appstate::load_last_work`] y purga lo caducado.
pub fn restore() -> Restore {
    let now = now_secs();
    restore_loaded(&appstate::state_file(), appstate::load_last_work(), now)
}

/// [`restore`] sobre una ruta explícita (solo pruebas: la lógica pura del
/// TTL y del purgado sin tocar el fichero real del SO).
#[cfg(test)]
pub fn restore_from(state_path: &Path, now: i64) -> Restore {
    restore_loaded(state_path, appstate::load_last_work_from(state_path), now)
}

/// Restauración pura sobre el hueco ya leído: caducada → limpia en silencio
/// (paridad SPA); carpeta sin `run.json` o corrupta → limpia Y devuelve el
/// error (el snapshot de `run.json` es la única autoridad: no hay marcador
/// de proyecto ni librería disponible).
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
        let _ = appstate::save_last_work_to(state_path, &None);
        return Restore::None;
    }
    let root = PathBuf::from(path);
    let probe = opm_store::ProjectStore::new();
    match probe.open_work(&root) {
        Ok(()) => {
            let canonical = opm_fs::canonicalize(&root).unwrap_or(root);
            Restore::Work(canonical)
        }
        Err(err) => {
            let _ = appstate::save_last_work_to(state_path, &None);
            Restore::Failed(err.to_string())
        }
    }
}

/// Raíz de la librería de pipelines (decisión #2, ver módulo).
pub fn library_root() -> PathBuf {
    library_root_from(
        std::env::var_os("OPM_PIPELINES_DIR").map(PathBuf::from),
        directories::BaseDirs::new().map(|dirs| dirs.data_dir().to_path_buf()),
        std::env::temp_dir(),
    )
}

/// [`library_root`] como función pura: `env_dir` (OPM_PIPELINES_DIR) si está
/// fijado y no vacío, si no `<data_dir>/opm/pipelines`, si no
/// `<temp>/opm/pipelines`. El cwd de lanzamiento no participa.
pub fn library_root_from(
    env_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    temp_dir: PathBuf,
) -> PathBuf {
    if let Some(dir) = env_dir.filter(|dir| !dir.as_os_str().is_empty()) {
        return dir;
    }
    match data_dir {
        Some(base) => base.join("opm").join("pipelines"),
        None => temp_dir.join("opm").join("pipelines"),
    }
}

/// La librería de pipelines sobre la raíz resuelta.
pub fn library() -> PipelineLibrary {
    PipelineLibrary::new(library_root())
}

/// `source.name` del `run.json` de una carpeta de trabajo — el documento de
/// librería que «Update from library» debe volver a adoptar. `None` si el
/// documento no es legible o no declara fuente (entonces el selector de
/// fichero hace de fallback).
pub fn work_source_name(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(".pipeline-manager").join("run.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let name = value.get("source")?.get("name")?.as_str()?.to_owned();
    Some(name)
}

/// Ámbito para una carpeta elegida (Open Folder, argumento CLI): con
/// `.pipeline-manager/run.json` → ventana de trabajo; sin él → ventana
/// exploradora simple que NO toca nada.
pub fn scope_for_folder(root: PathBuf) -> WindowScope {
    if root.join(".pipeline-manager").join("run.json").exists() {
        WindowScope::Work(root)
    } else {
        WindowScope::Folder(root)
    }
}

/// Selector de carpeta de TRABAJO (Project ▸ Open work): exige
/// `.pipeline-manager/run.json`; sin él hay error claro y `None` (nunca una
/// ventana degradada).
pub async fn pick_work(parent: &impl IsA<gtk::Window>) -> Option<PathBuf> {
    let folder = dialogs::open_folder(parent).await?;
    let probe = opm_store::ProjectStore::new();
    match probe.open_work(&folder) {
        Ok(()) => Some(folder),
        Err(err) => {
            crate::show_error(parent.upcast_ref::<gtk::Window>(), &err.to_string());
            None
        }
    }
}

/// Navegador de la librería de pipelines (Open pipeline / New work): lista
/// los nombres de `<root>/*.pipeline` con la raíz de la librería VISIBLE y
/// entrega el elegido.
///
/// `on_pick(Some(nombre))` al aceptar (botón o doble clic), `on_pick(None)`
/// al cancelar. Callback y no futuro: los flujos encadenan diálogos con
/// estado propio (ISSUE-023, patrón [`dialogs::prompt_text`]).
pub fn pick_library(
    parent: &impl IsA<gtk::Window>,
    on_pick: Rc<dyn Fn(Option<String>)>,
) {
    let root = library_root();
    let names = PipelineLibrary::new(&root).list();

    let window = gtk::Window::builder()
        .title("Open pipeline")
        .modal(true)
        .transient_for(parent)
        .default_width(460)
        .default_height(460)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);

    let location = gtk::Label::new(Some(&format!("Library: {}", root.display())));
    location.set_halign(gtk::Align::Start);
    location.set_xalign(0.0);
    location.set_wrap(true);
    location.add_css_class("opm-dim");
    location.set_tooltip_text(Some(&root.display().to_string()));
    vbox.append(&location);

    // Selección única + botones: la sensibilidad de Open nace de la celda.
    let selection: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let accept = gtk::Button::with_label("Open");
    accept.add_css_class("suggested-action");
    accept.set_sensitive(false);
    buttons.append(&cancel);
    buttons.append(&accept);

    if names.is_empty() {
        let empty = gtk::Label::new(Some(&format!(
            "No pipelines saved in {} yet.\nUse Project ▸ New pipeline and Save.",
            root.display()
        )));
        empty.set_halign(gtk::Align::Start);
        empty.set_xalign(0.0);
        empty.set_wrap(true);
        empty.add_css_class("opm-empty");
        vbox.append(&empty);
    } else {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        for name in &names {
            let row = gtk::ListBoxRow::new();
            let label = gtk::Label::new(Some(&format!("{name}.pipeline")));
            label.set_halign(gtk::Align::Start);
            label.set_margin_start(6);
            label.set_margin_end(6);
            label.set_margin_top(4);
            label.set_margin_bottom(4);
            label.add_css_class("opm-mono");
            row.set_child(Some(&label));
            list.append(&row);
        }
        {
            let names = names.clone();
            let selection = Rc::clone(&selection);
            let accept = accept.clone();
            list.connect_row_selected(move |_, row| {
                let picked = row
                    .and_then(|row| names.get(row.index() as usize))
                    .cloned();
                accept.set_sensitive(picked.is_some());
                *selection.borrow_mut() = picked;
            });
        }
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_child(Some(&list));
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        vbox.append(&scrolled);

        // Doble clic / Enter en una fila = abrir directamente.
        {
            let window = window.clone();
            let names = names.clone();
            let on_pick = Rc::clone(&on_pick);
            list.connect_row_activated(move |_, row| {
                if let Some(name) = names.get(row.index() as usize).cloned() {
                    window.destroy();
                    on_pick(Some(name));
                }
            });
        }
    }

    vbox.append(&buttons);
    window.set_default_widget(Some(&accept));
    window.set_child(Some(&vbox));

    {
        let window = window.clone();
        let selection = Rc::clone(&selection);
        let on_pick = Rc::clone(&on_pick);
        accept.connect_clicked(move |_| {
            let picked = selection.borrow().clone();
            window.destroy();
            on_pick(picked);
        });
    }
    {
        let window = window.clone();
        let on_pick = Rc::clone(&on_pick);
        cancel.connect_clicked(move |_| {
            window.destroy();
            on_pick(None);
        });
    }
    {
        let on_pick = Rc::clone(&on_pick);
        window.connect_close_request(move |_| {
            on_pick(None);
            glib::Propagation::Proceed
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
        let stale = Some(("/trabajo".to_owned(), 1_000_i64));
        appstate::save_last_work_to(&path, &stale).unwrap();
        assert_eq!(
            restore_from(&path, 1_000 + SESSION_TTL_SECS + 1),
            Restore::None
        );
        // Caducada ⇒ hueco limpio.
        assert_eq!(appstate::load_last_work_from(&path), None);
        let _ = std::fs::remove_file(&path);
    }

    /// Sesión fresca apuntando a una carpeta SIN `run.json` →
    /// `Restore::Failed` con el mensaje que nombra el documento y el hueco
    /// limpio (semántica `run.json`, no `project.config.json`).
    #[test]
    fn restore_carpeta_sin_run_json_limpia_y_reporta() {
        let state = temp_path("estado");
        let folder = temp_path("carpeta");
        std::fs::create_dir_all(&folder).unwrap();
        let slot = Some((folder.display().to_string(), 1_000_i64));
        appstate::save_last_work_to(&state, &slot).unwrap();

        let now = 1_000 + 60;
        match restore_from(&state, now) {
            Restore::Failed(message) => {
                assert!(
                    message.contains("run.json"),
                    "mensaje claro que nombra run.json: {message}"
                );
                assert!(
                    !message.contains("project.config.json"),
                    "sin referencia al marcador eliminado: {message}"
                );
            }
            other => panic!("esperaba Failed, llegó {other:?}"),
        }
        assert_eq!(appstate::load_last_work_from(&state), None);
        let _ = std::fs::remove_file(&state);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Sesión fresca y válida (carpeta con `run.json` creado por
    /// `create_work`) → `Restore::Work` con la raíz canónica; el hueco sigue
    /// vivo (paridad SPA: re-guarda al abrir).
    #[test]
    fn restore_trabajo_valido_abre() {
        let state = temp_path("estado-ok");
        let folder = temp_path("trabajo-ok");
        let store = opm_store::ProjectStore::new();
        let doc = opm_store::PipelineDoc::new(
            "demo",
            opm_pipeline::PipelineConfig::default(),
        );
        store.create_work(&folder, &doc).unwrap();

        let slot = Some((folder.display().to_string(), 1_000_i64));
        appstate::save_last_work_to(&state, &slot).unwrap();

        let root = opm_fs::canonicalize(&folder).unwrap();
        assert_eq!(restore_from(&state, 1_000 + 60), Restore::Work(root));
        // La sesión sigue viva (paridad SPA: re-guarda al abrir).
        assert_eq!(appstate::load_last_work_from(&state), slot);
        let _ = std::fs::remove_file(&state);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Raíz de la librería: OPM_PIPELINES_DIR manda; sin ella el data dir
    /// del sistema; sin data dir, el temporal. Nunca el cwd.
    #[test]
    fn raiz_de_libreria_con_precedencia_declarada() {
        let env = PathBuf::from("/fijo/pipelines");
        let data = PathBuf::from("/datos");
        let temp = PathBuf::from("/tmp");

        assert_eq!(
            library_root_from(Some(env.clone()), Some(data.clone()), temp.clone()),
            env,
            "OPM_PIPELINES_DIR manda"
        );
        assert_eq!(
            library_root_from(None, Some(data.clone()), temp.clone()),
            PathBuf::from("/datos/opm/pipelines"),
            "data_dir del sistema (precedente `directories` de state_file)"
        );
        assert_eq!(
            library_root_from(None, None, temp.clone()),
            PathBuf::from("/tmp/opm/pipelines"),
            "degradación al temporal"
        );
        // Un OPM_PIPELINES_DIR vacío no cuenta.
        assert_eq!(
            library_root_from(Some(PathBuf::new()), Some(data), temp),
            PathBuf::from("/datos/opm/pipelines")
        );
    }

    /// Ámbito de carpeta: con `run.json` → trabajo; sin él → explorador.
    #[test]
    fn carpeta_con_run_json_es_trabajo_sin_el_explorador() {
        let folder = temp_path("scope");
        std::fs::create_dir_all(folder.join(".pipeline-manager")).unwrap();

        assert!(matches!(
            scope_for_folder(folder.clone()),
            WindowScope::Folder(_)
        ));
        std::fs::write(folder.join(".pipeline-manager/run.json"), "{}").unwrap();
        assert!(matches!(
            scope_for_folder(folder.clone()),
            WindowScope::Work(_)
        ));
        let _ = std::fs::remove_dir_all(&folder);
    }
}
