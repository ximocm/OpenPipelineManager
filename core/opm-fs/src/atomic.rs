// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Escritura atómica en dos fases: staging + `rename` (§48, §3.7).
//!
//! §48 («Seguridad e integridad») exige «realizar guardado atómico cuando el
//! sistema lo permita»: el contenido se escribe primero en un temporal del
//! mismo directorio que el destino (mismo filesystem, luego el `rename` es
//! atómico) y solo después se renombra sobre el destino. Un crash en cualquier
//! punto deja el archivo original intacto: como mucho queda un temporal
//! huérfano. Es la garantía de §3.7 («Control del usuario»: acciones
//! explícitas, reversibles y observables; nunca sobrescritura silenciosa) y de
//! NFR-026 (sin pérdida silenciosa de datos).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Intentos de nombre temporal ante colisiones (`create_new`, §48).
const MAX_INTENTOS_TMP: usize = 100;

/// Contador de temporales dentro del proceso (§48).
static CONTADOR_TMP: AtomicUsize = AtomicUsize::new(0);

/// Guard de una escritura atómica en curso (§48).
///
/// El temporal ya está escrito y sincronizado en disco; [`StagedWrite::commit`]
/// lo renombra sobre el destino en un solo `rename` atómico. Si el guard se
/// consume sin commit — camino de error o proceso muerto —, el destino queda
/// intacto: el [`Drop`] elimina el temporal y un crash real deja solo un
/// temporal huérfano (§3.7, NFR-026).
#[derive(Debug)]
pub struct StagedWrite {
    tmp: PathBuf,
    target: PathBuf,
    /// Bandera de commit: tras renombrar, `Drop` no toca nada.
    committed: bool,
}

/// Escritura atómica en dos fases (§48): staging.
///
/// Crea un temporal `.«destino».opm-tmp-«pid»-«contador»` en el MISMO
/// directorio que `target` (mismo filesystem: el `rename` posterior es
/// atómico), escribe TODO el contenido con `write_all` y hace `sync_all`
/// (durabilidad, §48) antes de devolver el guard. Si `target` existe, el
/// temporal hereda sus permisos para que el `rename` no cambie el modo.
pub fn stage_write(target: &Path, content: &[u8]) -> std::io::Result<StagedWrite> {
    let nombre = target.file_name().ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            "el destino no termina en un nombre de archivo",
        )
    })?;
    let dir = target.parent().unwrap_or_else(|| Path::new(""));
    let (tmp, mut file) = crear_temporal(dir, nombre)?;
    // El guard elimina el temporal si cualquier paso posterior falla.
    let guard = StagedWrite {
        tmp,
        target: target.to_path_buf(),
        committed: false,
    };
    file.write_all(content)?;
    file.sync_all()?;
    drop(file);
    copiar_permisos_si_existe(target, guard.tmp_path())?;
    Ok(guard)
}

/// Escritura atómica completa: staging + commit (§48).
pub fn write_atomic(target: &Path, content: &[u8]) -> std::io::Result<()> {
    stage_write(target, content)?.commit()
}

impl StagedWrite {
    /// Renombra el temporal sobre el destino (atómico). Consume el guard.
    pub fn commit(self) -> std::io::Result<()> {
        fs::rename(&self.tmp, &self.target)?;
        // El rename ha tenido éxito: el temporal ya no existe y `Drop` no debe
        // tocar nada. Se consume el guard marcándolo como cometido.
        let mut guardia = self;
        guardia.committed = true;
        Ok(())
    }

    /// Ruta del temporal staging.
    pub fn tmp_path(&self) -> &Path {
        &self.tmp
    }

    /// Ruta del destino del commit.
    pub fn target(&self) -> &Path {
        &self.target
    }
}

impl Drop for StagedWrite {
    fn drop(&mut self) {
        // Sin commit: se elimina el temporal (camino de error limpio, §48).
        // Tras un commit no se toca nada.
        if !self.committed {
            let _ = fs::remove_file(&self.tmp);
        }
    }
}

/// Crea el temporal con `create_new` en `dir` (§48).
///
/// Nombre: `.«nombre_del_destino».opm-tmp-«pid»-«contador»`. Ante
/// colisión se incrementa el contador y se reintenta (hasta
/// [`MAX_INTENTOS_TMP`]).
fn crear_temporal(dir: &Path, nombre_destino: &OsStr) -> std::io::Result<(PathBuf, fs::File)> {
    let pid = std::process::id();
    let base = CONTADOR_TMP.fetch_add(1, Ordering::Relaxed);
    for intento in 0..MAX_INTENTOS_TMP {
        let mut nombre = OsString::from(".");
        nombre.push(nombre_destino);
        nombre.push(format!(".opm-tmp-{}-{}", pid, base.wrapping_add(intento)));
        let tmp = dir.join(nombre);
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            // Colisión con un temporal ya existente: se incrementa el contador.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            resultado => return resultado.map(|file| (tmp, file)),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        format!(
            "sin nombre temporal libre tras {} intentos",
            MAX_INTENTOS_TMP
        ),
    ))
}

