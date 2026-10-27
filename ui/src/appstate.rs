//! Estado persistente de la UI (§11 A13): ÚNICO dueño del fichero
//! `ui.ini-style` de la aplicación.
//!
//! Vive en el directorio de configuración por SO vía `directories`
//! (`~/.config/opm/ui.ini` en Linux, `%APPDATA%\opm\ui.ini` en Windows,
//! `~/Library/Application Support/opm/ui.ini` en macOS) — fuera de cualquier
//! proyecto (§14.2: abrir una carpeta no crea ni modifica archivos del
//! proyecto). Contiene:
//!
//! - el estado del explorador (`explorer.visible`, `explorer.width`), con
//!   su lógica pura delegada en [`crate::explorer`];
//! - el hueco de sesión `last_project` (`last_project` + `last_project_at`,
//!   timestamp unix): `crate::project::restore` lee este hueco y aplica la
//!   semántica de sesión con TTL de 1 h (paridad con el SPA).
//!
//! Formato `clave=valor` (una por línea) leído de forma tolerante: claves
//! desconocidas y valores ilegibles se ignoran y dejan el default. Toda
//! escritura es atómica ([`opm_fs::write_atomic`], §48) y de reemplazo del
//! fichero entero: cada guardado relé el estado completo, cambia SU clave y
//! reescribe, de modo que las claves de otros módulos no se pierden.
//! Un `last_project` con saltos de línea en la ruta no se persiste (rompería
//! el formato de una línea por clave; se prefiere no restaurar).

use crate::explorer::ExplorerState;
use opm_fs::write_atomic;
use std::path::{Path, PathBuf};

/// Estado completo del fichero `ui.ini`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct UiState {
    explorer: ExplorerState,
    last_project: Option<(String, i64)>,
}

/// Ruta de `ui.ini` en el directorio de configuración por SO.
///
/// Sin base utilizable degrada al directorio temporal: el estado es un
/// confort de UI, nunca un requisito para funcionar.
pub fn state_file() -> PathBuf {
    directories::BaseDirs::new()
        .map(|dirs| dirs.config_dir().join("opm").join("ui.ini"))
        .unwrap_or_else(|| std::env::temp_dir().join("opm").join("ui.ini"))
}

/// Lee y parsea el fichero completo de forma tolerante; sin archivo
/// legible, los defaults.
fn read_state(path: &Path) -> UiState {
    std::fs::read_to_string(path)
        .map(|text| parse_state(&text))
        .unwrap_or_default()
}

fn parse_state(text: &str) -> UiState {
    let explorer = crate::explorer::parse_explorer_text(text);
    let mut path: Option<String> = None;
    let mut at: Option<i64> = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "last_project" => {
                let value = value.trim();
                if !value.is_empty() {
                    path = Some(value.to_owned());
                }
            }
            "last_project_at" => at = value.trim().parse::<i64>().ok(),
            _ => {}
        }
    }
    UiState {
        explorer,
        // El hueco solo vale con ruta Y timestamp (una clave huérfana no
        // restaura nada).
        last_project: path.zip(at),
    }
}

/// Serializa el estado entero: primero las claves del explorador (su
/// formato propio), después el hueco de sesión.
fn compose_state(state: &UiState) -> String {
    let mut text = crate::explorer::serialize_explorer_text(&state.explorer);
    if let Some((path, at)) = &state.last_project {
        if !path.contains('\n') && !path.contains('\r') {
            text.push_str(&format!("last_project={path}\nlast_project_at={at}\n"));
        }
    }
    text
}

/// Escritura atómica del fichero completo (§48), creando el directorio si
/// hace falta.
fn write_state(path: &Path, state: &UiState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(path, compose_state(state).as_bytes())
}

// --- API pública: una operación por clave, sobre el fichero completo -------

/// Estado del explorador guardado; sin archivo legible, default (§14.3).
pub fn load_explorer() -> ExplorerState {
    load_explorer_from(&state_file())
}

/// [`load_explorer`] sobre una ruta explícita (tests y delegación de
/// `explorer::state`).
pub fn load_explorer_from(path: &Path) -> ExplorerState {
    read_state(path).explorer
}

