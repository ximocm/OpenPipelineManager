// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Diálogos GTK4 asíncronos de OPM (ISSUE-020).
//!
//! - [`open_file`] / [`save_file_as`]: `gtk::FileDialog` con filtros por tipo
//!   (texto, Markdown, YAML, JSON, shell, FASTA e imagen); el de guardar
//!   sugiere nombre (§12.4 Save As).
//! - [`open_folder`]: carpeta para la raíz del explorador (ISSUE-021).
//! - [`confirm_discard`]: confirmación explícita antes de descartar cambios
//!   sin guardar (§3.7: nada destructivo es silencioso).
//! - [`confirm_overwrite_external`]: elección explícita ante un archivo
//!   cambiado en disco — recargar (solo con buffer limpio), sobrescribir o
//!   cancelar (§12.7).
//! - [`confirm_trash`]: confirmación de borrado con aviso de recursividad
//!   (§14.9: un directorio se elimina con todo su contenido).
//!
//! Todos devuelven futuros vía la API real de gtk4 0.9 (`open_future`,
//! `save_future` y `choose_future` de `gtk::FileDialog` / `gtk::AlertDialog`,
//! GTK ≥ 4.10); sin main loop bloqueante y sin fetch de red (§3.4).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;

/// Filtro de texto plano y configuración (*.txt, *.cfg, *.ini, *.tsv,
/// *.csv + MIME `text/plain`) para los diálogos.
fn text_filter() -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Text files (*.txt, *.cfg, *.ini, *.tsv, *.csv)"));
    for pattern in ["*.txt", "*.cfg", "*.ini", "*.tsv", "*.csv"] {
        filter.add_pattern(pattern);
    }
    filter.add_mime_type("text/plain");
    filter
}

/// Filtro Markdown (*.md / *.markdown + MIME `text/markdown`).
fn markdown_filter() -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Markdown (*.md, *.markdown)"));
    filter.add_pattern("*.md");
    filter.add_pattern("*.markdown");
    filter.add_mime_type("text/markdown");
    filter
}

/// Filtro YAML (*.yml / *.yaml): pipelines y configuración.
fn yaml_filter() -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("YAML (*.yml, *.yaml)"));
    filter.add_pattern("*.yml");
    filter.add_pattern("*.yaml");
    filter
}

/// Filtro JSON (*.json).
fn json_filter() -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("JSON (*.json)"));
    filter.add_pattern("*.json");
    filter.add_mime_type("application/json");
    filter
}

/// Filtro de scripts shell (*.sh / *.bash).
fn shell_filter() -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Shell (*.sh, *.bash)"));
    filter.add_pattern("*.sh");
    filter.add_pattern("*.bash");
    filter.add_mime_type("application/x-sh");
    filter
}

/// Filtro FASTA (*.fa, *.fasta, *.fna, *.ffn, *.faa).
fn fasta_filter() -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("FASTA (*.fa, *.fasta, *.fna, *.ffn, *.faa)"));
    for pattern in ["*.fa", "*.fasta", "*.fna", "*.ffn", "*.faa"] {
        filter.add_pattern(pattern);
    }
    filter
}

/// Filtro de imágenes (PNG, JPG/JPEG, SVG, WebP, GIF).
fn image_filter() -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Images (*.png, *.jpg, *.jpeg, *.svg, *.webp, *.gif)"));
    for pattern in ["*.png", "*.jpg", "*.jpeg", "*.svg", "*.webp", "*.gif"] {
        filter.add_pattern(pattern);
    }
    for mime in ["image/png", "image/jpeg", "image/svg+xml", "image/webp", "image/gif"] {
        filter.add_mime_type(mime);
    }
    filter
}

/// Los tipos documentales del explorador (§14.4), en orden de filtro.
fn document_filters() -> gio::ListStore {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    for filter in [
        text_filter(),
        markdown_filter(),
        yaml_filter(),
        json_filter(),
        shell_filter(),
        fasta_filter(),
        image_filter(),
    ] {
        filters.append(&filter);
    }
    filters
}

/// Diálogo de apertura (§36, File ▸ Open): filtra los tipos documentales con
/// «Todos los archivos» como respaldo. Devuelve el path elegido o `None` si
/// se cancela.
pub async fn open_file(parent: &impl IsA<gtk::Window>) -> Option<PathBuf> {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Abrir archivo");
    dialog.set_modal(true);

    let markdown = markdown_filter();
    let filters = document_filters();
    let all = gtk::FileFilter::new();
    all.set_name(Some("Todos los archivos"));
    all.add_pattern("*");
    filters.append(&all);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&markdown));

    let parent = parent.upcast_ref::<gtk::Window>();
    let file = dialog.open_future(Some(parent)).await.ok()?;
    file.path()
}