/// Copia los permisos de `target` sobre `tmp` si `target` existe (§48).
///
/// Así el `rename` posterior no cambia el modo del archivo (en unix, verificado
/// con `PermissionsExt` en los tests). Un destino inexistente no tiene permisos
/// que preservar.
fn copiar_permisos_si_existe(target: &Path, tmp: &Path) -> std::io::Result<()> {
    match fs::metadata(target) {
        Ok(meta) => fs::set_permissions(tmp, meta.permissions()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Directorio temporal de test, eliminado al salir.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            Self(crate::test_dir::crear("opm-atomic"))
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        /// Nombres de las entradas del directorio, ordenados.
        fn nombres(&self) -> Vec<String> {
            let mut nombres: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            nombres.sort();
            nombres
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn write_atomic_crea_y_actualiza_sin_temporales() {
        // §48: temporal + rename; en el directorio solo queda el destino.
        let dir = TestDir::new();
        let destino = dir.file("doc.md");

        write_atomic(&destino, b"hola\n").unwrap();
        assert_eq!(fs::read(&destino).unwrap(), b"hola\n");
        assert_eq!(dir.nombres(), ["doc.md"]);

        write_atomic(&destino, b"actualizado\n").unwrap();
        assert_eq!(fs::read(&destino).unwrap(), b"actualizado\n");
        assert_eq!(dir.nombres(), ["doc.md"]);
    }

    #[test]
    #[cfg(unix)]
    fn write_atomic_preserva_permisos_del_destino() {
        // §48: el rename no debe cambiar el modo del archivo existente.
        use std::os::unix::fs::PermissionsExt;

        let dir = TestDir::new();
        let destino = dir.file("doc.md");
        fs::write(&destino, b"viejo\n").unwrap();
        fs::set_permissions(&destino, fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&destino, b"nuevo\n").unwrap();

        assert_eq!(fs::read(&destino).unwrap(), b"nuevo\n");
        let modo = fs::metadata(&destino).unwrap().permissions().mode() & 0o777;
        assert_eq!(modo, 0o600);
    }

    #[test]
    fn drop_sin_commit_conserva_destino_y_borra_temporal() {
        // §48: sin commit el destino queda intacto y el temporal se elimina.
        let dir = TestDir::new();
        let destino = dir.file("doc.md");
        fs::write(&destino, b"PREVIO\n").unwrap();

        let guard = stage_write(&destino, b"NUEVO\n").unwrap();
        let tmp = guard.tmp_path().to_path_buf();
        assert!(tmp.exists());
        assert_eq!(guard.target(), destino);
        drop(guard);

        assert!(!tmp.exists(), "el temporal debe eliminarse sin commit");
        assert_eq!(fs::read(&destino).unwrap(), b"PREVIO\n");
        assert_eq!(dir.nombres(), ["doc.md"]);
    }

    /// Proceso hijo de [`crash_simulado_no_corrompe_el_original`] (§48, §3.7).
    ///
    /// Si está definida `MINMARK_ATOMIC_CRASH` (valor = ruta destino), estageda
    /// `"NUEVO\n"` y muere con [`std::process::exit`] sin commit ni `Drop`:
    /// simula un crash real en mitad de la escritura atómica. Sin la variable
    /// retorna `Ok` inmediatamente, así pasa en toda ejecución normal de
    /// `cargo test`.
    #[test]
    fn hijo_muere_entre_staging_y_commit() -> std::io::Result<()> {
        let Ok(destino) = std::env::var("MINMARK_ATOMIC_CRASH") else {
            return Ok(());
        };
        let destino = PathBuf::from(destino);
        let _guard = stage_write(&destino, b"NUEVO\n")?;
        // `process::exit` no ejecuta destructors: el temporal queda huérfano y
        // el destino intacto, exactamente como un crash real (NFR-026).
        std::process::exit(0)
    }

    #[test]
    fn crash_simulado_no_corrompe_el_original() {
        // §48 / §3.7: un crash entre staging y commit nunca corrompe ni
        // trunca el archivo guardado.
        let dir = TestDir::new();
        let destino = dir.file("doc.md");
        fs::write(&destino, b"ORIGINAL\n").unwrap();

        // `--exact` compara el nombre completo que libtest registra para el
        // test hijo: ruta de módulos sin el crate raíz (p.ej.
        // `atomic::tests::hijo_muere_entre_staging_y_commit`).
        let modulo = module_path!();
        let sin_crate = modulo.splitn(2, "::").nth(1).unwrap_or(modulo);
        let nombre_hijo = format!("{}::hijo_muere_entre_staging_y_commit", sin_crate);

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([&nombre_hijo, "--exact", "--nocapture"])
            .env("MINMARK_ATOMIC_CRASH", &destino)
            .env("RUST_TEST_THREADS", "1")
            .status()
            .expect("no se pudo lanzar el proceso hijo");
        assert!(status.success(), "el proceso hijo terminó con {}", status);

        // (a) el destino conserva byte a byte su contenido original.
        assert_eq!(fs::read(&destino).unwrap(), b"ORIGINAL\n");

        // (b) queda exactamente un temporal huérfano del crash.
        let huerfanos: Vec<String> = dir
            .nombres()
            .into_iter()
            .filter(|n| n.contains("opm-tmp"))
            .collect();
        assert_eq!(
            huerfanos.len(),
            1,
            "debe quedar exactamente un temporal huérfano, no {:?}",
            huerfanos
        );

        // (c) un write_atomic posterior termina bien con el contenido nuevo.
        write_atomic(&destino, b"FINAL\n").unwrap();
        assert_eq!(fs::read(&destino).unwrap(), b"FINAL\n");
    }
}
