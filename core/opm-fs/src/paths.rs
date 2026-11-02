// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Validación de rutas y árbol de proyecto con symlinks seguros (ISSUE-019).
//!
//! - §48: toda ruta interna se valida antes de usarla; ninguna ruta resuelta
//!   puede salir del proyecto (path traversal) y los ciclos de symlink no
//!   pueden colgar el árbol.
//! - §14.7: los symlinks se muestran como entradas, pero las carpetas
//!   enlazadas mediante symlink no se recorren recursivamente.
//! - §17: las rutas relativas se interpretan contra el directorio del
//!   proyecto.

use crate::FsError;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

/// Entrada del árbol de proyecto (§14.4, §14.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// Ruta de la entrada (la del propio symlink, no la de su destino).
    pub path: PathBuf,
    /// Nombre base de la entrada.
    pub name: String,
    /// `true` si la entrada es un symlink (§14.7: se muestra como entrada).
    pub is_symlink: bool,
    /// Del destino real (sigue el symlink); solo informativo (§14.7).
    pub is_dir: bool,
}

/// Canoniza `path` con [`fs::canonicalize`] y normaliza el namespace de la
/// ruta resultante (§48).
///
/// En Windows `fs::canonicalize` devuelve rutas *verbatim*
/// (`\\?\C:\a\b.md`, `\\?\UNC\servidor\compartido\…`), que **no** comparan
/// por prefijo (`starts_with`/`strip_prefix`) con las rutas absolutas
/// «planas» que escriben el usuario, la UI y los ficheros del proyecto
/// (`C:\a\b.md`, `\\servidor\compartido\…`): `\\?\…` no compone igual que
/// `C:\…`, así que una ruta dentro del proyecto se clasificaría como ajena
/// (§48). Esta función devuelve siempre la forma plana:
/// `\\?\C:\a\b.md` → `C:\a\b.md` y `\\?\UNC\servidor\compartido\…` →
/// `\\servidor\compartido\…`.
///
/// **Invariante**: toda canonicalización de opm-fs (este módulo, `local`,
/// `ops` y `pipeline_paths`) y toda
/// canonicalización del `ui` que se combine o compare con rutas devueltas
/// por este crate pasa por aquí, de modo que las dos rutas de cualquier
/// comparación pertenecen al mismo namespace. En Linux y macOS no existe el
/// prefijo verbatim: el resultado es byte-idéntico al de `fs::canonicalize`.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let canon = fs::canonicalize(path)?;
    Ok(sin_verbatim(&canon))
}

/// Quita el prefijo verbatim de Windows a una ruta ya canónica:
/// `\\?\C:\a\b` → `C:\a\b` y `\\?\UNC\servidor\compartido\a` →
/// `\\servidor\compartido\a`. Los demás prefijos verbatim (p. ej.
/// `\\?\Volume{…}`, sin forma plana equivalente) y las rutas sin prefijo se
/// devuelven intactos.
fn sin_verbatim(canon: &Path) -> PathBuf {
    /// Reconstruye un `OsStr` recortado de [`OsStr::as_encoded_bytes`].
    ///
    /// SAFETY: los cortes de `sin_verbatim` se hacen siempre tras un prefijo
    /// de solo ASCII (`\\?\UNC\`, `\\?\`), y el resto es un sufijo de los
    /// bytes codificados de un `OsStr` real, que siguen siendo WTF-8 válido
    /// (Unix: bytes crudos; Windows: WTF-8 sin caracteres entre comodines).
    fn recorte(bytes: &[u8]) -> &OsStr {
        unsafe { OsStr::from_encoded_bytes_unchecked(bytes) }
    }

    let bytes = canon.as_os_str().as_encoded_bytes();
    if let Some(resto) = bytes.strip_prefix(&b"\\\\?\\UNC\\"[..]) {
        let mut out = OsString::from(r"\\");
        out.push(recorte(resto));
        return PathBuf::from(out);
    }
    match bytes.strip_prefix(&b"\\\\?\\"[..]) {
        // Solo la forma con letra de unidad (`\\?\C:\…`): se queda sin el
        // prefijo (`C:\…`).
        Some(resto)
            if resto.len() >= 3
                && resto[0].is_ascii_alphabetic()
                && resto[1] == b':'
                && (resto[2] == b'\\' || resto[2] == b'/') =>
        {
            PathBuf::from(recorte(resto))
        }
        _ => canon.to_path_buf(),
    }
}

