//! Project-relative path resolution, ported from
//! `backend/app/services/pipeline_paths.py` (working-tree ground truth).
//!
//! Three public helpers:
//! - [`resolve_pipeline_path`]: base-relative resolution with the guard rail
//!   of the Python backend — empty means `"."`, whitespace-only / NUL /
//!   absolute (POSIX *and* Windows drive/root/UNC forms, on every platform)
//!   are rejected, and the result must stay inside `root` after symlink
//!   resolution. Nonempty path text is preserved byte-for-byte otherwise: a
//!   POSIX filename containing backslashes is a legal single component and is
//!   never treated as a separator.
//! - [`resolve_project_path`]: root-relative convenience for the project
//!   operations (`project_tree.py` trims whitespace and maps blank to the
//!   root first, then applies the same containment rules with base = root).
//! - [`relative_project_path`]: `"../output/result.txt"`-style path from one
//!   working directory to a source output.

use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Expandir symlinks es acotado: el kernel también falla con ELOOP pasadas
/// ~40 expansiones, así que un bucle de symlinks termina con error en vez de
/// colgar la resolución.
const MAX_SYMLINK_EXPANSIONS: usize = 40;

/// Error de resolución de rutas de proyecto/pipeline (port de la excepción
/// Python `PipelinePathError`, subclase de `ValueError`). Cada mensaje es el
/// texto exacto de Python: las validaciones lo interpolan tras prefijos como
/// `Unsafe working directory: ` y sus tests pinchan subcadenas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// El `base` resuelto no pertenece al proyecto.
    BaseOutsideProject { base: String },
    /// Cadena compuesta solo por espacios en blanco.
    OnlyWhitespace,
    /// Byte nulo en la ruta.
    NullByte,
    /// Ruta absoluta (POSIX o forma de unidad/raíz/UNC de Windows, en
    /// cualquier plataforma).
    Absolute { path: String },
    /// Tras resolver symlinks la ruta sale del proyecto (path traversal).
    OutsideProject { path: String },
    /// La ruta no pudo resolverse (bucle de symlinks u error de E/S).
    Unresolvable { path: String },
    /// No existe ruta relativa entre dos destinos (p. ej. unidades distintas).
    NotRelative { path: String },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BaseOutsideProject { base } => {
                write!(
                    f,
                    "Pipeline path base is outside the current project: {base}"
                )
            }
            Self::OnlyWhitespace => {
                write!(f, "Pipeline path cannot contain only whitespace")
            }
            Self::NullByte => {
                write!(f, "Pipeline path contains an invalid null byte")
            }
            Self::Absolute { path } => {
                write!(
                    f,
                    "Pipeline path must be project-relative, not absolute: {path}"
                )
            }
            Self::OutsideProject { path } => {
                write!(
                    f,
                    "Pipeline path is outside the current project: {path}"
                )
            }
            Self::Unresolvable { path } => {
                write!(f, "Pipeline path cannot be resolved: {path}")
            }
            Self::NotRelative { path } => {
                write!(
                    f,
                    "Pipeline source path cannot be made relative to its target: {path}"
                )
            }
        }
    }
}

impl std::error::Error for PathError {}

/// Resuelve `requested` contra `base` dentro del proyecto `root` (port de
/// `pipeline_paths.py:resolve_pipeline_path`).
///
/// `base` suele ser el directorio de trabajo de un paso ya resuelto (o la
/// propia raíz). La ruta vacía significa `"."` (la raíz del `base`); todo lo
/// no vacío se conserva literal —espacios de sobra incluidos— y solo se
/// comprueba que, una vez resuelto (symlinks incluidos), no salga de `root`.
pub fn resolve_pipeline_path(
    root: &Path,
    requested: &str,
    base: &Path,
) -> Result<PathBuf, PathError> {
    let project_root = resolve_flexible(root)
        .map_err(|_| PathError::Unresolvable {
            path: root.display().to_string(),
        })?;
    let base_path = resolve_flexible(base).map_err(|_| PathError::Unresolvable {
        path: base.display().to_string(),
    })?;
    if !is_inside(&base_path, &project_root) {
        return Err(PathError::BaseOutsideProject {
            base: base_path.display().to_string(),
        });
    }

    let raw: &str = if requested.is_empty() {
        "."
    } else if requested.trim().is_empty() {
        return Err(PathError::OnlyWhitespace);
    } else {
        requested
    };
    if raw.contains('\0') {
        return Err(PathError::NullByte);
    }
    if is_absolute_form(raw) {
        return Err(PathError::Absolute {
            path: raw.to_owned(),
        });
    }

    let joined = base_path.join(raw);
    let resolved = resolve_flexible(&joined).map_err(|_| PathError::Unresolvable {
        path: joined.display().to_string(),
    })?;
    if !is_inside(&resolved, &project_root) {
        return Err(PathError::OutsideProject {
            path: raw.to_owned(),
        });
    }
    Ok(resolved)
}

