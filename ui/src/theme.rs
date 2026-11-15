//! Tema de la UI: la [`Palette`] de `opm-theme` más la detección
//! claro/oscuro de GTK (§23: la paleta es la fuente única de color).
//!
//! La detección consulta los `gtk::Settings` del display (theme name y la
//! preferencia oscura de la aplicación); sin display todavía devuelve el
//! tema claro. El CSS de la ventana se genera en `main.rs` con esta paleta.
//! El selector View ▸ Light/Dark/Follow system fija un [`ThemeMode`] que
//! manda sobre la detección (Light/Dark) o la respeta (System).

pub use opm_theme::Palette;

use std::cell::Cell;

/// Modo de tema de la interfaz (View ▸ Light/Dark/Follow system).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    /// Fuerza la paleta clara.
    Light,
    /// Fuerza la paleta oscura.
    Dark,
    /// Respeta la detección GTK del escritorio.
    #[default]
    System,
}

impl ThemeMode {
    /// Los tres modos, en orden del radio del menú (solo tests: la tabla de
    /// radios vive en [`crate::menus::actions`]).
    #[cfg(test)]
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System];

    /// Identificador estable: objetivo del radio `win.theme::<id>` y valor
    /// de la clave `theme_mode` de `ui.ini`.
    pub fn id(self) -> &'static str {
        match self {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
            ThemeMode::System => "system",
        }
    }

    /// [`ThemeMode::id`] inverso; `None` para identificadores desconocidos.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "light" => Some(ThemeMode::Light),
            "dark" => Some(ThemeMode::Dark),
            "system" => Some(ThemeMode::System),
            _ => None,
        }
    }
}

thread_local! {
    /// Modo vigente. `thread_local` como [`crate::install_theme_css`]: GTK es
    /// single-threaded y los tests no comparten estado entre sí.
    static MODE: Cell<ThemeMode> = const { Cell::new(ThemeMode::System) };
}

/// Modo de tema vigente.
pub fn mode() -> ThemeMode {
    MODE.with(Cell::get)
}

/// Fija el modo y alinea el tema GTK: Dark fuerza la preferencia oscura de
/// la aplicación, Light y System la revierten para que mande la paleta
/// forzada o el nombre de tema del escritorio respectivamente.
pub fn set_mode(mode: ThemeMode) {
    MODE.with(|cell| cell.set(mode));
    if gtk4::is_initialized() {
        if let Some(settings) = gtk4::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(mode == ThemeMode::Dark);
        }
    }
}

/// ¿El tema vigente es oscuro? Los modos forzados mandan; [`ThemeMode::System`]
/// delega en la detección GTK.
pub fn is_dark() -> bool {
    match mode() {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => detect_dark(),
    }
}

/// ¿El tema GTK vigente es oscuro? (solo [`ThemeMode::System`]).
///
/// Dos señales: `gtk-application-prefer-dark-theme` (fijada por la app) y el
/// nombre del tema terminando en `-dark`/`:dark` (Adwaita-dark, GTK_THEME
/// con variante).
fn detect_dark() -> bool {
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

    /// El modo forzado manda sobre la detección; System la respeta.
    #[test]
    fn modo_forzado_manda_sobre_la_deteccion() {
        let sistema = is_dark();
        set_mode(ThemeMode::Dark);
        assert!(is_dark());
        assert_eq!(palette(), Palette::DARK);
        set_mode(ThemeMode::Light);
        assert!(!is_dark());
        assert_eq!(palette(), Palette::LIGHT);
        set_mode(ThemeMode::System);
        assert_eq!(is_dark(), sistema);
    }

    /// Identificadores estables (radio `win.theme::<id>` y clave `theme_mode`).
    #[test]
    fn ids_de_modo_ida_y_vuelta() {
        for mode in ThemeMode::ALL {
            assert_eq!(ThemeMode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(ThemeMode::from_id("sepia"), None);
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