/// Resuelve `path` dentro de `root` y verifica que no sale del proyecto (§48).
///
/// `path` puede ser absoluta o relativa a `root` (§17). La resolución es
/// canónica: se resuelven `..` y symlinks, de modo que un symlink que apunta
/// fuera del proyecto se rechaza también (su canónica sale del root; §48:
/// path traversal). Si el destino aún no existe (caso Save As), se canoniza el
/// ancestro existente más cercano y se añade el resto de componentes, que
/// deben ser componentes normales: cualquier `..` o prefijo residual en la
/// parte inexistente se rechaza como [`FsError::PathOutsideProject`]. Un error
/// de E/S en la canonicalización es [`FsError::Io`].
pub fn resolve_in_project(root: &Path, path: &Path) -> Result<PathBuf, FsError> {
    let root_canon = canonicalize(root).map_err(|source| FsError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        // §17: las rutas relativas son contra el directorio del proyecto.
        root_canon.join(path)
    };
    let resolved = match canonicalize(&candidate) {
        Ok(resolved) => resolved,
        Err(source) if source.kind() == ErrorKind::NotFound => {
            // Destino aún inexistente (Save As): el resto se compone sobre el
            // ancestro existente más cercano (§48).
            resolve_missing_tail(&candidate)?
        }
        Err(source) => {
            return Err(FsError::Io {
                path: candidate,
                source,
            })
        }
    };
    if !resolved.starts_with(&root_canon) {
        return Err(FsError::PathOutsideProject { path: resolved });
    }
    Ok(resolved)
}

