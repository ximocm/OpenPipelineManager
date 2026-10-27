//! Project operations, ported from `backend/app/services/project_tree.py`
//! (working-tree ground truth) plus the batch-import guarantees of the upload
//! API (`backend/app/api/routes.py` + `test_file_upload_api.py`).
//!
//! Every path that enters a project goes through
//! [`crate::pipeline_paths::resolve_project_path`]: text is preserved as the
//! user typed it and nothing may escape the project root after symlink
//! resolution. The error kinds mirror the Python exception classes the HTTP
//! layer mapped to status codes: [`OpsError::AlreadyExists`] (FileExistsError,
//! 409), [`OpsError::NotFound`] (FileNotFoundError, 404) and the rule
//! violations that were ValueError (400).

use crate::paths::TreeEntry;
use crate::pipeline_paths::{resolve_flexible, resolve_managed_path, resolve_project_path, PathError};
use crate::write_atomic;
use std::cell::Cell;
use std::fmt;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Fichero que marca la raíz como proyecto OPM (`project_tree.py`).
pub const PROJECT_CONFIG_FILENAME: &str = "project.config.json";

/// Carpetas que las operaciones de árbol nunca listan ni borran
/// (`project_tree.py:IGNORED_NAMES`).
pub const IGNORED_NAMES: &[&str] = &[
    ".pipeline-manager",
    ".git",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    ".venv",
];

/// Carpetas por defecto del scaffold (`DEFAULT_PROJECT_DIRS`).
const DEFAULT_PROJECT_DIRS: &[&str] = &["Input", "steps", "outputs"];

/// Sufijos con vista previa de imagen (`IMAGE_SUFFIXES`, sin punto).
const IMAGE_SUFFIXES: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg"];

/// Profundidad máxima del árbol de proyecto (`build_tree(max_depth=5)`).
const MAX_TREE_DEPTH: usize = 5;

/// Contenido del `config.txt` del scaffold, literal de Python.
const CONFIG_TXT: &str = "Open Pipeline Manager project\n\n\
    Input files: Input/\n\
    Step workspaces: steps/<step-id>/\n\
    Shared outputs: outputs/\n";