/// Resuelve `requested` contra la raíz del proyecto (conveniencia raíz-relative
/// de las operaciones de `project_tree.py`).
///
/// Igual que en Python, el texto se recorta primero (`strip() or "."`): en
/// blanco o solo espacios resuelve a la propia raíz; después se aplican las
/// mismas reglas que [`resolve_pipeline_path`] con `base = root`.
pub fn resolve_project_path(root: &Path, requested: &str) -> Result<PathBuf, PathError> {
    let trimmed = requested.trim();
    let rel = if trimmed.is_empty() { "." } else { trimmed };
    resolve_pipeline_path(root, rel, root)
}

/// Resolución de una ruta GESTIONADA por la propia aplicación (workspaces de
/// pasos `steps/<id>/{input,work,output}`, `.pipeline-manager/**` — logs,
/// done, JSON de runtime — y `pipeline.project.json`).
///
/// Dos garantías, ambas obligatorias antes de leer, escribir o borrar:
/// 1. **Contención**: la ruta pasa [`resolve_project_path`] — tras resolver
///    symlinks no puede salir del proyecto (los errores de ruta llegan como
///    [`std::io::Error`] con el mensaje original de [`PathError`]).
/// 2. **Sin symlinks en el prefijo gestionado**: cada componente de la ruta
///    relativa se comprueba con `symlink_metadata` (sin seguir el enlace):
///    un enlace — colgante o no — en una ubicación gestionada es error,
///    nunca se sigue. Componentes aún inexistentes terminan la comprobación
///    (el resto de la ruta tampoco existe).
///
/// Devuelve la ruta ya contenida para usarla en el acceso al fichero.
pub fn resolve_managed_path(root: &Path, requested: &str) -> Result<PathBuf, std::io::Error> {
    let resolved = resolve_project_path(root, requested).map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string())
    })?;
    let base = resolve_flexible(root)?;
    let trimmed = requested.trim();
    let rel = if trimmed.is_empty() { "." } else { trimmed };
    let mut current = base;
    for component in Path::new(rel).components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(name) => current.push(name),
            // Inalcanzable: `resolve_project_path` ya rechazó formas absolutas.
            Component::Prefix(_) | Component::RootDir => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Managed path must be project-relative: {}", requested),
                ));
            }
        }
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "Managed path component is a symbolic link: {}",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            // Todavía no existe: el resto de la ruta tampoco puede existir.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
            Err(err) => return Err(err),
        }
    }
    Ok(resolved)
}

/// Ruta relativa (`"../output/result.txt"`, separador `/` siempre) desde el
/// directorio `from_dir` hasta `to`, ambos ya resueltos dentro de `root`
/// (port de `pipeline_paths.py:relative_project_path`).
///
/// `from_dir == to` resuelve a `"."`. Si alguna de las dos rutas (tras
/// resolverse) cae fuera de `root`, o no existe ruta relativa entre ellas,
/// devuelve [`PathError::OutsideProject`] / [`PathError::NotRelative`].
pub fn relative_project_path(
    root: &Path,
    from_dir: &Path,
    to: &Path,
) -> Result<String, PathError> {
    let project_root = resolve_flexible(root).map_err(|_| PathError::Unresolvable {
        path: root.display().to_string(),
    })?;
    let from = resolve_flexible(from_dir).map_err(|_| PathError::Unresolvable {
        path: from_dir.display().to_string(),
    })?;
    let target = resolve_flexible(to).map_err(|_| PathError::Unresolvable {
        path: to.display().to_string(),
    })?;
    if !is_inside(&from, &project_root) {
        return Err(PathError::OutsideProject {
            path: from.display().to_string(),
        });
    }
    if !is_inside(&target, &project_root) {
        return Err(PathError::OutsideProject {
            path: target.display().to_string(),
        });
    }
    relpath(&target, &from).ok_or_else(|| PathError::NotRelative {
        path: target.display().to_string(),
    })
}

