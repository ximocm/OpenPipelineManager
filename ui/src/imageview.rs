//! Visor de imágenes (§11 A11): las imágenes abren en una página DEDICADA
//! del stack central con `gtk::Picture`, nunca por la ruta UTF-8 del editor.
//!
//! El explorador y `File ▸ Open` enrutan aquí las extensiones de
//! [`IMAGE_EXTENSIONS`] (las mismas de `opm_fs::preview_file`); la pestaña
//! de texto de `TabManager` jamás carga un fichero de imagen.

use std::path::Path;

use gtk4 as gtk;
use gtk4::prelude::*;

/// Sufijos de imagen (sin punto, iguales a los de `opm_fs`).
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg"];

/// ¿`path` es un fichero de imagen que debe abrirse en el visor dedicado?
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let ext = ext.to_ascii_lowercase();
            IMAGE_EXTENSIONS.contains(&ext.as_str())
        })
        .unwrap_or(false)
}

/// Nombre mostrable de la página (último componente del path).
pub fn page_title(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Tipo media básico desde el sufijo (para la cabecera del visor).
fn media_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Página de visor para `path`: cabecera con la ruta + `gtk::Picture`
/// dentro de un `ScrolledWindow` (A11: nunca `TabManager`).
pub fn build_page(path: &Path) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("opm-image-viewer");

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.add_css_class("opm-image-toolbar");
    let label = gtk::Label::new(Some(&path.display().to_string()));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let media = gtk::Label::new(Some(media_for(path)));
    media.add_css_class("opm-dim");
    toolbar.append(&label);
    toolbar.append(&media);
    root.append(&toolbar);

    let body: gtk::Widget = if path.is_file() {
        let picture = gtk::Picture::for_filename(path);
        picture.set_can_shrink(false);
        picture.set_content_fit(gtk::ContentFit::Contain);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&picture));
        scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.upcast::<gtk::Widget>()
    } else {
        // Fichero borrado entre aperturas: aviso claro, sin imagen rota.
        let label = gtk::Label::new(Some(&format!(
            "Image not found: {}",
            path.display()
        )));
        label.set_halign(gtk::Align::Center);
        label.set_valign(gtk::Align::Center);
        label.add_css_class("opm-dim");
        label.upcast::<gtk::Widget>()
    };
    root.append(&body);

    root.upcast::<gtk::Widget>()
}

/// Clave de página de imagen en el stack central (`stack` child name).
///
/// La clave compara el path como TEXTO, así que `\` y `/` se normalizan
/// (P1): la misma imagen serializada de las dos formas (`C:\proy\res.png` y
/// `C:/proy/res.png`) comparte página en vez de duplicarla.
pub fn page_id(path: &Path) -> String {
    format!("image:{}", path.display().to_string().replace('\\', "/"))
}

/// ¿Una clave de página del stack es la imagen `path`?
pub fn page_id_matches(id: &str, path: &Path) -> bool {
    id == page_id(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rutas de imagen: case-insensitive, con y sin extensión (A11).
    #[test]
    fn rutas_de_imagen() {
        assert!(is_image_path(Path::new("a/b/shot.PNG")));
        assert!(is_image_path(Path::new("diagram.svg")));
        assert!(is_image_path(Path::new("photo.jpeg")));
        assert!(!is_image_path(Path::new("pipeline.yml")));
        assert!(!is_image_path(Path::new("README")));
        assert!(!is_image_path(Path::new("archive.png.txt")));
    }

    #[test]
    fn titulos_y_claves_de_pagina() {
        let path = Path::new("/proy/outputs/res.png");
        assert_eq!(page_title(path), "res.png");
        assert_eq!(page_id(path), "image:/proy/outputs/res.png");
        assert!(page_id_matches(&page_id(path), path));
        assert!(!page_id_matches("image:/otro.png", path));

        // P1: la clave es un string de path — formas mezcladas de la MISMA
        // imagen comparten página (y la forma Windows normalizada no abre
        // una segunda página junto a la POSIX).
        let windows = Path::new(r"C:\proy\res.png");
        assert_eq!(page_id(windows), "image:C:/proy/res.png");
        assert!(page_id_matches(&page_id(windows), Path::new("C:/proy/res.png")));
        assert!(page_id_matches(&page_id(Path::new("C:/proy/res.png")), windows));
    }

    /// Montaje de la página: con imagen ausente y con fichero real
    /// (requiere sesión gráfica).
    #[test]
    #[ignore = "requiere sesión gráfica GTK"]
    fn pagina_de_visor_se_construye() {
        assert!(gtk::init().is_ok());
        let missing = std::env::temp_dir().join("opm-imageview-missing.png");
        let widget = build_page(&missing);
        assert!(widget.parent().is_none(), "página suelta antes de empaquetar");
        let other = build_page(Path::new("Cargo.toml"));
        assert!(other.parent().is_none());
    }
}