/// Error de una operación de proyecto (los tipos Python se agrupan aquí:
/// `ValueError` → las variantes de regla, `FileExistsError` →
/// [`OpsError::AlreadyExists`], `FileNotFoundError` → [`OpsError::NotFound`],
/// resto de `OSError` → [`OpsError::Io`]).
#[derive(Debug)]
pub enum OpsError {
    /// Regla de rutas violada (path traversal, absoluto, NUL…).
    Path(PathError),
    /// El destino ya existe: nunca sobrescritura (Python FileExistsError).
    AlreadyExists { path: PathBuf },
    /// La ruta no existe (Python FileNotFoundError).
    NotFound { path: PathBuf },
    /// Operación prohibida sobre la raíz del proyecto (Python ValueError).
    RootForbidden { operation: &'static str },
    /// Nombre que no es un único segmento de archivo/carpeta.
    NotSingleSegment,
    /// El destino de un movimiento/importación no es un directorio.
    TargetNotDirectory,
    /// Intento de mover un directorio dentro de sí mismo.
    DirectoryIntoItself,
    /// Escritura sobre algo que no es un fichero.
    TargetNotAFile,
    /// Borrado de una carpeta gestionada/ignorada.
    IgnoredName { name: String },
    /// La carpeta no contiene `project.config.json`.
    NotAProject,
    /// Nombre de entrada que no es UTF-8 válido (§12.5: error claro, sin
    /// conversiones silenciosas) — solo lo levanta el listado del árbol.
    NotUtf8 { path: PathBuf },
    /// Error de E/S del sistema.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for OpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(err) => write!(f, "{err}"),
            Self::AlreadyExists { path } => {
                write!(f, "Path already exists: {}", path.display())
            }
            Self::NotFound { path } => write!(f, "Path not found: {}", path.display()),
            Self::RootForbidden { operation } => {
                write!(f, "Cannot {operation} the project root")
            }
            Self::NotSingleSegment => {
                write!(f, "Name must be a single file or folder name")
            }
            Self::TargetNotDirectory => write!(f, "Target is not a directory"),
            Self::DirectoryIntoItself => write!(f, "Cannot move a directory into itself"),
            Self::TargetNotAFile => write!(f, "Target is not a file"),
            Self::IgnoredName { name } => {
                write!(f, "Cannot delete managed or ignored folder: {name}")
            }
            Self::NotAProject => {
                write!(
                    f,
                    "Folder is not an Open Pipeline Manager project: missing {PROJECT_CONFIG_FILENAME}"
                )
            }
            Self::NotUtf8 { path } => {
                write!(
                    f,
                    "{} is not valid UTF-8 (only UTF-8 names are supported, §12.5)",
                    path.display()
                )
            }
            Self::Io { path, source } => write!(f, "I/O error on {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for OpsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Path(err) => Some(err),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<PathError> for OpsError {
    fn from(err: PathError) -> Self {
        Self::Path(err)
    }
}

/// Sanea un identificador de paso/archivo (`safe_segment` de Python):
/// espacios → `_`, caracteres disallowed → `_`, colapsa `_`, recorta `._` y
/// usa `project` como fallback.
pub fn safe_segment(value: &str) -> String {
    let spaced = value.split_whitespace().collect::<Vec<_>>().join("_");
    let replaced: String = spaced
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut collapsed = replaced.replace("__", "_");
    while collapsed.contains("__") {
        collapsed = collapsed.replace("__", "_");
    }
    let trimmed = collapsed.trim_matches(|c| c == '.' || c == '_');
    if trimmed.is_empty() {
        String::from("project")
    } else {
        trimmed.to_owned()
    }
}

/// Crea la estructura de un proyecto en `root` (`ensure_project_scaffold`):
/// `Input/`, `steps/`, `outputs/`, `project.config.json` (solo si no existe;
/// `name` es el valor guardado) y `config.txt` (solo si no existe).
pub fn ensure_project_scaffold(root: &Path, name: &str) -> Result<(), OpsError> {
    let resolved = resolve_flexible(root)
        .map_err(|source| OpsError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    fs::create_dir_all(&resolved).map_err(|source| map_create_err(&resolved, source))?;
    for dirname in DEFAULT_PROJECT_DIRS {
        ensure_dir(&resolved.join(dirname))?;
    }
    let config = resolved.join(PROJECT_CONFIG_FILENAME);
    if !config.exists() {
        write_atomic(&config, scaffold_json(name).as_bytes()).map_err(|source| {
            OpsError::Io {
                path: config.clone(),
                source,
            }
        })?;
    }
    let config_txt = resolved.join("config.txt");
    if !config_txt.exists() {
        write_atomic(&config_txt, CONFIG_TXT.as_bytes()).map_err(|source| {
            OpsError::Io {
                path: config_txt.clone(),
                source,
            }
        })?;
    }
    Ok(())
}

/// Valida que `root` es un proyecto OPM (`validate_project_folder`): carpeta
/// existente con `project.config.json` dentro.
pub fn validate_project_folder(root: &Path) -> Result<(), OpsError> {
    let resolved = resolve_flexible(root)
        .map_err(|source| OpsError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    match fs::metadata(&resolved) {
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(OpsError::NotFound { path: resolved });
        }
        Err(source) => {
            return Err(OpsError::Io {
                path: resolved,
                source,
            });
        }
        Ok(meta) if !meta.is_dir() => return Err(OpsError::NotFound { path: resolved }),
        Ok(_) => {}
    }
    if !resolved.join(PROJECT_CONFIG_FILENAME).exists() {
        return Err(OpsError::NotAProject);
    }
    Ok(())
}

/// `steps/<safe_segment(step_id)>/{input,work,output}`
/// (`ensure_step_folders`, con `safe_segment` aplicado como en Python).
///
/// F2: cada ruta gestionada pasa [`resolve_managed_path`] — contención
/// dentro del proyecto Y prefijo sin symlinks (un `steps/<id>` enlazado
/// hacia fuera nunca se sigue; el error se devuelve antes de crear nada).
pub fn ensure_step_folders(root: &Path, step_id: &str) -> Result<(), OpsError> {
    let resolved = resolve_flexible(root)
        .map_err(|source| OpsError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    let segment = safe_segment(step_id);
    let step_rel = format!("steps/{segment}");
    let check = |rel: &str| {
        resolve_managed_path(&resolved, rel).map_err(|source| OpsError::Io {
            path: resolved.join(rel),
            source,
        })
    };
    check(&step_rel)?;
    for dirname in ["input", "work", "output"] {
        check(&format!("{step_rel}/{dirname}"))?;
    }
    let steps_root = resolved.join("steps");
    ensure_dir(&steps_root)?;
    let step_root = steps_root.join(&segment);
    for dirname in ["input", "work", "output"] {
        ensure_dir_all(&step_root.join(dirname))?;
    }
    Ok(())
}

/// Crea un fichero (`directory=false`) o una carpeta (`directory=true`) en
/// `requested` (`create_project_path`): nunca sobrescribe — si ya existe,
/// [`OpsError::AlreadyExists`].
///
/// F5: la publicación es EXCLUSIVA (`create_new`, sin ventana de
/// `rename-over` sobre la comprobación `exists()`); el reemplazo atómico de
/// [`crate::write_atomic`] sigue reservado a los flujos de sobrescritura
/// legítimos (`write_project_file`).
pub fn create_project_path(
    root: &Path,
    requested: &str,
    content: &str,
    directory: bool,
) -> Result<PathBuf, OpsError> {
    let candidate = resolve_project_path(root, requested)?;
    if candidate.exists() {
        return Err(OpsError::AlreadyExists {
            path: candidate,
        });
    }
    if let Some(parent) = candidate.parent() {
        ensure_dir_all(parent)?;
    }
    if directory {
        create_dir_exclusive(&candidate)?;
    } else {
        create_file_exclusive(&candidate, content)?;
    }
    Ok(candidate)
}

/// Creación exclusiva de fichero: `create_new` puro — un destino que aparezca
/// tras el chequeo (`exists()`) provoca [`OpsError::AlreadyExists`] con sus
/// datos intactos; jamás truncate/reemplazo.
fn create_file_exclusive(candidate: &Path, content: &str) -> Result<(), OpsError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(candidate)
        .map_err(|source| map_create_err(candidate, source))?;
    file.write_all(content.as_bytes())
        .map_err(|source| OpsError::Io {
            path: candidate.to_path_buf(),
            source,
        })?;
    Ok(())
}

/// Creación exclusiva de carpeta: `mkdir` atómico (`create_dir` falla con
/// EEXIST si el destino aparece) — sin `rename` ni sobrescritura silenciosa.
fn create_dir_exclusive(candidate: &Path) -> Result<(), OpsError> {
    fs::create_dir(candidate).map_err(|source| map_create_err(candidate, source))
}

/// Sobrescribe el contenido de un fichero de texto
/// (`write_project_file`): solo ficheros — un directorio es
/// [`OpsError::TargetNotAFile`] (Python ValueError).
pub fn write_project_file(root: &Path, requested: &str, content: &str) -> Result<PathBuf, OpsError> {
    let candidate = resolve_project_path(root, requested)?;
    if candidate.exists() && !candidate.is_file() {
        return Err(OpsError::TargetNotAFile);
    }
    if let Some(parent) = candidate.parent() {
        ensure_dir_all(parent)?;
    }
    write_atomic(&candidate, content.as_bytes()).map_err(|source| OpsError::Io {
        path: candidate.clone(),
        source,
    })?;
    Ok(candidate)
}

/// Renombra una entrada dentro de su propio directorio
/// (`rename_project_path`): un único segmento de nombre, raíz prohibida y
/// [`OpsError::AlreadyExists`] si el destino existe (409).
pub fn rename_project_path(
    root: &Path,
    requested: &str,
    new_name: &str,
) -> Result<PathBuf, OpsError> {
    let source = resolve_project_path(root, requested)?;
    let resolved_root = resolve_flexible(root).map_err(|source| OpsError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if source == resolved_root {
        return Err(OpsError::RootForbidden {
            operation: "rename",
        });
    }
    if new_name.contains('/')
        || new_name.contains('\\')
        || new_name.contains('\0')
        || new_name.trim().is_empty()
    {
        return Err(OpsError::NotSingleSegment);
    }
    let parent = source
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| OpsError::Io {
            path: source.clone(),
            source: std::io::Error::new(ErrorKind::InvalidInput, "source has no parent"),
        })?;
    let target = parent.join(new_name.trim());
    if target.exists() {
        return Err(OpsError::AlreadyExists { path: target });
    }
    fs::rename(&source, &target).map_err(|err| map_rename_err(&source, err))?;
    Ok(target)
}

/// Mueve una entrada dentro de otro directorio del proyecto
/// (`move_project_path`): destino directorio, sin moverse dentro de sí
/// mismo, [`OpsError::AlreadyExists`] si el nombre ya existe (409).
pub fn move_project_path(
    root: &Path,
    source_path: &str,
    target_directory: &str,
) -> Result<PathBuf, OpsError> {
    let source = resolve_project_path(root, source_path)?;
    let destination_dir = resolve_project_path(root, target_directory)?;
    let resolved_root = resolve_flexible(root).map_err(|source| OpsError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if source == resolved_root {
        return Err(OpsError::RootForbidden { operation: "move" });
    }
    if !destination_dir.is_dir() {
        return Err(OpsError::TargetNotDirectory);
    }
    if source.is_dir() && destination_dir.starts_with(&source) {
        return Err(OpsError::DirectoryIntoItself);
    }
    let file_name = source
        .file_name()
        .map(|name| name.to_os_string())
        .ok_or_else(|| OpsError::Io {
            path: source.clone(),
            source: std::io::Error::new(ErrorKind::InvalidInput, "source has no file name"),
        })?;
    let target = destination_dir.join(file_name);
    if target.exists() {
        return Err(OpsError::AlreadyExists { path: target });
    }
    fs::rename(&source, &target).map_err(|err| map_rename_err(&source, err))?;
    Ok(target)
}

/// Borra una entrada (`delete_project_path`): raíz prohibida, carpetas
/// gestionadas/ignoradas ([`OpsError::IgnoredName`]) y recursivo para
/// directorios; lo que no existe es [`OpsError::NotFound`] (404).
pub fn delete_project_path(root: &Path, requested: &str) -> Result<(), OpsError> {
    let candidate = resolve_project_path(root, requested)?;
    let resolved_root = resolve_flexible(root).map_err(|source| OpsError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if candidate == resolved_root {
        return Err(OpsError::RootForbidden { operation: "delete" });
    }
    if let Some(name) = candidate.file_name().map(|name| name.to_string_lossy()) {
        if IGNORED_NAMES.contains(&name.as_ref()) {
            return Err(OpsError::IgnoredName {
                name: name.into_owned(),
            });
        }
    }
    match fs::metadata(&candidate) {
        Err(err) if err.kind() == ErrorKind::NotFound => {
            Err(OpsError::NotFound { path: candidate })
        }
        Err(source) => Err(OpsError::Io {
            path: candidate,
            source,
        }),
        Ok(meta) if meta.is_dir() => {
            fs::remove_dir_all(&candidate).map_err(|source| map_remove_err(&candidate, source))
        }
        Ok(_) => fs::remove_file(&candidate).map_err(|source| map_remove_err(&candidate, source)),
    }
}

/// Un fichero de una importación por lotes: `source` (ruta local) se copia a
/// `name` dentro del directorio destino.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSource {
    pub source: PathBuf,
    pub name: String,
}

/// Valor por defecto de [`case_insensitive_destinations`]: `true` donde el
/// filesystem por defecto del SO es case-insensitive (Windows y macOS).
const DEFAULT_CASE_INSENSITIVE_DESTINATIONS: bool = cfg!(windows) || cfg!(target_os = "macos");

thread_local! {
    /// Solo pruebas: sobrescribe el valor de plataforma de
    /// [`case_insensitive_destinations`] EN EL HILO ACTUAL (`None` =
    /// plataforma). Estado por hilo: los tests en paralelo no se pisan.
    static CASE_INSENSITIVE_OVERRIDE: Cell<Option<bool>> = Cell::new(None);
}

/// ¿El filesystem de destino del lote trata `REPORT.txt` y `report.txt`
/// como el mismo nombre? Valor de plataforma por defecto, salvo que las
/// pruebas lo fuercen con [`force_case_insensitive_destinations`].
fn case_insensitive_destinations() -> bool {
    CASE_INSENSITIVE_OVERRIDE
        .with(Cell::get)
        .unwrap_or(DEFAULT_CASE_INSENSITIVE_DESTINATIONS)
}

/// Solo para pruebas (`#[doc(hidden)]`): fuerza la rama case-sensitivity del
/// preflight de [`import_files`] en el HILO actual; `None` restaura el
/// valor de plataforma.
#[doc(hidden)]
pub fn force_case_insensitive_destinations(value: Option<bool>) {
    CASE_INSENSITIVE_OVERRIDE.with(|cell| cell.set(value));
}

/// Clave de colisión de un destino del lote: cada segmento sin espacios
/// exteriores SIEMPRE (p. ej. `" duplicate.txt "` y `"duplicate.txt"` son el
/// mismo destino, el `trim` del preflight ya normaliza el nombre) y, solo en
/// un filesystem de destino case-insensitive, en minúsculas (en Linux
/// `report.txt` y `REPORT.txt` son destinos legítimamente distintos).
fn destination_fold_key(destination: &Path) -> String {
    let key = destination
        .to_string_lossy()
        .split(|c| c == '/' || c == '\\')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if case_insensitive_destinations() {
        key.to_lowercase()
    } else {
        key
    }
}

/// ¿Colisionan `a` y `b` bajo la semántica de NOMBRADO del filesystem de
/// destino (P2: `report.txt` vs `REPORT.txt` pasan la comparación exacta y
/// chocarían después en la reserva exclusiva con un error genérico en vez
/// del rechazo claro de duplicado del preflight).
fn destinations_conflict(a: &Path, b: &Path) -> bool {
    destination_fold_key(a) == destination_fold_key(b)
}

/// Importación por lotes (`import_files`, equivalente desktop de la API de
/// subida en lote):
///
/// 1. preflight de TODOS los destinos — nombre de un solo segmento, destino
///    inexistente (un symlink cuenta como existente, también el colgado) y
///    sin duplicados entre lotes (incluidas variantes con espacios, que
///    normaliza `trim`, y —si el filesystem de destino es case-insensitive—
///    destinos que solo colisionan en mayúsculas, p. ej. `report.txt` vs
///    `REPORT.txt`: [`destinations_conflict`]);
/// 2. reserva exclusiva (`create_new`) de cada destino: red de seguridad
///    frente a carreras posteriores al preflight;
/// 3. si algo falla, solo se borran los ficheros creados por esta llamada
///    (rollback con comprobación de identidad); ningún destino preexistente
///    se toca.
pub fn import_files(
    root: &Path,
    target_directory: &str,
    files: &[ImportSource],
) -> Result<Vec<PathBuf>, OpsError> {
    let destination_dir = resolve_project_path(root, target_directory)?;
    if !destination_dir.is_dir() {
        return Err(OpsError::TargetNotDirectory);
    }

    // Preflight: validar todo y leer todo antes de crear nada.
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut prepared: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(files.len());
    for file in files {
        let name = file.name.as_str();
        if name.contains('/')
            || name.contains('\\')
            || name.contains('\0')
            || name.trim().is_empty()
        {
            return Err(OpsError::NotSingleSegment);
        }
        let target = destination_dir.join(name.trim());
        if target.exists() || target.is_symlink() {
            return Err(OpsError::AlreadyExists { path: target });
        }
        // Duplicados del lote: variantes de espacios siempre y —donde el
        // filesystem de destino es case-insensitive— destinos que solo
        // colisionan en mayúsculas (P2): el mismo error que un destino ya
        // existente, y ANTES de escribir nada.
        if seen
            .iter()
            .any(|previous| destinations_conflict(previous, &target))
        {
            return Err(OpsError::AlreadyExists { path: target });
        }
        seen.push(target.clone());
        let bytes = fs::read(&file.source).map_err(|source| OpsError::Io {
            path: file.source.clone(),
            source,
        })?;
        prepared.push((target, bytes));
    }

    // Reserva exclusiva + escritura, con rollback de lo creado por esta llamada.
    let mut created: Vec<(PathBuf, Option<(u64, u64)>)> = Vec::new();
    let mut handles: Vec<(PathBuf, fs::File)> = Vec::new();
    let mut result: Result<(), OpsError> = Ok(());
    for (target, _) in &prepared {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
        {
            Ok(handle) => {
                let identity = handle.metadata().ok().and_then(|meta| file_identity(&meta));
                created.push((target.clone(), identity));
                handles.push((target.clone(), handle));
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                result = Err(OpsError::AlreadyExists {
                    path: target.clone(),
                });
                break;
            }
            Err(source) => {
                result = Err(OpsError::Io {
                    path: target.clone(),
                    source,
                });
                break;
            }
        }
    }
    if result.is_ok() {
        for ((target, bytes), (_, handle)) in prepared.iter().zip(handles.iter_mut()) {
            if let Err(source) = handle.write_all(bytes) {
                result = Err(OpsError::Io {
                    path: target.clone(),
                    source,
                });
                break;
            }
        }
    }
    drop(handles);
    if let Err(err) = result {
        rollback_created(&created);
        return Err(err);
    }
    Ok(prepared.into_iter().map(|(target, _)| target).collect())
}

/// Identidad (dispositivo, inodo) de un fichero abierto; `None` donde el SO
/// no la expone — en ese caso el rollback solo respeta los enlaces simbólicos
/// (nunca borra un destino que se haya convertido en symlink).
fn file_identity(meta: &fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// Borra únicamente los ficheros creados por esta llamada que siguen siendo
/// los mismos (rollback de `upload_project_files`).
fn rollback_created(created: &[(PathBuf, Option<(u64, u64)>)]) {
    for (path, identity) in created {
        let Ok(meta) = fs::symlink_metadata(path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if let (Some(ours), Some(current)) = (identity, file_identity(&meta)) {
            if *ours != current {
                continue;
            }
        }
        let _ = fs::remove_file(path);
    }
}

/// Árbol de proyecto (`TreeNode` de `app.models.state`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeKind {
    File,
    Directory,
}

/// Nodo de [`build_tree`]: `path` es relativo a la raíz con `/` y la raíz se
/// etiqueta `"."`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub name: String,
    pub path: String,
    pub kind: TreeKind,
    pub children: Vec<TreeNode>,
}

/// Árbol completo del proyecto (`build_tree`): profundidad máxima
/// [`MAX_TREE_DEPTH`], sin [`IGNORED_NAMES`], carpetas antes que ficheros y
/// orden case-insensitive; la raíz se etiqueta `"."`.
pub fn build_tree(root: &Path) -> Result<TreeNode, OpsError> {
    let resolved = resolve_flexible(root)
        .map_err(|source| OpsError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    walk_node(&resolved, &resolved, 0)
}

/// Hijos directos de `path` bajo la política ÚNICA del árbol: nombres en
/// [`IGNORED_NAMES`] fuera, carpetas antes que ficheros y orden
/// case-insensitive por nombre (los nombres no UTF-8 ordenan con su forma
/// lossy, como los ordena `walk_node`). Devuelve las rutas YA en ese orden.
/// Es la fuente de verdad compartida por [`build_tree`] (recorrido completo)
/// y [`list_tree_children`] (listado perezoso de un nivel).
fn policy_children(path: &Path) -> Result<Vec<PathBuf>, OpsError> {
    let read = fs::read_dir(path).map_err(|source| OpsError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut keyed: Vec<(bool, String, PathBuf)> = Vec::new();
    for entry in read {
        let entry = entry.map_err(|source| OpsError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let entry_name = entry.file_name().to_string_lossy().into_owned();
        if IGNORED_NAMES.contains(&entry_name.as_str()) {
            continue;
        }
        let child = entry.path();
        let child_is_file = fs::metadata(&child).map(|m| m.is_file()).unwrap_or(false);
        keyed.push((child_is_file, entry_name.to_lowercase(), child));
    }
    keyed.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    Ok(keyed.into_iter().map(|(_, _, path)| path).collect())
}

/// Listado perezoso de UN nivel del árbol para el explorador: los hijos
/// directos de `dir` bajo la MISMA política de [`build_tree`] (fuente única:
/// [`IGNORED_NAMES`] fuera, carpetas antes que ficheros, orden
/// case-insensitive y tope de profundidad [`MAX_TREE_DEPTH`] — un `dir` en la
/// profundidad máxima se lista sin hijos, como en `build_tree`). `dir` debe
/// estar dentro de `root`; los nombres no UTF-8 son
/// [`OpsError::NotUtf8`] (§12.5: sin conversiones silenciosas).
pub fn list_tree_children(root: &Path, dir: &Path) -> Result<Vec<TreeEntry>, OpsError> {
    let resolved_root = resolve_flexible(root).map_err(|source| OpsError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let resolved_dir = resolve_flexible(dir).map_err(|source| OpsError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    if resolved_dir != resolved_root && !resolved_dir.starts_with(&resolved_root) {
        return Err(PathError::OutsideProject {
            path: dir.display().to_string(),
        }
        .into());
    }
    let depth = resolved_dir
        .strip_prefix(&resolved_root)
        .map(|relative| relative.components().count())
        .unwrap_or(0);
    if depth >= MAX_TREE_DEPTH {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for child in policy_children(&resolved_dir)? {
        // `read_dir` siempre entrega entradas con nombre (nunca `..`).
        let Some(file_name) = child.file_name() else {
            continue;
        };
        let name = file_name
            .to_os_string()
            .into_string()
            .map_err(|_| OpsError::NotUtf8 { path: child.clone() })?;
        let is_symlink = fs::symlink_metadata(&child)
            .map_err(|source| OpsError::Io {
                path: child.clone(),
                source,
            })?
            .file_type()
            .is_symlink();
        let is_dir = match fs::metadata(&child) {
            Ok(meta) => meta.is_dir(),
            // Symlink roto: se lista, pero no tiene destino real (§14.7).
            Err(source) if source.kind() == ErrorKind::NotFound => false,
            Err(source) => return Err(OpsError::Io { path: child.clone(), source }),
        };
        entries.push(TreeEntry {
            path: child,
            name,
            is_symlink,
            is_dir,
        });
    }
    Ok(entries)
}

fn walk_node(path: &Path, root: &Path, depth: usize) -> Result<TreeNode, OpsError> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let node_path = if path == root {
        String::from(".")
    } else {
        relative_posix(path, root)
    };
    let is_file = fs::metadata(path).map(|meta| meta.is_file()).unwrap_or(false);
    if is_file {
        return Ok(TreeNode {
            name,
            path: node_path,
            kind: TreeKind::File,
            children: Vec::new(),
        });
    }
    let mut children = Vec::new();
    if depth < MAX_TREE_DEPTH {
        for child in policy_children(path)? {
            children.push(walk_node(&child, root, depth + 1)?);
        }
    }
    Ok(TreeNode {
        name,
        path: node_path,
        kind: TreeKind::Directory,
        children,
    })
}

/// Vista previa de fichero (`FilePreview` de Python).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePreview {
    /// Ruta inexistente, fuera del proyecto o que no es un fichero.
    Missing,
    /// Texto UTF-8 sin NUL (CRLF normalizado a LF).
    Text { content: String },
    /// Imagen: bytes crudos y tipo media (`png/jpg/jpeg/gif/webp/svg`).
    Image {
        bytes: Vec<u8>,
        media: &'static str,
    },
    /// Binario (NUL o UTF-8 inválido).
    Binary,
}

/// Clasifica `requested` para la vista de ficheros (`preview_file`): cualquier
/// problema de ruta resuelve a [`FilePreview::Missing`] como en Python.
pub fn preview_file(root: &Path, requested: &str) -> Result<FilePreview, OpsError> {
    let candidate = match resolve_project_path(root, requested) {
        Ok(path) => path,
        Err(_) => return Ok(FilePreview::Missing),
    };
    match fs::metadata(&candidate) {
        Err(_) => return Ok(FilePreview::Missing),
        Ok(meta) if !meta.is_file() => return Ok(FilePreview::Missing),
        Ok(_) => {}
    }
    let extension = candidate
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_lowercase);
    if let Some(ext) = extension.as_deref() {
        if IMAGE_SUFFIXES.contains(&ext) {
            let bytes = fs::read(&candidate).map_err(|source| OpsError::Io {
                path: candidate.clone(),
                source,
            })?;
            return Ok(FilePreview::Image {
                bytes,
                media: media_type(ext),
            });
        }
    }
    let raw = fs::read(&candidate).map_err(|source| OpsError::Io {
        path: candidate.clone(),
        source,
    })?;
    if raw.contains(&0) {
        return Ok(FilePreview::Binary);
    }
    match String::from_utf8(raw) {
        Ok(content) => Ok(FilePreview::Text {
            content: content.replace("\r\n", "\n"),
        }),
        Err(_) => Ok(FilePreview::Binary),
    }
}

/// Tipo media de `mimetypes.guess_type` para los sufijos de imagen.
fn media_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Entrada de los listadores de carpetas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: PathBuf,
}

/// Resultado de [`list_directories`] (selector de carpetas).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryList {
    pub path: PathBuf,
    pub parent: Option<PathBuf>,
    pub directories: Vec<DirectoryEntry>,
}

/// Resultado de [`list_filesystem`] (selector de ficheros y carpetas).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemList {
    pub path: PathBuf,
    pub parent: Option<PathBuf>,
    pub directories: Vec<DirectoryEntry>,
    pub files: Vec<DirectoryEntry>,
}

/// Carpetas de `path` (`None` → directorio personal): enlace al padre, sin
/// entradas ocultas y, si `path` no es un directorio, su carpeta contenedora
/// (`list_directories`).
pub fn list_directories(path: Option<&Path>) -> Result<DirectoryList, OpsError> {
    let current = resolve_listing_root(path)?;
    let mut directories: Vec<(String, DirectoryEntry)> = Vec::new();
    for child in read_listing(&current)? {
        let (entry, is_dir) = child;
        if is_dir {
            directories.push((entry.name.to_lowercase(), entry));
        }
    }
    directories.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(DirectoryList {
        parent: parent_of(&current),
        path: current,
        directories: directories.into_iter().map(|(_, entry)| entry).collect(),
    })
}

/// Como [`list_directories`] pero también lista ficheros, opcionalmente
/// filtrados por `extensions` con punto (p. ej. `[".yml", ".json"]`), con las
/// carpetas primero (`list_filesystem`).
pub fn list_filesystem(
    path: Option<&Path>,
    extensions: Option<&[&str]>,
) -> Result<FilesystemList, OpsError> {
    let current = resolve_listing_root(path)?;
    // Clave de orden de Python: `(item.is_file(), item.name.lower())`.
    let mut keyed: Vec<(bool, String, DirectoryEntry)> = Vec::new();
    for (entry, is_dir) in read_listing(&current)? {
        if !is_dir {
            let suffix = Path::new(&entry.name)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_lowercase);
            // `child.suffix.lower() in extensions` de Python: el sufijo lleva
            // punto, pero se acepta la forma sin punto también.
            let wanted = match extensions {
                None => true,
                Some(list) => matches!(suffix.as_deref(), Some(suffix) if list
                    .iter()
                    .any(|ext| ext.trim_start_matches('.') == suffix)),
            };
            if !wanted {
                continue;
            }
        }
        keyed.push((!is_dir, entry.name.to_lowercase(), entry));
    }
    keyed.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let mut directories = Vec::new();
    let mut files = Vec::new();
    for (is_file, _, entry) in keyed {
        if is_file {
            files.push(entry);
        } else {
            directories.push(entry);
        }
    }
    Ok(FilesystemList {
        parent: parent_of(&current),
        path: current,
        directories,
        files,
    })
}

/// Resuelve la carpeta a listar: `~` expandible, `None` → directorio personal,
/// inexistente → [`OpsError::NotFound`], fichero → su contenedora.
fn resolve_listing_root(path: Option<&Path>) -> Result<PathBuf, OpsError> {
    let expanded = match path {
        Some(path) => expanduser(path),
        None => home_dir().ok_or_else(|| OpsError::Io {
            path: PathBuf::from("~"),
            source: std::io::Error::new(ErrorKind::NotFound, "home directory not available"),
        })?,
    };
    let resolved = resolve_flexible(&expanded).map_err(|source| OpsError::Io {
        path: expanded.clone(),
        source,
    })?;
    let meta = match fs::metadata(&resolved) {
        Err(source) if source.kind() == ErrorKind::NotFound => {
            return Err(OpsError::NotFound { path: resolved });
        }
        Err(source) => {
            return Err(OpsError::Io {
                path: resolved,
                source,
            });
        }
        Ok(meta) => meta,
    };
    if meta.is_dir() {
        return Ok(resolved);
    }
    match resolved.parent().map(Path::to_path_buf) {
        Some(parent) if parent != resolved => Ok(parent),
        _ => Ok(resolved),
    }
}

/// Entradas directas de `current`: sin ocultas (prefijo `.`), clasificación
/// siguiendo symlinks, ruta = destino resuelto; las entradas con errores se
/// omiten (como el `except OSError: continue` de Python).
fn read_listing(current: &Path) -> Result<Vec<(DirectoryEntry, bool)>, OpsError> {
    let read = fs::read_dir(current).map_err(|source| OpsError::Io {
        path: current.to_path_buf(),
        source,
    })?;
    let mut out = Vec::new();
    for entry in read {
        let entry = entry.map_err(|source| OpsError::Io {
            path: current.to_path_buf(),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let raw = entry.path();
        let Ok(meta) = fs::metadata(&raw) else {
            continue;
        };
        if !meta.is_dir() && !meta.is_file() {
            continue;
        }
        let Ok(resolved) = resolve_flexible(&raw) else {
            continue;
        };
        out.push((
            DirectoryEntry {
                name,
                path: resolved,
            },
            meta.is_dir(),
        ));
    }
    Ok(out)
}

/// Padre de `current` (sin repetir la raíz, `None` en la raíz del filesystem).
fn parent_of(current: &Path) -> Option<PathBuf> {
    current
        .parent()
        .map(Path::to_path_buf)
        .filter(|parent| parent != current)
}

/// `Path.expanduser()` para la entrada del selector: `~` y `~/…`.
fn expanduser(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if text == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    } else if let Some(rest) = text.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    path.to_path_buf()
}

/// Directorio personal (`HOME` / `USERPROFILE`).
fn home_dir() -> Option<PathBuf> {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(key) {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

/// Ruta relativa con `/` entre `root` y `path` (bajo `root` por construcción).
fn relative_posix(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rest) => rest
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// `mkdir(exist_ok=True)` de Python: la carpeta existente está bien, un
/// fichero en su lugar es [`OpsError::AlreadyExists`].
fn ensure_dir(path: &Path) -> Result<(), OpsError> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(source) => Err(map_create_err(path, source)),
    }
}

/// `mkdir(parents=True, exist_ok=True)` de Python.
fn ensure_dir_all(path: &Path) -> Result<(), OpsError> {
    fs::create_dir_all(path).map_err(|source| map_create_err(path, source))
}

fn map_create_err(path: &Path, source: std::io::Error) -> OpsError {
    if source.kind() == ErrorKind::AlreadyExists {
        OpsError::AlreadyExists {
            path: path.to_path_buf(),
        }
    } else {
        OpsError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

fn map_rename_err(source_path: &Path, err: std::io::Error) -> OpsError {
    if err.kind() == ErrorKind::NotFound {
        OpsError::NotFound {
            path: source_path.to_path_buf(),
        }
    } else {
        OpsError::Io {
            path: source_path.to_path_buf(),
            source: err,
        }
    }
}

fn map_remove_err(path: &Path, source: std::io::Error) -> OpsError {
    if source.kind() == ErrorKind::NotFound {
        OpsError::NotFound {
            path: path.to_path_buf(),
        }
    } else {
        OpsError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// `json.dumps(..., indent=2)` para el `project.config.json`, con el orden de
/// claves y el `ensure_ascii` de Python.
fn scaffold_json(name: &str) -> String {
    format!(
        "{{\n  \"name\": {},\n  \"version\": 1,\n  \"created_at\": \"{}\",\n  \"folders\": {{\n    \"input\": \"Input\",\n    \"steps\": \"steps\",\n    \"outputs\": \"outputs\"\n  }}\n}}",
        json_string(name),
        utc_iso_now(),
    )
}

/// Cadena JSON al estilo `json.dumps` (ASCII escapado por unidades UTF-16).
fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for unit in value.encode_utf16() {
        match unit {
            0x22 => out.push_str("\\\""), // "
            0x5c => out.push_str("\\\\"), // \
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0..=0x1f => out.push_str(&format!("\\u{unit:04x}")),
            unit if unit < 0x80 => out.push(unit as u8 as char),
            unit => out.push_str(&format!("\\u{unit:04x}")),
        }
    }
    out.push('"');
    out
}

/// Marca de tiempo UTC tipo ISO-8601 (formato `datetime.now(timezone.utc)
/// .isoformat()`: microsegundos solo si son distintos de cero).
fn utc_iso_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let micros = now.subsec_micros();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
    if micros == 0 {
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}+00:00")
    } else {
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{micros:06}+00:00")
    }
}

/// Días desde el epoch a fecha civil (algoritmo de días propios de Howard
/// Hinnant).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_segment_ports_python_cases() {
        assert_eq!(safe_segment("My Pipeline Project"), "My_Pipeline_Project");
        assert_eq!(safe_segment(" bad / name "), "bad_name");
        assert_eq!(safe_segment(""), "project");
        assert_eq!(safe_segment("   "), "project");
        assert_eq!(safe_segment("..."), "project");
        assert_eq!(safe_segment("..hidden"), "hidden");
        assert_eq!(safe_segment("__x__"), "x");
        assert_eq!(safe_segment("a\tb"), "a_b");
        assert_eq!(safe_segment("step#1"), "step_1");
        assert_eq!(safe_segment("héllo wörld"), "héllo_wörld");
        assert_eq!(safe_segment("UPPER.case"), "UPPER.case");
        assert_eq!(safe_segment("café au lait"), "café_au_lait");
        assert_eq!(safe_segment("100%_done"), "100_done");
        assert_eq!(safe_segment("a/b\\c"), "a_b_c");
        assert_eq!(safe_segment("a__b___c"), "a_b_c");
    }

    #[test]
    fn scaffold_json_matches_python_shape() {
        let json = scaffold_json("My \"Pipe\"");
        assert!(json.starts_with("{\n  \"name\": \"My \\\"Pipe\\\"\",\n  \"version\": 1,\n  \"created_at\": \""));
        assert!(json.contains("\"20"));
        assert!(json.ends_with(
            "\"folders\": {\n    \"input\": \"Input\",\n    \"steps\": \"steps\",\n    \"outputs\": \"outputs\"\n  }\n}"
        ));
        // ensure_ascii de Python: no-ASCII → \uXXXX.
        assert_eq!(json_string("café"), "\"caf\\u00e9\"");
    }

    /// F5: el camino CREATE abre con `create_new` — probado DIRECTAMENTE para
    /// saltarse el chequeo previo: ni truncate, ni rename-over, ni pisar datos.
    #[test]
    fn create_helpers_never_replace_existing_targets() {
        let base = std::env::temp_dir().join(format!(
            "opm-create-exclusive-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).expect("temp base");

        let file = base.join("file.txt");
        fs::write(&file, "keep").unwrap();
        let err = create_file_exclusive(&file, "clobber").unwrap_err();
        assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "keep");

        let dir = base.join("dir");
        fs::create_dir(&dir).unwrap();
        let err = create_dir_exclusive(&dir).unwrap_err();
        assert!(matches!(err, OpsError::AlreadyExists { .. }), "{err:?}");
        assert!(dir.is_dir());

        // Y la creación válida sigue escribiendo el contenido íntegro.
        let fresh = base.join("fresh.txt");
        create_file_exclusive(&fresh, "new").unwrap();
        assert_eq!(fs::read_to_string(&fresh).unwrap(), "new");
        let fresh_dir = base.join("fresh-dir");
        create_dir_exclusive(&fresh_dir).unwrap();
        assert!(fresh_dir.is_dir());

        let _ = fs::remove_dir_all(&base);
    }

    /// P2: las dos ramas de [`destinations_conflict`] quedan fijadas en
    /// cualquier host a través del gancho por hilo — variantes de espacios
    /// pliegan SIEMPRE; mayúsculas SOLO donde el FS de destino es
    /// case-insensitive.
    #[test]
    fn destinations_conflict_plega_espacios_y_mayusculas_por_rama() {
        let report = Path::new("/proy/Input/report.txt");
        let upper = Path::new("/proy/Input/REPORT.txt");

        // Pliegue de variantes con espacios: siempre, en ambas ramas.
        assert!(destinations_conflict(
            report,
            Path::new("/proy/Input/ report.txt ")
        ));
        force_case_insensitive_destinations(Some(false));
        assert!(destinations_conflict(
            report,
            Path::new("/proy/Input/ report.txt ")
        ));

        // Rama case-insensitive forzada (Windows/macOS): colisionan.
        force_case_insensitive_destinations(Some(true));
        assert!(destinations_conflict(report, upper));
        // Rama case-sensitive forzada (Linux): destinos distintos.
        force_case_insensitive_destinations(Some(false));
        assert!(!destinations_conflict(report, upper));
        force_case_insensitive_destinations(None);

        // Sin forzar: decide el valor de plataforma.
        assert_eq!(
            destinations_conflict(report, upper),
            DEFAULT_CASE_INSENSITIVE_DESTINATIONS
        );

        // Nombres distintos no colisionan en ninguna rama.
        force_case_insensitive_destinations(Some(true));
        assert!(!destinations_conflict(report, Path::new("/proy/Input/report.md")));
        assert!(!destinations_conflict(
            Path::new("/proy/Input/sub/report.txt"),
            upper
        ));
        force_case_insensitive_destinations(None);
    }
}
