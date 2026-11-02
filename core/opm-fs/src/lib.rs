// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Filesystem of Open Pipeline Manager: file I/O over the editor buffer
//! ([`LocalFs`] implements [`FileStore`]), atomic saves, safe project-relative
//! path resolution ([`resolve_pipeline_path`] family) and the project
//! operations (scaffold, guarded file ops, batch import, tree/preview/list)
//! ported from the Python backend.
//!
//! Promises kept from the storage core:
//! - only UTF-8 for documents: clear error, no silent conversion;
//! - LF stays LF, CRLF stays CRLF;
//! - external changes are never overwritten silently (save guard +
//!   [`FsError::ExternalModification`]);
//! - atomic saves ([`write_atomic`]) and path validation: no resolved path
//!   ever leaves the project root.

mod atomic;
mod local;
mod ops;
mod paths;
mod pipeline_paths;
#[cfg(test)]
mod test_dir;

pub use atomic::{stage_write, write_atomic, StagedWrite};
pub use local::{FileStamp, LocalFs};
pub use ops::{
    build_tree, create_project_path, delete_project_path, ensure_project_scaffold,
    ensure_step_folders, force_case_insensitive_destinations, import_files, list_directories,
    list_filesystem, list_tree_children, move_project_path, preview_file, rename_project_path,
    safe_segment, validate_project_folder, write_project_file, DirectoryEntry, DirectoryList,
    FilePreview, FilesystemList, ImportSource, OpsError, TreeKind, TreeNode,
    IGNORED_NAMES, PROJECT_CONFIG_FILENAME,
};
pub use paths::{canonicalize, read_dir_entries, resolve_in_project, walk_tree, TreeEntry};
pub use pipeline_paths::{
    relative_project_path, resolve_managed_path, resolve_pipeline_path, resolve_project_path,
    PathError,
};

use opm_document::Document;
use std::fmt;
use std::path::{Path, PathBuf};

/// Errores de filesystem del core.
#[derive(Debug)]
pub enum FsError {
    /// El archivo no es UTF-8 válido (§12.5: error claro, sin conversión).
    NotUtf8 { path: PathBuf },
    /// Error de E/S del sistema.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// `save` sobre un documento sin archivo asignado: la UI debe abrir
    /// Save As (§12.4).
    MissingPath,
    /// La ruta resuelta sale del proyecto (§48: path traversal).
    PathOutsideProject { path: PathBuf },
    /// El archivo cambió en disco desde que se cargó (§12.7, §48): no se
    /// sobrescribe sin confirmación del usuario.
    ExternalModification { path: PathBuf },
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUtf8 { path } => {
                write!(
                    f,
                    "{} no es UTF-8 válido (solo se admite UTF-8, §12.5)",
                    path.display()
                )
            }
            Self::Io { path, source } => write!(f, "error de E/S en {}: {source}", path.display()),
            Self::MissingPath => {
                write!(
                    f,
                    "el documento no tiene archivo asignado; use Save As (§12.4)"
                )
            }
            Self::PathOutsideProject { path } => {
                write!(f, "la ruta {} sale del proyecto (§48)", path.display())
            }
            Self::ExternalModification { path } => write!(
                f,
                "{} cambió externamente desde que se cargó (§12.7)",
                path.display()
            ),
        }
    }
}

impl std::error::Error for FsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Almacenamiento de documentos sobre el buffer único (§6.1).
///
/// [`LocalFs`] lo implementa con `std::fs`, con guardado atómico (§48) y una
/// [`FileStamp`] por archivo abierto/guardado para detectar cambios externos
/// (§12.7).
pub trait FileStore {
    /// Documento nuevo, vacío y sin path (§12.1).
    fn create_new(&mut self) -> Document;
    /// Lee y valida UTF-8 (§12.5); el buffer preserva LF/CRLF (§12.6).
    fn open(&mut self, path: &Path) -> Result<Document, FsError>;
    /// Guardado explícito en el path del documento (§12.3). Sin autosave.
    ///
    /// Falla con [`FsError::MissingPath`] si el documento no tiene archivo y
    /// con [`FsError::ExternalModification`] si el archivo cambió en disco
    /// desde que se cargó (nunca sobrescritura silenciosa, §12.7).
    fn save(&mut self, document: &mut Document) -> Result<(), FsError>;
    /// Guardado a un path nuevo (§12.4).
    fn save_as(&mut self, document: &mut Document, path: &Path) -> Result<(), FsError>;
}