/// Canoniza el ancestro existente más cercano de un destino inexistente y
/// compone el resto de componentes encima (§48: Save As sobre un archivo nuevo).
///
/// Los componentes ausentes deben ser normales: un `..` o un prefijo residual
/// en la parte inexistente no tiene resolución segura y se rechaza como
/// [`FsError::PathOutsideProject`].
fn resolve_missing_tail(candidate: &Path) -> Result<PathBuf, FsError> {
    let mut tail: Vec<OsString> = Vec::new();
    let mut cursor = candidate;
    loop {
        match cursor.components().next_back() {
            Some(Component::Normal(name)) => tail.push(name.to_os_string()),
            _ => {
                return Err(FsError::PathOutsideProject {
                    path: candidate.to_path_buf(),
                })
            }
        }
        let Some(parent) = cursor.parent() else {
            return Err(FsError::PathOutsideProject {
                path: candidate.to_path_buf(),
            });
        };
        match canonicalize(parent) {
            Ok(base) => {
                let mut resolved = base;
                for name in tail.iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(source) if source.kind() == ErrorKind::NotFound => cursor = parent,
            Err(source) => {
                return Err(FsError::Io {
                    path: parent.to_path_buf(),
                    source,
                })
            }
        }
    }
}

/// Entradas directas de `dir`, marcando symlinks (§14.7).
///
/// Orden estable por nombre. `is_symlink` se obtiene con `symlink_metadata`
/// (no sigue el enlace) e `is_dir` con `metadata` (sigue el enlace al destino
/// real; solo informativo, §14.7). Un symlink roto se lista como entrada con
/// `is_dir = false`. Un directorio ilegible es [`FsError::Io`] (§48).
pub fn read_dir_entries(dir: &Path) -> Result<Vec<TreeEntry>, FsError> {
    let read = fs::read_dir(dir).map_err(|source| FsError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.map_err(|source| FsError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let is_symlink = fs::symlink_metadata(&path)
            .map_err(|source| FsError::Io {
                path: path.clone(),
                source,
            })?
            .file_type()
            .is_symlink();
        let is_dir = match fs::metadata(&path) {
            Ok(meta) => meta.is_dir(),
            // Symlink roto: se muestra, pero no tiene destino real (§14.7).
            Err(source) if source.kind() == ErrorKind::NotFound => false,
            Err(source) => return Err(FsError::Io { path, source }),
        };
        // §12.5: solo UTF-8, sin conversiones silenciosas.
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| FsError::NotUtf8 { path: path.clone() })?;
        entries.push(TreeEntry {
            path,
            name,
            is_symlink,
            is_dir,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Recorrido en profundidad del árbol desde `root` (§14.7, §48).
///
/// Preorden: las entradas de cada directorio van en orden por nombre y los
/// hijos de un subdirectorio inmediatamente después de él. El propio `root` no
/// se lista; si es un symlink a un directorio se canoniza al empezar y se
/// trata como directorio real. Los symlinks se listan como entradas
/// (`is_symlink = true`) pero nunca se desciende en carpetas alcanzadas vía
/// symlink (§14.7), con lo que los ciclos son imposibles por construcción;
/// además se registran las rutas canónicas de los directorios visitados para
/// no visitar ninguno dos veces (§48: evitar ciclos de symlink). El recorrido
/// es iterativo y siempre termina, sin recursión infinita ni panic.
pub fn walk_tree(root: &Path) -> Result<Vec<TreeEntry>, FsError> {
    let root = canonicalize(root).map_err(|source| FsError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let mut visited: HashSet<PathBuf> = HashSet::new();
    visited.insert(root.clone());
    let mut out = Vec::new();
    let mut stack: Vec<std::vec::IntoIter<TreeEntry>> = vec![read_dir_entries(&root)?.into_iter()];
    while !stack.is_empty() {
        let next = stack.last_mut().and_then(|frame| frame.next());
        let Some(entry) = next else {
            stack.pop();
            continue;
        };
        // Solo se desciende en directorios reales: nunca en carpetas
        // enlazadas mediante symlink (§14.7).
        if entry.is_dir && !entry.is_symlink {
            let canon = canonicalize(&entry.path).map_err(|source| FsError::Io {
                path: entry.path.clone(),
                source,
            })?;
            if visited.insert(canon) {
                stack.push(read_dir_entries(&entry.path)?.into_iter());
            }
        }
        out.push(entry);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Symlink portable para tests: en Windows distingue fichero de carpeta
    /// (`symlink_file`/`symlink_dir`) y devuelve `Err` cuando el sistema no
    /// permite crear symlinks (sin privilegio ni Developer Mode). Los tests
    /// tratan ese error como «este escenario no aplica en esta máquina» y
    /// terminan sin aserciones.
    #[cfg(unix)]
    fn symlink(original: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[cfg(windows)]
    fn symlink(original: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
        let original = original.as_ref();
        let link = link.as_ref();
        // El destino relativo se resuelve contra la carpeta del enlace.
        let base = link.parent().unwrap_or_else(|| Path::new(""));
        let es_carpeta = base.join(original).is_dir();
        if es_carpeta {
            std::os::windows::fs::symlink_dir(original, link)
        } else {
            std::os::windows::fs::symlink_file(original, link)
        }
    }

    /// Directorio temporal de test, eliminado al salir.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            Self(crate::test_dir::crear("opm-paths"))
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn proyecto() -> (TestDir, PathBuf) {
        let dir = TestDir::new();
        let root = dir.file("proyecto");
        fs::create_dir_all(&root).unwrap();
        (dir, root)
    }

    /// `sin_verbatim` (la normalización que `canonicalize` aplica al
    /// resultado de Windows): `\\?\C:\…` → `C:\…` y
    /// `\\?\UNC\servidor\…` → `\\servidor\…`; el resto de formas (planas,
    /// otros prefijos verbatim, Unix) no se toca. Comprobable en cualquier
    /// SO porque la transformación es sobre la grafía, no sobre el disco.
    #[test]
    fn sin_verbatim_normaliza_las_rutas_de_windows() {
        assert_eq!(
            sin_verbatim(Path::new(r"\\?\C:\proyecto\cap\f.md")),
            PathBuf::from(r"C:\proyecto\cap\f.md")
        );
        assert_eq!(
            sin_verbatim(Path::new(r"\\?\UNC\servidor\compartido\f.md")),
            PathBuf::from(r"\\servidor\compartido\f.md")
        );
        assert_eq!(
            sin_verbatim(Path::new(r"C:\proyecto")),
            PathBuf::from(r"C:\proyecto")
        );
        assert_eq!(
            sin_verbatim(Path::new(r"\\?\Volume{0123}")),
            PathBuf::from(r"\\?\Volume{0123}")
        );
        assert_eq!(
            sin_verbatim(Path::new("/tmp/proyecto")),
            PathBuf::from("/tmp/proyecto")
        );
    }

    #[test]
    fn resolve_rechaza_fuera_del_proyecto() {
        // §48: path traversal y rutas absolutas externas.
        let (_dir, root) = proyecto();
        assert!(matches!(
            resolve_in_project(&root, Path::new("../../etc/passwd")),
            Err(FsError::PathOutsideProject { .. })
        ));
        assert!(matches!(
            resolve_in_project(&root, Path::new("/etc/passwd")),
            Err(FsError::PathOutsideProject { .. })
        ));
    }

    #[test]
    fn resolve_rechaza_symlink_que_apunta_fuera_y_acepta_el_interno() {
        // §48: la canónica de un symlink que sale del proyecto también sale.
        let (_dir, root) = proyecto();
        fs::create_dir_all(root.join("interno")).unwrap();
        let fuera = TestDir::new();
        if symlink("interno", root.join("enlace_in")).is_err()
            || symlink(fuera.path(), root.join("enlace_out")).is_err()
        {
            return; // sin privilegio de symlinks (Windows): no aplica aquí
        }

        let esperado = canonicalize(&root.join("interno")).unwrap();
        assert_eq!(
            resolve_in_project(&root, &root.join("enlace_in")).unwrap(),
            esperado
        );
        assert!(matches!(
            resolve_in_project(&root, &root.join("enlace_out")),
            Err(FsError::PathOutsideProject { .. })
        ));
    }

    #[test]
    fn resolve_relativas_y_destino_inexistente_dentro() {
        // §17: relativas contra root; §48: Save As sobre un destino nuevo.
        let (_dir, root) = proyecto();
        fs::create_dir_all(root.join("docs")).unwrap();
        let raiz = canonicalize(&root).unwrap();

        let resuelto = resolve_in_project(&root, Path::new("docs/notas.md")).unwrap();
        assert!(resuelto.is_absolute());
        assert_eq!(resuelto, raiz.join("docs/notas.md"));

        // `..` seguro se resuelve con normalidad.
        assert_eq!(
            resolve_in_project(&root, Path::new("docs/../docs/notas.md")).unwrap(),
            raiz.join("docs/notas.md")
        );

        // Destino inexistente dentro del root (Save As) → Ok.
        assert_eq!(
            resolve_in_project(&root, Path::new("docs/nuevo/capitulo.md")).unwrap(),
            raiz.join("docs/nuevo/capitulo.md")
        );

        // Un `..` residual en la parte inexistente es imposible (§48).
        assert!(matches!(
            resolve_in_project(&root, Path::new("nuevo/../../fuera.md")),
            Err(FsError::PathOutsideProject { .. })
        ));
    }

    #[test]
    fn walk_tree_termina_con_ciclo_de_symlink() {
        // §14.7 / §48: el symlink se lista, pero el ciclo no se recorre.
        let (_dir, root) = proyecto();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("a/uno.md"), "x\n").unwrap();
        if symlink("..", root.join("a/enlace")).is_err()
            || symlink(".", root.join("ciclo")).is_err()
        {
            return; // sin privilegio de symlinks (Windows): no aplica aquí
        }

        let entries = walk_tree(&root).unwrap();
        let nombres: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(nombres, ["a", "enlace", "uno.md", "ciclo"]);

        // Sin entradas repetidas.
        let mut vistas = HashSet::new();
        for e in &entries {
            assert!(vistas.insert(e.path.clone()), "entrada repetida: {:?}", e);
        }

        let enlace = entries.iter().find(|e| e.name == "enlace").unwrap();
        assert!(enlace.is_symlink);
        let ciclo = entries.iter().find(|e| e.name == "ciclo").unwrap();
        assert!(ciclo.is_symlink);
    }

    #[test]
    fn walk_tree_no_desciende_en_carpeta_enlazada() {
        // §14.7: las carpetas enlazadas no se recorren recursivamente.
        let (_dir, root) = proyecto();
        fs::create_dir_all(root.join("real")).unwrap();
        fs::write(root.join("real/dentro.md"), "x\n").unwrap();
        if symlink("real", root.join("enlace_carpeta")).is_err() {
            return; // sin privilegio de symlinks (Windows): no aplica aquí
        }

        let entries = walk_tree(&root).unwrap();
        let nombres: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // `dentro.md` aparece una sola vez, vía `real`; el symlink, solo como
        // entrada (preorden por nombre en cada directorio).
        assert_eq!(nombres, ["enlace_carpeta", "real", "dentro.md"]);
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.name == "dentro.md")
                .map(|e| e.path.clone())
                .collect::<Vec<_>>(),
            [canonicalize(&root).unwrap().join("real/dentro.md")]
        );

        let enlace = entries.iter().find(|e| e.name == "enlace_carpeta").unwrap();
        assert!(enlace.is_symlink);
        assert!(enlace.is_dir); // informativo: el destino real es un dir (§14.7)
    }

    #[test]
    fn read_dir_entries_marca_symlinks_y_orden_estable() {
        // §14.7: symlinks de archivo y de carpeta, con orden estable por nombre.
        let (_dir, root) = proyecto();
        fs::create_dir_all(root.join("carpeta")).unwrap();
        fs::write(root.join("archivo.md"), "x\n").unwrap();
        if symlink("archivo.md", root.join("enlace_archivo")).is_err()
            || symlink("carpeta", root.join("enlace_carpeta")).is_err()
        {
            return; // sin privilegio de symlinks (Windows): no aplica aquí
        }

        let entries = read_dir_entries(&root).unwrap();
        assert_eq!(
            entries,
            vec![
                TreeEntry {
                    path: root.join("archivo.md"),
                    name: "archivo.md".into(),
                    is_symlink: false,
                    is_dir: false,
                },
                TreeEntry {
                    path: root.join("carpeta"),
                    name: "carpeta".into(),
                    is_symlink: false,
                    is_dir: true,
                },
                TreeEntry {
                    path: root.join("enlace_archivo"),
                    name: "enlace_archivo".into(),
                    is_symlink: true,
                    is_dir: false,
                },
                TreeEntry {
                    path: root.join("enlace_carpeta"),
                    name: "enlace_carpeta".into(),
                    is_symlink: true,
                    is_dir: true,
                },
            ]
        );
        // Orden estable entre llamadas.
        assert_eq!(read_dir_entries(&root).unwrap(), entries);
    }
}
