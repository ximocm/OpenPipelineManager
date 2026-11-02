// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Paletas de tema Light/Dark (§11, §23).
//!
//! Fuente única de color: genera las variables CSS del Preview y expone los
//! mismos valores para la UI GTK4 (ISSUE-020 estiliza widgets con [`Palette`]).
//!
//! Paletas de §23: física (red, orange, yellow, green, blue, purple, gray,
//! black, white) y semántica (danger, warning, success, info, note, muted,
//! highlight, accent). Los colores del proyecto (`custom_colors`, §58.2) se
//! aplican como fallback en el CSS.

/// Colores de un tema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: &'static str,
    pub fg: &'static str,
    pub muted: &'static str,
    pub code_bg: &'static str,
    pub rule: &'static str,
    pub quote: &'static str,
    /// Colores por nombre: paletas física y semántica (§23).
    pub colors: &'static [(&'static str, &'static str)],
}

const LIGHT_COLORS: &[(&str, &str)] = &[
    ("red", "#d32f2f"),
    ("orange", "#ef6c00"),
    ("yellow", "#b58900"),
    ("green", "#2e7d32"),
    ("blue", "#1565c0"),
    ("purple", "#6a1b9a"),
    ("gray", "#757575"),
    ("black", "#1f2328"),
    ("white", "#ffffff"),
    ("danger", "#c62828"),
    ("warning", "#e65100"),
    ("success", "#2e7d32"),
    ("info", "#0277bd"),
    ("note", "#455a64"),
    ("muted", "#757575"),
    ("highlight", "#fff59d"),
    ("accent", "#1565c0"),
];

const DARK_COLORS: &[(&str, &str)] = &[
    ("red", "#ef5350"),
    ("orange", "#ffa726"),
    ("yellow", "#fdd835"),
    ("green", "#66bb6a"),
    ("blue", "#42a5f5"),
    ("purple", "#ab47bc"),
    ("gray", "#9e9e9e"),
    ("black", "#e6e6e6"),
    ("white", "#15181c"),
    ("danger", "#ef5350"),
    ("warning", "#ffa726"),
    ("success", "#66bb6a"),
    ("info", "#29b6f6"),
    ("note", "#90a4ae"),
    ("muted", "#9e9e9e"),
    ("highlight", "#5d4a1f"),
    ("accent", "#42a5f5"),
];

impl Palette {
    /// Tema claro (§11).
    pub const LIGHT: Self = Self {
        bg: "#ffffff",
        fg: "#1f2328",
        muted: "#6b7280",
        code_bg: "#f6f8fa",
        rule: "#d0d7de",
        quote: "#57606a",
        colors: LIGHT_COLORS,
    };

    /// Tema oscuro (§11).
    pub const DARK: Self = Self {
        bg: "#15181c",
        fg: "#e6e6e6",
        muted: "#9aa4af",
        code_bg: "#1f2429",
        rule: "#3a4048",
        quote: "#9aa4af",
        colors: DARK_COLORS,
    };

    /// Valor de un color por nombre (§23).
    pub fn color(&self, name: &str) -> Option<&'static str> {
        self.colors
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| *value)
    }
}

/// Variables CSS de los tres modos `data-theme` (§11): Light por defecto,
/// Dark explícito y System resuelto con `prefers-color-scheme`.
pub fn theme_css() -> String {
    let mut out = String::new();
    write_vars(&mut out, ":root", &Palette::LIGHT);
    write_vars(&mut out, "html[data-theme=\"dark\"]", &Palette::DARK);
    out.push_str("@media (prefers-color-scheme: dark) {\n");
    write_vars(
        &mut out,
        "  html[data-theme=\"system\"]",
        &Palette::DARK,
    );
    out.push_str("}\n");
    out
}

fn write_vars(out: &mut String, selector: &str, palette: &Palette) {
    out.push_str(selector);
    out.push_str(" {\n");
    out.push_str(&format!("  --mm-bg: {}; --mm-fg: {}; --mm-muted: {};\n", palette.bg, palette.fg, palette.muted));
    out.push_str(&format!(
        "  --mm-code-bg: {}; --mm-rule: {}; --mm-quote: {};\n",
        palette.code_bg, palette.rule, palette.quote
    ));
    for (name, value) in palette.colors {
        out.push_str(&format!("  --mm-color-{name}: {value};\n"));
    }
    out.push_str("}\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paletas_completas_23() {
        for palette in [&Palette::LIGHT, &Palette::DARK] {
            // 9 físicos + 8 semánticos (§23).
            assert_eq!(palette.colors.len(), 17);
            for name in [
                "red", "orange", "yellow", "green", "blue", "purple", "gray", "black", "white",
                "danger", "warning", "success", "info", "note", "muted", "highlight", "accent",
            ] {
                assert!(palette.color(name).is_some(), "falta {name}");
            }
            assert_eq!(palette.color("noexiste"), None);
        }
    }

    #[test]
    fn css_generado_con_ambos_temas() {
        let css = theme_css();
        assert!(css.contains(":root"));
        assert!(css.contains("--mm-bg: #ffffff"));
        assert!(css.contains("html[data-theme=\"dark\"]"));
        assert!(css.contains("--mm-bg: #15181c"));
        // System resuelto por media query del sistema.
        assert!(css.contains("@media (prefers-color-scheme: dark)"));
        assert!(css.contains("html[data-theme=\"system\"]"));
    }
}