/// Diálogo de apertura de carpeta (ISSUE-021, File ▸ Open Folder): la
/// carpeta elegida pasa a ser la raíz del explorador. Devuelve la carpeta o
/// `None` si se cancela.
pub async fn open_folder(parent: &impl IsA<gtk::Window>) -> Option<PathBuf> {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Abrir carpeta");
    dialog.set_modal(true);
    let parent = parent.upcast_ref::<gtk::Window>();
    let folder = dialog.select_folder_future(Some(parent)).await.ok()?;
    folder.path()
}

/// Diálogo «Guardar como» (§12.4): sugiere `suggested` como nombre inicial.
/// Devuelve el path elegido o `None` si se cancela.
pub async fn save_file_as(parent: &impl IsA<gtk::Window>, suggested: &str) -> Option<PathBuf> {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Guardar como");
    dialog.set_modal(true);
    if !suggested.is_empty() {
        dialog.set_initial_name(Some(suggested));
    }

    let markdown = markdown_filter();
    let filters = document_filters();
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&markdown));

    let parent = parent.upcast_ref::<gtk::Window>();
    let file = dialog.save_future(Some(parent)).await.ok()?;
    file.path()
}

/// Confirmación de descarte de cambios sin guardar (§3.7).
///
/// `title` identifica el documento afectado. Solo el botón «Descartar»
/// devuelve `true`; Cancelar, Escape o cerrar el diálogo devuelven `false`
/// (nunca se descarta en silencio).
pub async fn confirm_discard(parent: &impl IsA<gtk::Window>, title: &str) -> bool {
    let dialog = gtk::AlertDialog::builder()
        .message("¿Descartar cambios sin guardar?")
        .detail(format!("Se perderán los cambios de {title}."))
        .buttons(["Descartar", "Cancelar"])
        .default_button(1)
        .cancel_button(1)
        .build();
    let parent = parent.upcast_ref::<gtk::Window>();
    matches!(dialog.choose_future(Some(parent)).await, Ok(0))
}

/// §12.7: elección del usuario ante un fichero modificado fuera de OPM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalChange {
    /// El buffer del usuario pisa el fichero de disco (consentimiento
    /// explícito, §12.7 — nunca sobrescritura silenciosa).
    Overwrite,
    /// El buffer se recarga desde el disco (`TabManager::reload_from_disk`);
    /// solo se ofrece con el buffer limpio, porque la recarga nunca toca un
    /// buffer con cambios sin guardar (§12.7).
    Reload,
    /// Ni sobrescribir ni recargar: todo queda como está.
    Cancel,
}

/// §12.7: el archivo cambió en disco; actuar solo con consentimiento
/// explícito. Con `may_reload` (buffer SIN cambios sin guardar) el diálogo
/// ofrece además «Recargar desde disco», el camino que alimenta la
/// recarga de `TabManager::reload_from_disk` (consumo real de esa
/// recarga); con
/// cambios locales sin guardar solo caben «Sobrescribir» y «Cancelar» — el
/// buffer del usuario nunca se descarta por sorpresa.
pub async fn confirm_overwrite_external(
    parent: &impl IsA<gtk::Window>,
    path: &Path,
    may_reload: bool,
) -> ExternalChange {
    let buttons: &[&str] = if may_reload {
        &["Recargar desde disco", "Sobrescribir", "Cancelar"]
    } else {
        &["Sobrescribir", "Cancelar"]
    };
    let detail = if may_reload {
        format!(
            "«{}» fue modificado fuera de OPM. Recargar adopta el contenido del disco; \
             Sobrescribir pisa el archivo externo con tu versión.",
            path.display()
        )
    } else {
        format!(
            "«{}» fue modificado fuera de OPM y tienes cambios sin guardar. \
             ¿Sobrescribirlo con tu versión?",
            path.display()
        )
    };
    let dialog = gtk::AlertDialog::builder()
        .message("El archivo cambió en disco")
        .detail(detail)
        .buttons(buttons)
        .default_button(buttons.len() as i32 - 1)
        .cancel_button(buttons.len() as i32 - 1)
        .build();
    let parent = parent.upcast_ref::<gtk::Window>();
    match dialog.choose_future(Some(parent)).await {
        Ok(0) if may_reload => ExternalChange::Reload,
        Ok(0) => ExternalChange::Overwrite,
        Ok(1) if may_reload => ExternalChange::Overwrite,
        _ => ExternalChange::Cancel,
    }
}

