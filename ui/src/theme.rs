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
    // GTK_THEME overrides the renderer without necessarily changing Settings.
    if let Ok(name) = std::env::var("GTK_THEME") {
        return dark_name(&name);
    }
    if settings.is_gtk_application_prefer_dark_theme() {
        return true;
    }
    settings
        .gtk_theme_name()
        .map(|name| {
            let name = name.to_string();
            dark_name(&name)
        })
        .unwrap_or(false)
}

/// Paleta del tema vigente (§23).
pub fn palette() -> Palette {
    let mut palette = if is_dark() {
        Palette::DARK
    } else {
        Palette::LIGHT
    };
    if gtk4::is_initialized() {
        let name = std::env::var("GTK_THEME")
            .ok()
            .or_else(|| {
                gtk4::Settings::default()
                    .and_then(|s| s.gtk_theme_name())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default()
            .to_lowercase();
        if name.contains("highcontrast") || name.ends_with(":hc") {
            palette.rule = palette.fg;
        }
    }
    palette
}

fn dark_name(name: &str) -> bool {
    let name = name.to_lowercase();
    name.ends_with("-dark") || name.ends_with(":dark") || name.ends_with("inverse")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtk_theme_variant_names_include_dark_and_high_contrast_inverse() {
        for name in ["Adwaita:dark", "Adwaita-dark", "HighContrastInverse"] {
            assert!(dark_name(name));
        }
        assert!(!dark_name("HighContrast"));
        assert!(!dark_name("Adwaita"));
    }

    /// La paleta elegida es SIEMPRE uno de los dos valores canónicos de la
    /// fuente única de color (§23); el resto de helpers no muta estado.
    #[test]
    fn paleta_es_uno_de_los_canonicos() {
        assert!(matches!(palette(), Palette::LIGHT | Palette::DARK));
        // Sin display en los tests, la detección es conservadora.
        let detected = is_dark();
        assert_eq!(
            palette(),
            if detected {
                Palette::DARK
            } else {
                Palette::LIGHT
            }
        );
    }
}
