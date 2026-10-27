// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Implementación local de [`FileStore`] con `std::fs` (ISSUE-013, ISSUE-019).

use crate::{FileStore, FsError};
use opm_document::Document;
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Huella del archivo en disco (mtime + longitud) para detectar cambios
/// externos (§12.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    fn of(path: &Path) -> std::io::Result<Self> {
        let meta = fs::metadata(path)?;
        Ok(Self {
            modified: meta.modified().ok(),
            len: meta.len(),
        })
    }
}

/// [`FileStore`] sobre el filesystem local.
///
/// Registra una [`FileStamp`] por archivo al abrirlo y al guardarlo; `save`
/// rechaza sobrescribir un archivo que haya cambiado en disco (§12.7, §48:
/// nunca sobrescrituras silenciosas) y todo guardado es atómico (§48,
/// [`crate::write_atomic`]). [`LocalFs::changed_externally`] sigue siendo la
/// garantía previa al guardado.
#[derive(Debug, Default)]
pub struct LocalFs {
    stamps: HashMap<PathBuf, FileStamp>,
    /// Raíz del proyecto para validar rutas (§48); `None` = archivo suelto.
    project_root: Option<PathBuf>,
}

impl LocalFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// ¿Cambia el archivo en disco sin que el buffer lo refleje (§12.7)?
    ///
    /// `Ok(true)` también cubre el caso de un archivo existente nunca
    /// abierto/guardado por esta instancia: sin huella registrada no puede
    /// garantizarse que no cambió, y no se sobrescribe en silencio. Un archivo
    /// borrado externamente no es un cambio pendiente: `save` lo recrea
    /// (acción explícita del usuario, §12.7).
    pub fn changed_externally(&self, path: &Path) -> Result<bool, FsError> {
        match FileStamp::of(path) {
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(false),
            Err(source) => Err(FsError::Io {
                path: path.to_path_buf(),
                source,
            }),
            Ok(current) => Ok(self.stamps.get(path) != Some(&current)),
        }
    }

    /// Guardado tras confirmación explícita del usuario (§3.7, §12.7).
    ///
    /// Únicamente para el caso en que `save` ha fallado con
    /// [`FsError::ExternalModification`] y el usuario ha confirmado en la UI
    /// que quiere sobrescribir el cambio externo con el contenido del buffer:
    /// acción explícita, observable y con destino conocido. Nunca usar como
    /// camino por defecto (§3.6: sin escrituras silenciosas).
    pub fn save_confirmed(&mut self, document: &mut Document) -> Result<(), FsError> {
        let Some(path) = document.path().map(Path::to_path_buf) else {
            return Err(FsError::MissingPath);
        };
        self.validated(&path)?;
        self.write_file(&path, document.content())?;
        document.mark_saved();
        self.record_stamp(&path)?;
        Ok(())
    }

    /// Fija la raíz del proyecto para validar rutas (§48). `None` (por
    /// defecto) = modo archivo suelto (§12), sin validación de pertenencia.
    pub fn set_project_root(&mut self, root: Option<PathBuf>) {
        self.project_root = root;
    }

    /// Re-llavea la huella de `old` a `new` tras un rename/movimiento PROPIO
    /// de la app (§12.7): el fichero en disco no cambia, pero sin re-llavarlo
    /// el siguiente guardado en `new` no encontraría huella y reportaría un
    /// falso [`FsError::ExternalModification`].
    pub fn retarget_stamp(&mut self, old: &Path, new: &Path) {
        if let Some(stamp) = self.stamps.remove(old) {
            self.stamps.insert(new.to_path_buf(), stamp);
        }
    }

    /// Descarta las huellas de `path` (y de sus descendientes) tras un
    /// borrado PROPIO (§12.7): la huella de un fichero ya borrado no debe
    /// decidir sobre nada, ni sobre una recreación posterior en esa ruta.
    pub fn forget_stamps_under(&mut self, path: &Path) {
        self.stamps
            .retain(|key, _| !(key == path || key.starts_with(path)));
    }

    /// Valida que `path` pertenece al proyecto cuando hay raíz fijada (§48):
    /// rechaza path traversal y symlinks que resuelven fuera del proyecto.
    fn validated(&self, path: &Path) -> Result<(), FsError> {
        match &self.project_root {
            Some(root) => crate::paths::resolve_in_project(root, path).map(|_| ()),
            None => Ok(()),
        }
    }

    fn ensure_unchanged(&self, path: &Path) -> Result<(), FsError> {
        if self.changed_externally(path)? {
            Err(FsError::ExternalModification {
                path: path.to_path_buf(),
            })
        } else {
            Ok(())
        }
    }

    fn write_file(&self, path: &Path, content: &str) -> Result<(), FsError> {
        // Contenido byte a byte (§6.2): el buffer nunca normalizó los saltos de
        // línea, así que LF/CRLF se preservan tal cual (§12.6). Guardado
        // atómico: temporal en el mismo directorio + rename (§48); un crash a
        // mitad de escritura nunca corrompe el archivo (test en `atomic`).
        crate::atomic::write_atomic(path, content.as_bytes()).map_err(|source| FsError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    fn record_stamp(&mut self, path: &Path) -> Result<(), FsError> {
        let stamp = FileStamp::of(path).map_err(|source| FsError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        self.stamps.insert(path.to_path_buf(), stamp);
        Ok(())
    }
}

impl FileStore for LocalFs {
    fn create_new(&mut self) -> Document {
        Document::new()
    }

    fn open(&mut self, path: &Path) -> Result<Document, FsError> {
        self.validated(path)?;
        let bytes = fs::read(path).map_err(|source| FsError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        // §12.5: UTF-8 estricto; nunca conversión silenciosa.
        let text = String::from_utf8(bytes)
            .map_err(|_| FsError::NotUtf8 {
                path: path.to_path_buf(),
            })?;
        let document = Document::from_text(Some(path.to_path_buf()), text);
        self.record_stamp(path)?;
        Ok(document)
    }

    fn save(&mut self, document: &mut Document) -> Result<(), FsError> {
        let Some(path) = document.path().map(Path::to_path_buf) else {
            return Err(FsError::MissingPath);
        };
        self.validated(&path)?;
        self.ensure_unchanged(&path)?;
        self.write_file(&path, document.content())?;
        document.mark_saved();
        self.record_stamp(&path)?;
        Ok(())
    }

    fn save_as(&mut self, document: &mut Document, path: &Path) -> Result<(), FsError> {
        self.validated(path)?;
        // Save As es una acción explícita con destino elegido por el usuario
        // (§12.4); la confirmación de sobrescritura es responsabilidad del
        // diálogo (ISSUE-020).
        self.write_file(path, document.content())?;
        document.set_path(path.to_path_buf());
        document.mark_saved();
        self.record_stamp(path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Directorio temporal de test, eliminado al salir.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            Self(crate::test_dir::crear("opm-local"))
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn create_new_es_lf_sin_path() {
        let mut fs = LocalFs::new();
        let doc = fs.create_new();
        assert_eq!(doc.path(), None);
        assert_eq!(doc.content(), "");
        assert_eq!(doc.line_ending(), opm_document::LineEnding::Lf);
        assert!(!doc.is_modified());
    }

    #[test]
    fn open_utf8_valido_preserva_contenido() {
        let dir = TestDir::new();
        let path = dir.file("notas.md");
        fs::write(&path, "# Título\n\ncuerpo\n").unwrap();

        let mut store = LocalFs::new();
        let doc = store.open(&path).unwrap();
        assert_eq!(doc.content(), "# Título\n\ncuerpo\n");
        assert_eq!(doc.path(), Some(path.as_path()));
        assert!(!doc.is_modified());
    }

    #[test]
    fn open_utf8_invalido_da_error_claro() {
        // §12.5: sin conversiones silenciosas.
        let dir = TestDir::new();
        let path = dir.file("binario.md");
        fs::write(&path, [0xFF, 0xFE, 0x80]).unwrap();

        let mut store = LocalFs::new();
        let error = store.open(&path).unwrap_err();
        assert!(matches!(&error, FsError::NotUtf8 { path: p } if *p == path));
        assert!(error.to_string().contains("UTF-8"));
    }

    #[test]
    fn preserva_lf_y_crlf_al_guardar() {
        // §12.6: LF permanece LF, CRLF permanece CRLF.
        let dir = TestDir::new();
        let mut store = LocalFs::new();

        for (name, raw) in [("lf.md", "uno\ndos\n"), ("crlf.md", "uno\r\ndos\r\n")] {
            let path = dir.file(name);
            fs::write(&path, raw).unwrap();
            let mut doc = store.open(&path).unwrap();
            assert_eq!(doc.line_ending().as_str(), if raw.contains('\r') { "\r\n" } else { "\n" });

            doc.replace_content(raw);
            store.save(&mut doc).unwrap();
            assert_eq!(fs::read(&path).unwrap(), raw.as_bytes());
        }
    }

    #[test]
    fn save_sin_path_da_missing_path() {
        let mut store = LocalFs::new();
        let mut doc = store.create_new();
        doc.replace_content("x");
        assert!(matches!(store.save(&mut doc), Err(FsError::MissingPath)));
    }

    #[test]
    fn save_as_a_nuevo_path_y_save_posterior() {
        let dir = TestDir::new();
        let path = dir.file("nuevo.md");

        let mut store = LocalFs::new();
        let mut doc = store.create_new();
        doc.replace_content("# Nuevo\n");
        store.save_as(&mut doc, &path).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "# Nuevo\n");
        assert_eq!(doc.path(), Some(path.as_path()));
        assert!(!doc.is_modified());

        // Tras Save As, save actualiza en el mismo archivo.
        doc.replace_content("# Editado\n");
        store.save(&mut doc).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Editado\n");
        assert!(!doc.is_modified());
    }

    #[test]
    fn cambio_externo_bloquea_el_guardado() {
        // §12.7 / §48: nunca sobrescrituras silenciosas.
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "original\n").unwrap();

        let mut store = LocalFs::new();
        let mut doc = store.open(&path).unwrap();
        assert_eq!(store.changed_externally(&path).unwrap(), false);

        // Otro programa modifica el archivo (longitud distinta: detectable
        // aunque el mtime tenga resolución gruesa).
        fs::write(&path, "modificado por otro programa\n").unwrap();
        assert_eq!(store.changed_externally(&path).unwrap(), true);

        doc.replace_content("edición local\n");
        let error = store.save(&mut doc).unwrap_err();
        assert!(matches!(&error, FsError::ExternalModification { path: p } if *p == path));
        // El buffer sigue modificado y el archivo externo intacto.
        assert!(doc.is_modified());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "modificado por otro programa\n"
        );

        // Volver a abrir registra la nueva huella y vuelve a permitir guardar.
        let mut doc = store.open(&path).unwrap();
        assert_eq!(store.changed_externally(&path).unwrap(), false);
        doc.replace_content("edición local\n");
        store.save(&mut doc).unwrap();
    }

    #[test]
    fn guardar_recrea_archivo_borrado_externamente() {
        // §12.7: el contenido sigue en memoria y el usuario puede Save.
        let dir = TestDir::new();
        let path = dir.file("borrado.md");
        fs::write(&path, "original\n").unwrap();

        let mut store = LocalFs::new();
        let mut doc = store.open(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(store.changed_externally(&path).unwrap(), false);
        doc.replace_content("se regenera\n");
        store.save(&mut doc).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "se regenera\n");
    }

    #[test]
    fn guardado_atomico_no_deja_temporales() {
        // §48: temporal + rename; en el directorio solo queda el destino.
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "uno\n").unwrap();

        let mut store = LocalFs::new();
        let mut doc = store.open(&path).unwrap();
        doc.replace_content("dos\n");
        store.save(&mut doc).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "dos\n");
        let mut entries = fs::read_dir(&dir.0).unwrap();
        let only = entries.next().unwrap().unwrap().file_name();
        assert_eq!(only, std::ffi::OsString::from("doc.md"));
        assert!(entries.next().is_none(), "sobran temporales en el directorio");
    }

    #[test]
    fn save_confirmed_sobrescribe_el_cambio_externo_tras_confirmar() {
        // §3.7/§12.7: sin confirmación nunca; con confirmación explícita sí.
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "original\n").unwrap();

        let mut store = LocalFs::new();
        let mut doc = store.open(&path).unwrap();
        doc.replace_content("edición local\n");
        fs::write(&path, "otro programa\n").unwrap();

        // Sin confirmación: rechazo y el archivo externo queda intacto.
        assert!(matches!(
            store.save(&mut doc),
            Err(FsError::ExternalModification { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "otro programa\n");

        // Confirmación explícita del usuario: sobrescritura observable.
        store.save_confirmed(&mut doc).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "edición local\n");
        assert!(!doc.is_modified());
        assert_eq!(store.changed_externally(&path).unwrap(), false);
    }

    #[test]
    fn retarget_stamp_mueve_la_huella_al_path_renombrado() {
        // P2-9: tras un rename propio, guardar en la nueva ruta no debe ver
        // un falso cambio externo (§12.7) — la huella se re-llava.
        let dir = TestDir::new();
        let old = dir.file("doc.md");
        fs::write(&old, "hola\n").unwrap();

        let mut store = LocalFs::new();
        let mut doc = store.open(&old).unwrap();
        assert_eq!(store.changed_externally(&old).unwrap(), false);

        // Renombrar no cambia contenido ni mtime, pero la huella vive bajo
        // la clave VIEJA: sin re-llavarla, `new` parecería modificado.
        let new = dir.file("renombrado.md");
        fs::rename(&old, &new).unwrap();
        assert_eq!(
            store.changed_externally(&new).unwrap(),
            true,
            "sin huella en la clave nueva → falso ExternalModification"
        );

        store.retarget_stamp(&old, &new);
        assert_eq!(store.changed_externally(&new).unwrap(), false);

        // Guardar en la nueva ruta procede sin confirmación externa.
        doc.set_path(new.clone());
        doc.replace_content("hola!\n");
        store.save(&mut doc).unwrap();
        assert_eq!(fs::read_to_string(&new).unwrap(), "hola!\n");
    }

    #[test]
    fn forget_stamps_under_descarta_la_huella_del_borrado() {
        // P2-9: la huella de un fichero borrado propio no decide sobre nada.
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "hola\n").unwrap();

        let mut store = LocalFs::new();
        let _doc = store.open(&path).unwrap();
        assert_eq!(store.changed_externally(&path).unwrap(), false);

        store.forget_stamps_under(&path);
        assert_eq!(
            store.changed_externally(&path).unwrap(),
            true,
            "sin huella registrado no se garantiza nada (§12.7)"
        );
    }

    #[test]
    fn rutas_fuera_del_proyecto_se_rechazan() {
        // §48: validación de rutas internas en modo proyecto.
        let dir = TestDir::new();
        let proyecto = dir.0.join("proyecto");
        let fuera = dir.0.join("fuera.md");
        fs::create_dir_all(&proyecto).unwrap();
        fs::write(&fuera, "externo\n").unwrap();

        let mut store = LocalFs::new();
        store.set_project_root(Some(proyecto.clone()));

        assert!(matches!(
            store.open(&fuera),
            Err(FsError::PathOutsideProject { .. })
        ));
        let mut doc = store.create_new();
        doc.replace_content("x");
        assert!(matches!(
            store.save_as(&mut doc, &fuera),
            Err(FsError::PathOutsideProject { .. })
        ));

        // Dentro del proyecto sí.
        let dentro = proyecto.join("notas.md");
        store.save_as(&mut doc, &dentro).unwrap();
        assert_eq!(fs::read_to_string(&dentro).unwrap(), "x");
    }
}