/// Resolución no estricta de `path` a una ruta absoluta sin symlinks
/// (equivalente a `pathlib.Path.resolve()` con `strict=False` en Python):
///
/// - se siguen los symlinks existentes (con techo de expansiones para bucles);
/// - `..` se normaliza léxicamente sobre lo resuelto aunque componentes
///   intermedios no existan (`/raiz/falta/../x` → `/raiz/x`);
/// - la cola inexistente se conserva sin crear (el destino puede no existir:
///   es el caso «Guardar como» / «crear archivo»).
///
/// Los errores de E/S se devuelven como [`std::io::Error`]; los bucles de
/// symlinks como un error de E/S «too many levels of symbolic links».
pub(crate) fn resolve_flexible(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    let mut queue: VecDeque<Step> = steps_of(&absolute).into_iter().collect();
    let mut expansions: usize = 0;
    while let Some(step) = queue.pop_front() {
        match step {
            Step::Raw(piece) => out.push(piece),
            Step::Parent => {
                // Raíz/prefijo: no hay nada que recortar (pop es no-op).
                out.pop();
            }
            Step::Normal(name) => {
                let candidate = out.join(&name);
                let is_symlink = fs::symlink_metadata(&candidate)
                    .map(|meta| meta.file_type().is_symlink())
                    .unwrap_or(false);
                if !is_symlink {
                    out.push(name);
                    continue;
                }
                expansions += 1;
                if expansions > MAX_SYMLINK_EXPANSIONS {
                    // Bucle de symlinks (equivale al RuntimeError/`ELOOP` de
                    // `Path.resolve()` de Python).
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "too many levels of symbolic links",
                    ));
                }
                let target = fs::read_link(&candidate)?;
                if target.is_absolute() {
                    out = PathBuf::new();
                }
                let mut target_steps: Vec<Step> = steps_of(&target);
                while let Some(next) = target_steps.pop() {
                    queue.push_front(next);
                }
            }
        }
    }
    Ok(out)
}

/// Paso propietario de la cola de resolución: los `Component` de std prestan
/// del path original y no pueden sobrevivir a un `read_link`.
enum Step {
    /// Raíz (`/`) o prefijo de unidad (`C:`): se reaplica tal cual.
    Raw(std::ffi::OsString),
    /// `..` (léxico sobre lo resuelto).
    Parent,
    /// Componente normal.
    Normal(std::ffi::OsString),
}

/// Componentes de `path` como [`Step`]s, sin `.` (que Python también ignora).
fn steps_of(path: &Path) -> Vec<Step> {
    path.components()
        .filter_map(|component| match component {
            Component::RootDir | Component::Prefix(_) => {
                Some(Step::Raw(component.as_os_str().to_os_string()))
            }
            Component::CurDir => None,
            Component::ParentDir => Some(Step::Parent),
            Component::Normal(name) => Some(Step::Normal(name.to_os_string())),
        })
        .collect()
}

