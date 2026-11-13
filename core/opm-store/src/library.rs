//! `PipelineLibrary` — named native `.pipeline` documents under an explicit
//! root (the application `pipelines/` location is resolved by the UI layer,
//! not here: every entry point takes explicit paths so the core stays
//! testable).
//!
//! One canonical UTF-8 JSON document per reusable pipeline; YAML
//! (`.yml`/`.yaml`) and JSON (`.json`) exist only as explicit import/export
//! formats and never become a second document the app must operate.

use std::fs;
use std::path::{Path, PathBuf};

use opm_fs::{resolve_managed_path, safe_segment, write_atomic};
use opm_pipeline::parse_pipeline;

use crate::envelope::PipelineDoc;
use crate::store::StoreError;

/// Extension of native library documents (encoding: UTF-8 JSON).
const LIBRARY_EXT: &str = "pipeline";
/// File suffix of native library documents.
const LIBRARY_SUFFIX: &str = ".pipeline";

/// How [`PipelineLibrary::save`] treats an existing name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveMode {
    /// Refuse to replace an existing document ([`StoreError::NameExists`]).
    CreateNew,
    /// Replace the existing document atomically.
    Overwrite,
}

/// Encoding of an explicit [`PipelineLibrary::export`] (the pipeline body,
/// by-alias wire format either way).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportFormat {
    Json,
    Yaml,
}

/// The named reusable-pipeline library: `<root>/<name>.pipeline` files.
pub struct PipelineLibrary {
    root: PathBuf,
}

impl PipelineLibrary {
    /// Library over an explicit root. The root may not exist yet; it is
    /// created on the first [`Self::save`].
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Library root as given.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Sorted file stems of every `*.pipeline` document. A missing or
    /// unreadable root lists as empty (nothing saved yet).
    pub fn list(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.path().extension().and_then(|ext| ext.to_str()) == Some(LIBRARY_EXT)
            })
            .filter_map(|entry| {
                entry
                    .path()
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(String::from)
            })
            .collect();
        names.sort();
        names
    }

    /// Load one document by name (file stem, sanitized like [`Self::save`]).
    pub fn load(&self, name: &str) -> Result<PipelineDoc, StoreError> {
        let stem = library_stem(name)?;
        let path = self.path_for(&stem)?;
        if !path.exists() {
            return Err(StoreError::Message(format!("No pipeline named {stem}")));
        }
        let text = fs::read_to_string(&path).map_err(StoreError::Io)?;
        PipelineDoc::from_json(&text, &path)
    }

    /// Save one document under `name`, atomically. `CreateNew` over an
    /// existing name fails with [`StoreError::NameExists`]; `Overwrite`
    /// replaces the previous complete document in one atomic rename.
    pub fn save(&self, name: &str, doc: &PipelineDoc, mode: SaveMode) -> Result<(), StoreError> {
        let stem = library_stem(name)?;
        let path = self.path_for(&stem)?;
        if mode == SaveMode::CreateNew && path.exists() {
            return Err(StoreError::NameExists(stem));
        }
        let text = doc.to_json()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(StoreError::Io)?;
        }
        write_atomic(&path, text.as_bytes()).map_err(StoreError::Io)?;
        Ok(())
    }

    /// Import an external `.yml`/`.yaml`/`.json` pipeline as a new native
    /// document (CreateNew semantics: imports never silently overwrite).
    pub fn import_file(&self, path: &Path, name: &str) -> Result<PipelineDoc, StoreError> {
        let stem = library_stem(name)?;
        let text = fs::read_to_string(path).map_err(StoreError::Io)?;
        let mut doc = if path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("pipeline")) {
            let mut doc = PipelineDoc::from_json(&text, path)?;
            doc.normalize();
            doc
        } else {
            let pipeline = parse_pipeline(&path.to_string_lossy(), &text)
                .map_err(|err| StoreError::Message(err.to_string()))?;
            let mut doc = PipelineDoc::new(&stem, pipeline);
            doc.normalize_for_import();
            doc
        };
        doc.name = stem.clone();
        self.save(&stem, &doc, SaveMode::CreateNew)?;
        Ok(doc)
    }

    /// Export a document's pipeline body for explicit sharing (JSON or YAML
    /// by-alias wire format). Relative destinations resolve against the
    /// library root; file dialogs pass absolute paths.
    pub fn export(
        &self,
        doc: &PipelineDoc,
        dest: &Path,
        format: ExportFormat,
    ) -> Result<(), StoreError> {
        let body = match format {
            ExportFormat::Json => serde_json::to_string_pretty(&doc.pipeline)
                .map_err(|err| StoreError::Message(err.to_string()))?,
            ExportFormat::Yaml => serde_yml::to_string(&doc.pipeline)
                .map_err(|err| StoreError::Message(err.to_string()))?,
        };
        let mut target = dest.to_path_buf();
        if target.is_relative() {
            target = self.root.join(&target);
        }
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(StoreError::Io)?;
            }
        }
        write_atomic(&target, body.as_bytes()).map_err(StoreError::Io)?;
        Ok(())
    }

    /// Managed path of `<stem>.pipeline` inside the library root —
    /// containment + symlink-free components (F2), no hand-rolled safety.
    fn path_for(&self, stem: &str) -> Result<PathBuf, StoreError> {
        resolve_managed_path(&self.root, &format!("{stem}.{LIBRARY_EXT}"))
            .map_err(StoreError::Io)
    }
}

/// File stem for a user-supplied library name: trim, drop a redundant
/// `.pipeline` suffix, sanitize to one safe segment (`safe_segment`), and
/// reject empty names outright (before `safe_segment`'s `"project"` fallback).
fn library_stem(name: &str) -> Result<String, StoreError> {
    let trimmed = name.trim();
    let stem = trimmed
        .strip_suffix(LIBRARY_SUFFIX)
        .map(str::trim)
        .unwrap_or(trimmed);
    if stem.is_empty() {
        return Err(StoreError::Message("Pipeline name is required".to_string()));
    }
    Ok(safe_segment(stem))
}