/// Confirmación de borrado (§14.9, §3.7: nada destructivo en silencio) con
/// aviso de recursividad: una carpeta se elimina CON TODO SU CONTENIDO y no
/// hay papelera que lo recupere. Solo «Eliminar» devuelve `true`.
pub async fn confirm_trash(parent: &impl IsA<gtk::Window>, name: &str) -> bool {
    let dialog = gtk::AlertDialog::builder()
        .message("¿Eliminar?")
        .detail(format!(
            "«{name}» se eliminará de forma permanente. Si es una carpeta, \
             se elimina CON TODO SU CONTENIDO de forma irreversible."
        ))
        .buttons(["Eliminar", "Cancelar"])
        .default_button(1)
        .cancel_button(1)
        .build();
    let parent = parent.upcast_ref::<gtk::Window>();
    matches!(dialog.choose_future(Some(parent)).await, Ok(0))
}

/// Confirmación con mensaje y detalle propios (§3.7 / DECISIONS: borrado de
/// una ruta con pestañas sucias dentro, texto paridad con el SPA). Solo el
/// botón `confirm_label` devuelve `true`.
pub async fn confirm_message(
    parent: &impl IsA<gtk::Window>,
    message: &str,
    detail: &str,
    confirm_label: &str,
) -> bool {
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .buttons([confirm_label, "Cancel"])
        .default_button(1)
        .cancel_button(1)
        .build();
    let parent = parent.upcast_ref::<gtk::Window>();
    matches!(dialog.choose_future(Some(parent)).await, Ok(0))
}

/// Prompt de texto modal para operaciones del explorador (§14.8): nombre de
/// archivo/carpeta, nuevo nombre en `Rename` o destino en `Move`.
///
/// `on_submit(Some(text))` al aceptar (botón o Enter), `on_submit(None)` al
/// cancelar. Callback y no futuro: las operaciones del explorador son
/// cadenas de diálogos con estado propio (ISSUE-023).
pub fn prompt_text(
    parent: &impl IsA<gtk::Window>,
    title: &str,
    initial: &str,
    on_submit: Rc<dyn Fn(Option<String>)>,
) {
    let window = gtk::Window::builder()
        .title(title)
        .modal(true)
        .transient_for(parent)
        .default_width(360)
        .resizable(false)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancelar");
    let accept = gtk::Button::with_label("Aceptar");
    accept.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&accept);

    vbox.append(&entry);
    vbox.append(&buttons);
    window.set_default_widget(Some(&accept));
    window.set_child(Some(&vbox));

    {
        let window = window.clone();
        let entry = entry.clone();
        let on_submit = Rc::clone(&on_submit);
        accept.connect_clicked(move |_| {
            let text = entry.text().to_string();
            window.destroy();
            on_submit(Some(text));
        });
    }
    {
        let window = window.clone();
        let on_submit = Rc::clone(&on_submit);
        cancel.connect_clicked(move |_| {
            window.destroy();
            on_submit(None);
        });
    }
    window.connect_close_request({
        let on_submit = Rc::clone(&on_submit);
        move |_| {
            on_submit(None);
            glib::Propagation::Proceed
        }
    });

    window.present();
}

/// Filtro de ficheros de pipeline (`.yml`, `.yaml`, `.json`) para importar
/// y exportar (§6: `project/` necesita estos selectores).
pub fn pipeline_filter() -> gtk::FileFilter {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Pipeline (*.yml, *.yaml, *.json)"));
    for pattern in ["*.yml", "*.yaml", "*.json"] {
        filter.add_pattern(pattern);
    }
    filter.add_mime_type("application/yaml");
    filter.add_mime_type("application/x-yaml");
    filter.add_mime_type("application/json");
    filter
}

/// Selector de fichero de pipeline a importar (§6, Project ▸ Import
/// pipeline). Devuelve el path elegido o `None` si se cancela.
pub async fn open_pipeline(parent: &impl IsA<gtk::Window>) -> Option<PathBuf> {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Import pipeline");
    dialog.set_modal(true);

    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&pipeline_filter());
    let all = gtk::FileFilter::new();
    all.set_name(Some("Todos los archivos"));
    all.add_pattern("*");
    filters.append(&all);
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&pipeline_filter()));

    let parent = parent.upcast_ref::<gtk::Window>();
    let file = dialog.open_future(Some(parent)).await.ok()?;
    file.path()
}

/// Selector «Guardar como» para exportar el pipeline en JSON (§6, Project ▸
/// Export pipeline). Sugiere `suggested` como nombre inicial. Devuelve el
/// path elegido o `None` si se cancela.
pub async fn save_pipeline(parent: &impl IsA<gtk::Window>, suggested: &str) -> Option<PathBuf> {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Export pipeline");
    dialog.set_modal(true);
    if !suggested.is_empty() {
        dialog.set_initial_name(Some(suggested));
    }
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&pipeline_filter());
    dialog.set_filters(Some(&filters));
    dialog.set_default_filter(Some(&pipeline_filter()));

    let parent = parent.upcast_ref::<gtk::Window>();
    let file = dialog.save_future(Some(parent)).await.ok()?;
    file.path()
}
