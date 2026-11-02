// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Estado persistente del explorador (ISSUE-022, §14.3: «estado restaurable»).
//!
//! Visibilidad y ancho del panel lateral, restaurados al abrir una ventana de
//! proyecto. El ARCHIVO `ui.ini` lo posee [`crate::appstate`] (§11 A13:
//! único dueño del estado de la UI, en el directorio de configuración por SO
//! vía `directories`), fuera de cualquier proyecto (§14.2: abrir una carpeta
//! no crea ni modifica archivos del proyecto; §3.1: los archivos son del
//! usuario).
//!
//! Este módulo conserva la lógica pura: la struct [`ExplorerState`] con su
//! formato mínimo `clave=valor` (una por línea), leído de forma tolerante:
//! claves desconocidas y valores ilegibles se ignoran y dejan el default.
//! La lectura/escritura del fichero delega en `crate::appstate`.

#[cfg(test)]
use std::path::Path;

/// Ancho por defecto del panel (§14.3: ancho redimensionable).
pub const DEFAULT_WIDTH: i32 = 280;
/// Límites del ancho del panel: ni un hilo ni toda la ventana.
pub const MIN_WIDTH: i32 = 160;
pub const MAX_WIDTH: i32 = 800;

/// Estado del explorador de una ventana (§14.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplorerState {
    /// Panel visible al abrir la ventana.
    pub visible: bool,
    /// Ancho del panel en píxeles, dentro de [`MIN_WIDTH`]..=[`MAX_WIDTH`].
    pub width: i32,
}

impl Default for ExplorerState {
    fn default() -> Self {
        Self {
            visible: true,
            width: DEFAULT_WIDTH,
        }
    }
}

impl ExplorerState {
    /// Ancho acotado a [`MIN_WIDTH`]..=[`MAX_WIDTH`] (§14.3).
    pub fn clamp_width(width: i32) -> i32 {
        width.clamp(MIN_WIDTH, MAX_WIDTH)
    }
}

/// Serializa el estado en el formato `clave=valor` de `ui.ini`.
pub fn serialize(state: &ExplorerState) -> String {
    format!(
        "explorer.visible={}\nexplorer.width={}\n",
        state.visible,
        ExplorerState::clamp_width(state.width)
    )
}

/// Parsea `ui.ini` de forma tolerante: lo ilegible conserva su default.
pub fn parse(text: &str) -> ExplorerState {
    let mut state = ExplorerState::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "explorer.visible" => match value.trim() {
                "true" => state.visible = true,
                "false" => state.visible = false,
                _ => {}
            },
            "explorer.width" => {
                if let Ok(width) = value.trim().parse::<i32>() {
                    state.width = ExplorerState::clamp_width(width);
                }
            }
            // Claves desconocidas: ignoradas (§58.1: sin pérdida silenciosa
            // de datos del usuario; esto es estado propio de la aplicación).
            _ => {}
        }
    }
    state
}

/// Lee el estado guardado; sin archivo legible, [`ExplorerState::default`].
pub fn load() -> ExplorerState {
    crate::appstate::load_explorer()
}

/// Guarda el estado (escritura atómica §48). El error se propaga para que el
/// llamador decida; el explorador funciona igual sin persistencia.
pub fn save(state: &ExplorerState) -> std::io::Result<()> {
    crate::appstate::save_explorer(state)
}

/// [`load`] sobre una ruta explícita (tests).
#[cfg(test)]
pub fn load_from(path: &Path) -> ExplorerState {
    crate::appstate::load_explorer_from(path)
}

/// [`save`] sobre una ruta explícita (tests).
#[cfg(test)]
pub fn save_to(path: &Path, state: &ExplorerState) -> std::io::Result<()> {
    crate::appstate::save_explorer_to(path, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_file(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "opm-ui-state-{tag}-{}-{nanos}.ini",
            std::process::id()
        ))
    }

    #[test]
    fn defaults_14_3() {
        // §14.3: panel visible y ancho razonable al arrancar.
        let state = ExplorerState::default();
        assert!(state.visible);
        assert_eq!(state.width, DEFAULT_WIDTH);
        assert!((MIN_WIDTH..=MAX_WIDTH).contains(&state.width));
    }

    #[test]
    fn ancho_acotado_a_limites() {
        assert_eq!(ExplorerState::clamp_width(1), MIN_WIDTH);
        assert_eq!(ExplorerState::clamp_width(5000), MAX_WIDTH);
        assert_eq!(ExplorerState::clamp_width(320), 320);
    }

    #[test]
    fn serialize_y_parse_conservan_el_estado() {
        for state in [
            ExplorerState {
                visible: false,
                width: 400,
            },
            ExplorerState {
                visible: true,
                width: MIN_WIDTH,
            },
            ExplorerState::default(),
        ] {
            assert_eq!(parse(&serialize(&state)), state);
        }
    }

    #[test]
    fn parse_tolerante_con_basura() {
        // Archivo corrupto o de otra versión: se conservan los defaults por
        // clave, sin panic.
        let state = parse(
            "esto no es ini\nexplorer.visible=quizas\nexplorer.width=anchísimo\notra.clave=1\n",
        );
        assert_eq!(state, ExplorerState::default());

        // Claves válidas entre líneas ilegibles siguen aplicándose.
        let state = parse("\n# comentario\nexplorer.visible=false\nexplorer.width=1234\n");
        assert_eq!(
            state,
            ExplorerState {
                visible: false,
                width: MAX_WIDTH
            }
        );
    }

    #[test]
    fn guardar_y_cargar_desde_ruta() {
        let path = temp_file("roundtrip");
        let state = ExplorerState {
            visible: false,
            width: 333,
        };
        save_to(&path, &state).unwrap();
        assert_eq!(load_from(&path), state);
        std::fs::remove_file(&path).unwrap();

        // Sin archivo: defaults.
        assert_eq!(load_from(&path), ExplorerState::default());
    }
}
