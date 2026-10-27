//! Tema de la UI: la [`Palette`] de `opm-theme` más la detección
//! claro/oscuro de GTK (§23: la paleta es la fuente única de color).
//!
//! La detección consulta los `gtk::Settings` del display (theme name y la
//! preferencia oscura de la aplicación); sin display todavía devuelve el
//! tema claro. El CSS de la ventana se genera en `main.rs` con esta paleta.

pub use opm_theme::Palette;

/// ¿El tema GTK vigente es oscuro?
///
/// Dos señales: `gtk-application-prefer-dark-theme` (fijada por la app) y el
/// nombre del tema terminando en `-dark`/`:dark` (Adwaita-dark, GTK_THEME
/// con variante).
pub fn is_dark() -> bool {
    if !gtk4::is_initialized() {
        return false;
    }
    let Some(settings) = gtk4::Settings::default() else {
        return false;
    };
    if settings.is_gtk_application_prefer_dark_theme() {
        return true;
    }
    settings
        .gtk_theme_name()
        .map(|name| {
            let name = name.to_string();
            name.ends_with("-dark") || name.ends_with(":dark")
        })
        .unwrap_or(false)
}

/// Paleta del tema vigente (§23).
pub fn palette() -> Palette {
    if is_dark() {
        Palette::DARK
    } else {
        Palette::LIGHT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La paleta elegida es SIEMPRE uno de los dos valores canónicos de la
    /// fuente única de color (§23); el resto de helpers no muta estado.
    #[test]
    fn paleta_es_uno_de_los_canonicos() {
        assert!(matches!(palette(), Palette::LIGHT | Palette::DARK));
        // Sin display en los tests, la detección es conservadora.
        let detected = is_dark();
        assert_eq!(palette(), if detected { Palette::DARK } else { Palette::LIGHT });
    }
}
