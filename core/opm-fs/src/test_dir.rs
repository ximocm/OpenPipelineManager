// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Directorios temporales únicos para los tests (§15).
//!
//! Los tests crean sus directorios en paralelo dentro del mismo proceso y el
//! reloj del sistema puede repetir `nanos` (en Windows su granularidad es de
//! milisegundos): nombrando solo con pid + nanos, dos tests podían recibir el
//! MISMO directorio y pisarse entre sí (temporales huérfanos ajenos,
//! directorios borrados por el `Drop` del test vecino). El contador del
//! proceso garantiza nombres distintos y `create_dir` falla en vez de
//! compartir un directorio ya existente.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static CONTADOR: AtomicUsize = AtomicUsize::new(0);

/// Crea (vacío) un directorio temporal único llamado
/// `«prefijo»-«pid»-«nanos»-«contador»`.
pub(crate) fn crear(prefijo: &str) -> PathBuf {
    for _ in 0..100 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let cuenta = CONTADOR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("{prefijo}-{}-{nanos}-{cuenta}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return path,
            // Nombre tomado (p. ej. resto de una ejecución anterior con el
            // mismo pid): se reintenta con el siguiente contador.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("no se pudo crear el directorio temporal {path:?}: {e}"),
        }
    }
    panic!("sin nombre libre para el directorio temporal de test");
}