/// ¿Está `path` dentro (o igual a) `root`? Ambas rutas ya resueltas: la
/// comparación es por componentes y caza symlinks que escapan.
fn is_inside(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

/// Port de `_is_absolute_path`: absoluto POSIX **o** cualquier forma de
/// unidad/raíz/UNC de Windows, rechazada en todas las plataformas.
///
/// `PureWindowsPath(raw).is_absolute() || .drive || .root` — una letra antes
/// de los dos puntos es unidad (`C:rel` también cuenta), una barra inicial es
/// raíz (`\foo`, `\\srv\share`). Un nombre plano `foo\bar` no es absoluto: en
/// POSIX es un nombre con barra invertida literal (y debe seguirsiéndolo).
fn is_absolute_form(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.first() == Some(&b'/') {
        return true;
    }
    if bytes.first() == Some(&b'\\') {
        return true;
    }
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

/// `os.path.relpath(target, start)` en formato posix (`/` siempre, para que
/// el valor guardado en la config sea igual en todas las plataformas).
/// `None` si las raíces difieren (unidades distintas en Windows) o si algún
/// componente no es UTF-8.
fn relpath(target: &Path, start: &Path) -> Option<String> {
    fn components(path: &Path) -> Vec<Component<'_>> {
        path.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect()
    }
    let start_comps = components(start);
    let target_comps = components(target);
    let mut common = 0;
    while common < start_comps.len()
        && common < target_comps.len()
        && start_comps[common] == target_comps[common]
    {
        common += 1;
    }
    if common == 0 {
        // Raíces distintas (p. ej. `C:` frente a `D:`): no hay relativa.
        return None;
    }
    let mut parts: Vec<String> = vec![String::from(".."); start_comps.len() - common];
    for component in &target_comps[common..] {
        parts.push(component.as_os_str().to_str()?.to_owned());
    }
    if parts.is_empty() {
        return Some(String::from("."));
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project(tag: &str) -> PathBuf {
        let root = crate::test_dir::crear(&format!("opm-paths-{tag}"));
        fs::create_dir_all(root.join("wd")).unwrap();
        fs::create_dir_all(root.join("output")).unwrap();
        root
    }

    #[test]
    fn empty_means_root_and_blank_is_whitespace_only() {
        let root = project("empty");
        assert_eq!(
            resolve_pipeline_path(&root, "", &root).unwrap(),
            resolve_flexible(&root).unwrap()
        );
        let err = resolve_pipeline_path(&root, "   ", &root).unwrap_err();
        assert_eq!(err, PathError::OnlyWhitespace);
        // Mensaje verbatim de Python (lo interpolan las validaciones).
        assert_eq!(
            err.to_string(),
            "Pipeline path cannot contain only whitespace"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn nul_byte_is_rejected() {
        let root = project("nul");
        let err = resolve_pipeline_path(&root, "a\0b", &root).unwrap_err();
        assert_eq!(err, PathError::NullByte);
        assert_eq!(
            err.to_string(),
            "Pipeline path contains an invalid null byte"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn windows_absolute_forms_rejected_on_every_platform() {
        let root = project("winabs");
        for raw in [
            "D:\\outside",
            "D:/outside",
            "\\\\server\\share\\x",
            "\\rooted",
            "C:drive-relative",
            "C:",
        ] {
            let err = resolve_pipeline_path(&root, raw, &root).unwrap_err();
            assert!(
                matches!(err, PathError::Absolute { ref path } if path == raw),
                "{raw} -> {err:?}"
            );
            // El prefijo verbatim de Python lo pincha test_validation con "absolute".
            assert!(err.to_string().contains("absolute"), "{err}");
        }
        // POSIX absoluto también.
        assert!(matches!(
            resolve_pipeline_path(&root, "/etc/passwd", &root),
            Err(PathError::Absolute { .. })
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn backslash_names_stay_literal_on_posix() {
        let root = project("backslash");
        let resolved = resolve_pipeline_path(&root, "input\\file.txt", &root).unwrap();
        #[cfg(unix)]
        assert_eq!(resolved, root.join("input\\file.txt"));
        #[cfg(windows)]
        assert_eq!(resolved, root.join("input").join("file.txt"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn exact_text_preserved_including_padding() {
        let root = project("padding");
        let resolved = resolve_pipeline_path(&root, " input.txt ", &root).unwrap();
        assert_eq!(resolved, root.join(" input.txt "));
        // `.` componentes caen, `..` normaliza por lo que falta (probes Py 3.12).
        let resolved = resolve_pipeline_path(&root, "./x/./y", &root).unwrap();
        assert_eq!(resolved, root.join("x").join("y"));
        let resolved = resolve_pipeline_path(&root, "falta/../x", &root).unwrap();
        assert_eq!(resolved, root.join("x"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn traversal_outside_root_is_rejected() {
        let root = project("traversal");
        for raw in ["../escape", "a/../../escape", "wd/../../escape"] {
            let err = resolve_pipeline_path(&root, raw, &root).unwrap_err();
            assert!(
                matches!(err, PathError::OutsideProject { .. }),
                "{raw} -> {err:?}"
            );
            assert!(err.to_string().contains("outside"), "{err}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_and_loop_are_rejected() {
        let root = project("symlinks");
        let outside = crate::test_dir::crear("opm-paths-outside");
        fs::write(outside.join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
        let err = resolve_pipeline_path(&root, "link.txt", &root).unwrap_err();
        assert!(
            matches!(err, PathError::OutsideProject { .. }),
            "{err:?}"
        );

        std::os::unix::fs::symlink(root.join("b"), root.join("a")).unwrap();
        std::os::unix::fs::symlink(root.join("a"), root.join("b")).unwrap();
        let err = resolve_pipeline_path(&root, "a", &root).unwrap_err();
        assert!(matches!(err, PathError::Unresolvable { .. }), "{err:?}");
        // Mensaje verbatim (pinchan "cannot be resolved").
        assert!(err.to_string().contains("cannot be resolved"), "{err}");

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn base_must_stay_inside_the_project() {
        let root = project("base");
        let outside = crate::test_dir::crear("opm-paths-base-outside");
        let err = resolve_pipeline_path(&root, "x", &outside).unwrap_err();
        assert!(
            matches!(err, PathError::BaseOutsideProject { .. }),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn base_relative_resolution_from_a_working_directory() {
        let root = project("base-rel");
        let wd = resolve_flexible(&root.join("wd")).unwrap();
        assert_eq!(
            resolve_pipeline_path(&root, "../output/x", &wd).unwrap(),
            root.join("output").join("x")
        );
        assert_eq!(
            resolve_pipeline_path(&root, "local.txt", &wd).unwrap(),
            root.join("wd").join("local.txt")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn project_path_trims_and_maps_blank_to_root() {
        let root = project("project-path");
        let resolved_root = resolve_flexible(&root).unwrap();
        assert_eq!(
            resolve_project_path(&root, " Input/x ").unwrap(),
            root.join("Input").join("x")
        );
        assert_eq!(resolve_project_path(&root, "").unwrap(), resolved_root);
        assert_eq!(resolve_project_path(&root, "   ").unwrap(), resolved_root);
        assert!(matches!(
            resolve_project_path(&root, "../escape"),
            Err(PathError::OutsideProject { .. })
        ));
        assert!(matches!(
            resolve_project_path(&root, "C:\\evil"),
            Err(PathError::Absolute { .. })
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_project_path_shape() {
        let root = project("relshape");
        let wd = root.join("analysis");
        let result = relative_project_path(&root, &wd, &root.join("output/result.txt")).unwrap();
        assert_eq!(result, "../output/result.txt");
        // Mismo directorio → ".".
        assert_eq!(
            relative_project_path(&root, &wd, &wd).unwrap(),
            "."
        );
        // Hacia abajo, sin "..".
        assert_eq!(
            relative_project_path(&root, &root, &wd.join("local.txt")).unwrap(),
            "analysis/local.txt"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn relative_project_path_keeps_posix_backslashes() {
        let root = project("relslash");
        let produce = root.join("produce");
        let source = resolve_pipeline_path(&root, "result\\file.txt", &produce).unwrap();
        assert_eq!(
            relative_project_path(&root, &root.join("consume"), &source).unwrap(),
            "../produce/result\\file.txt"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn relative_project_path_rejects_paths_outside_root() {
        let root = project("relin");
        let outside = crate::test_dir::crear("opm-paths-relin-outside");
        let err = relative_project_path(&root, &root.join("wd"), &outside).unwrap_err();
        assert!(
            matches!(err, PathError::OutsideProject { .. }),
            "{err:?}"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }
}