/// Guarda el estado del explorador conservando el resto de claves (§14.3).
pub fn save_explorer(state: &ExplorerState) -> std::io::Result<()> {
    save_explorer_to(&state_file(), state)
}

/// [`save_explorer`] sobre una ruta explícita (tests y delegación de
/// `explorer::state`).
pub fn save_explorer_to(path: &Path, state: &ExplorerState) -> std::io::Result<()> {
    let mut ui = read_state(path);
    ui.explorer = *state;
    write_state(path, &ui)
}

/// Hueco de sesión: último proyecto abierto con su timestamp unix; `None`
/// sin archivo legible. `crate::project` aplica el TTL de 1 h sobre este
/// valor (restauración de sesión; el alta la hace `build_window`).
pub fn load_last_project() -> Option<(String, i64)> {
    load_last_project_from(&state_file())
}

/// [`load_last_project`] sobre una ruta explícita (tests).
pub fn load_last_project_from(path: &Path) -> Option<(String, i64)> {
    read_state(path).last_project
}

/// Guarda el hueco de sesión conservando el resto de claves.
pub fn save_last_project(slot: &Option<(String, i64)>) -> std::io::Result<()> {
    save_last_project_to(&state_file(), slot)
}

/// [`save_last_project`] sobre una ruta explícita (tests).
pub fn save_last_project_to(path: &Path, slot: &Option<(String, i64)>) -> std::io::Result<()> {
    let mut ui = read_state(path);
    ui.last_project = slot.clone();
    write_state(path, &ui)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "opm-appstate-{tag}-{}-{nanos}.ini",
            std::process::id()
        ))
    }

    #[test]
    fn sin_archivo_las_claves_dan_sus_defaults() {
        let path = temp_file("ausente");
        assert_eq!(load_explorer_from(&path), ExplorerState::default());
        assert_eq!(load_last_project_from(&path), None);
    }

    #[test]
    fn roundtrip_explorer_sin_tocar_el_hueco_de_sesion() {
        let path = temp_file("explorer");
        let slot = Some(("/proyecto/uno".to_owned(), 1_700_000_000_i64));
        save_last_project_to(&path, &slot).unwrap();

        let state = ExplorerState {
            visible: false,
            width: 444,
        };
        save_explorer_to(&path, &state).unwrap();
        assert_eq!(load_explorer_from(&path), state);
        // Reescritura de reemplazo completo: el hueco de sesión sobrevive.
        assert_eq!(load_last_project_from(&path), slot);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn roundtrip_sesion_sin_tocar_el_explorador() {
        let path = temp_file("sesion");
        let explorer = ExplorerState {
            visible: false,
            width: 321,
        };
        save_explorer_to(&path, &explorer).unwrap();

        let slot = Some(("/datos/proyecto x".to_owned(), 42_i64));
        save_last_project_to(&path, &slot).unwrap();
        assert_eq!(load_last_project_from(&path), slot);
        assert_eq!(load_explorer_from(&path), explorer);

        // Quitar la sesión conserva el explorador.
        save_last_project_to(&path, &None).unwrap();
        assert_eq!(load_last_project_from(&path), None);
        assert_eq!(load_explorer_from(&path), explorer);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn tolerante_a_basura_y_a_claves_huerfanas() {
        let state = parse_state(
            "linea rota\nexplorer.visible=false\nexplorer.width=mucha\n\
             last_project=/solo-ruta\notra=1\n",
        );
        // Ancho ilegible → default; visible sí se aplica; sesión incompleta
        // (sin timestamp) no restaura.
        assert_eq!(
            state.explorer,
            ExplorerState {
                visible: false,
                width: ExplorerState::default().width,
            }
        );
        assert_eq!(state.last_project, None);
    }

    #[test]
    fn ruta_con_salto_de_linea_no_se_persiste() {
        let path = temp_file("newline");
        let slot = Some(("/mal\nexplorer.width=99999".to_owned(), 7_i64));
        save_last_project_to(&path, &slot).unwrap();
        // No se guardó (rompería el formato): nada restaurable…
        assert_eq!(load_last_project_from(&path), None);
        // …y el ancho del explorador no fue secuestrado.
        assert_eq!(
            load_explorer_from(&path).width,
            ExplorerState::default().width
        );
        std::fs::remove_file(&path).unwrap();
    }
}
