// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Modelo de pestañas (§37): ciclo de vida de tabs, paths y guardado.
//!
//! [`TabManager`] posee un [`LocalFs`] interno (ISSUE-013/019) para todo el
//! file I/O (§12): abrir con [`FileStore::open`], guardar solo de forma
//! explícita (§3.6) y nunca sobrescribir en silencio un cambio externo
//! (§12.7). El historial Undo/Redo vive dentro de cada [`Editor`] y cambiar de
//! modo no lo reinicia (§8.5).

use super::scope::WindowScope;
use crate::editor::EditMode;
use crate::paths;
use opm_document::{Editor, Position, Range, Selection};
use opm_fs::{FileStore, FsError, LocalFs};
use std::path::{Path, PathBuf};

/// Una pestaña: un documento y su estado de UI (§37).
///
/// Invariante: `path` espeja `Editor::document().path()` (§12.4); los
/// métodos de [`TabManager`] los mantienen sincronizados.
pub struct Tab {
    /// Modelo único del documento (§6.1); su historial Undo/Redo (§8.5) vive
    /// aquí y cambiar de modo no lo reinicia.
    pub editor: Editor,
    /// Modo de edición activo de esta pestaña (Markdown/Visual).
    pub mode: EditMode,
    /// Archivo asignado; `None` = «Sin título N» (§12.1).
    pub path: Option<PathBuf>,
    /// N de «Sin título N» cuando `path == None` (§37).
    pub untitled_number: u64,
    /// El archivo cambió o desapareció en disco (§12.7, ISSUE-026): se avisa
    /// en el título (§3.7) **sin** recargar el buffer.
    pub external_change: bool,
}

/// Conjunto de pestañas con un único tab activo (§37).
///
/// Invariante: **siempre** queda al menos un tab; cerrar el último crea uno
/// nuevo «Sin título N» (§37). Los índices de los métodos son los de
/// [`TabManager::tabs`].
pub struct TabManager {
    tabs: Vec<Tab>,
    /// Contenido en disco de cada tab (`saved[i]` ⇔ `tabs[i]`): base para que
    /// el indicador de cambios sin guardar (§6.2) vuelva a `false` cuando un
    /// undo/redo devuelve el buffer exactamente al contenido guardado.
    saved: Vec<String>,
    active: usize,
    /// Próximo N de «Sin título N» (§37); nunca se reutiliza, para que los
    /// títulos sean únicos.
    next_untitled: u64,
    fs: LocalFs,
    /// Ámbito de la ventana (ISSUE-021): standalone o de proyecto.
    scope: WindowScope,
}

impl TabManager {
    /// Arranca con un tab vacío «Sin título 1» (§37, §12.1).
    pub fn new() -> Self {
        let mut manager = Self {
            tabs: Vec::new(),
            saved: Vec::new(),
            active: 0,
            next_untitled: 1,
            fs: LocalFs::new(),
            scope: WindowScope::Standalone,
        };
        let tab = manager.blank_tab();
        manager.push_tab(tab, String::new());
        manager
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> &Tab {
        &self.tabs[self.active]
    }

    /// Fija el ámbito de la ventana (ISSUE-021): las ventanas de proyecto
    /// validan sus rutas contra la raíz (§48); las standalone no restringen
    /// nada (Notepad++).
    pub fn set_scope(&mut self, scope: WindowScope) {
        self.fs.set_project_root(scope.project_root());
        self.scope = scope;
    }

    /// Crea un tab vacío «Sin título N» y lo activa; devuelve su índice.
    pub fn new_tab(&mut self) -> usize {
        let tab = self.blank_tab();
        self.push_tab(tab, String::new())
    }

    /// Abre `path` en un tab nuevo que queda activo y devuelve su índice
    /// (§12.2, §12.5: solo UTF-8, sin conversiones silenciosas).
    pub fn open(&mut self, path: &Path) -> Result<usize, FsError> {
        let document = self.fs.open(path)?;
        let saved = document.content().to_string();
        let tab = Tab {
            editor: Editor::new(document),
            mode: EditMode::Markdown,
            path: Some(path.to_path_buf()),
            untitled_number: 0,
            external_change: false,
        };
        Ok(self.push_tab(tab, saved))
    }

    /// Guardado explícito del tab activo en su archivo (§3.6, §12.3).
    ///
    /// Sin path asignado falla con [`FsError::MissingPath`] (§12.4: la UI
    /// debe abrir «Guardar como»). Si el archivo cambió en disco se propaga
    /// [`FsError::ExternalModification`] sin tragarlo (§12.7: nunca
    /// sobrescrituras silenciosas).
    pub fn save_active(&mut self) -> Result<(), FsError> {
        let index = self.active;
        let tab = &mut self.tabs[index];
        let Some(path) = tab.path.clone() else {
            return Err(FsError::MissingPath);
        };
        // El path del tab manda; se espeja en el Document (§12.4).
        tab.editor.document_mut().set_path(path);
        let result = self.fs.save(tab.editor.document_mut());
        self.record_saved(index, &result);
        result
    }

    /// «Guardar como» del tab activo (§12.4): asigna el path y guarda.
    pub fn save_active_as(&mut self, path: &Path) -> Result<(), FsError> {
        let index = self.active;
        let tab = &mut self.tabs[index];
        let result = self.fs.save_as(tab.editor.document_mut(), path);
        if result.is_ok() {
            tab.path = Some(path.to_path_buf());
        }
        self.record_saved(index, &result);
        result
    }

    /// Sobrescribe con el buffer **solo** tras confirmación explícita del
    /// usuario (§3.7, §12.7). Único camino para vencer un
    /// [`FsError::ExternalModification`]; nunca usar por defecto (§3.6).
    pub fn save_active_confirmed(&mut self) -> Result<(), FsError> {
        let index = self.active;
        let tab = &mut self.tabs[index];
        let Some(path) = tab.path.clone() else {
            return Err(FsError::MissingPath);
        };
        tab.editor.document_mut().set_path(path);
        let result = self.fs.save_confirmed(tab.editor.document_mut());
        self.record_saved(index, &result);
        result
    }

    /// Cierra el tab activo. El llamador debe haber confirmado antes si
    /// estaba modificado (§3.7). Invariante: SIEMPRE queda ≥1 tab (cerrar el
    /// último crea uno nuevo «Sin título N», §37).
    pub fn close_active(&mut self) {
        let index = self.active;
        self.tabs.remove(index);
        self.saved.remove(index);
        if self.tabs.is_empty() {
            self.new_tab();
        } else {
            self.active = index.min(self.tabs.len() - 1);
        }
    }

    /// Activa el tab `index`. Un índice obsoleto de un callback GTK se
    /// ignora sin panic (frontera UI).
    pub fn set_active(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    /// Cambia el modo del tab activo (Markdown ⇄ Visual) y lo devuelve.
    /// §8.5: no toca el historial Undo/Redo, que vive en el `Editor`.
    pub fn toggle_mode_active(&mut self) -> EditMode {
        let tab = &mut self.tabs[self.active];
        tab.mode = tab.mode.toggled();
        tab.mode
    }

    /// Deshace la última edición del tab activo (§8.5). `false` = historial
    /// vacío.
    pub fn undo_active(&mut self) -> bool {
        let undone = self.tabs[self.active].editor.undo();
        if undone {
            self.resync_saved_flag();
        }
        undone
    }

    /// Rehace la última edición deshecha del tab activo (§8.5). `false` =
    /// nada que rehacer.
    pub fn redo_active(&mut self) -> bool {
        let redone = self.tabs[self.active].editor.redo();
        if redone {
            self.resync_saved_flag();
        }
        redone
    }

    /// ¿El tab `index` tiene cambios sin guardar (§6.2)? Es
    /// `Editor::document().is_modified()`; tras undo/redo el flag se
    /// resincroniza con la realidad «buffer == contenido guardado» (§6.2).
    ///
    /// # Panics
    ///
    /// Índice fuera de rango (comportamiento de índice de slice).
    pub fn is_modified(&self, index: usize) -> bool {
        self.tabs[index].editor.document().is_modified()
    }

    /// Título de pestaña (§37): «● nombre» si modificado, «⚠ nombre» si el
    /// archivo cambió en disco (§12.7, ISSUE-026), «nombre» si no; nombre =
    /// nombre del fichero o «Sin título N».
    ///
    /// # Panics
    ///
    /// Índice fuera de rango (comportamiento de índice de slice).
    pub fn title(&self, index: usize) -> String {
        let tab = &self.tabs[index];
        let name = match &tab.path {
            Some(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            None => format!("Sin título {}", tab.untitled_number),
        };
        let flags = match (self.is_modified(index), tab.external_change) {
            (true, true) => "●⚠ ",
            (true, false) => "● ",
            (false, true) => "⚠ ",
            (false, false) => "",
        };
        format!("{flags}{name}")
    }

    /// Aviso §12.7 (ISSUE-026): `path` cambió o desapareció en disco; las
    /// pestañas afectadas quedan marcadas en el título (§3.7) **sin** tocar
    /// su buffer (§12.7: nunca recarga automática). Un evento sobre una
    /// carpeta afecta a sus pestañas hijas.
    ///
    /// OPM no tiene watcher (refresco manual): lo consume
    /// [`Self::mark_external_changes`] desde el Refresh del explorador.
    /// Por segmentos con separadores normalizados (P1): una pestaña
    /// serializada `folder\file.md` también recibe el aviso de `folder`.
    pub fn mark_external(&mut self, path: &Path) {
        for tab in &mut self.tabs {
            if tab
                .path
                .as_ref()
                .is_some_and(|p| paths::is_descendant(p, path))
            {
                tab.external_change = true;
            }
        }
    }

    /// Detecta con la huella del [`LocalFs`] qué ficheros abiertos cambió
    /// en disco y los marca con [`Self::mark_external`] (§12.7, sin recargar
    /// ningún buffer: el usuario decide). Consumido por el Refresh manual
    /// del explorador — la detección vive en producción, no en un watcher
    /// futuro.
    pub fn mark_external_changes(&mut self) {
        let changed: Vec<PathBuf> = (0..self.tabs.len())
            .filter_map(|index| {
                let path = self.tabs[index].path.clone()?;
                match self.fs.changed_externally(&path) {
                    Ok(true) => Some(path),
                    _ => None,
                }
            })
            .collect();
        for path in changed {
            self.mark_external(&path);
        }
    }

    /// Recarga desde disco el buffer de las pestañas abiertas en `path`
    /// (§12.7): sustituye el contenido del [`Editor`] **sin transacción**
    /// (§8.5: la escritura no pasa por el historial, que no se reinicia)
    /// conservando la selección (§8.6), marca guardado (sin «●») y limpia el
    /// banderín `external_change`. Además refresca la huella interna del
    /// archivo para que un guardado posterior no lo tome por cambio externo
    /// (§12.7).
    ///
    /// Las pestañas con modificaciones sin guardar NO se tocan (§12.7: el
    /// buffer del usuario manda y el aviso ⚠ permanece; el guardado
    /// confirmado es su camino para sobrescribir). Un `path` sin pestaña
    /// abierta es un no-op.
    ///
    /// El consumidor es la opción «Recargar» del diálogo de cambio externo de
    /// [`crate::menus::dialogs`] (§12.7) — la recarga NUNCA es automática.
    pub fn reload_from_disk(&mut self, path: &Path) -> Result<(), FsError> {
        for index in 0..self.tabs.len() {
            if self.tabs[index].path.as_deref() != Some(path) {
                continue;
            }
            if self.is_modified(index) {
                continue;
            }
            let document = self.fs.open(path)?;
            let content = document.content().to_string();
            let tab = &mut self.tabs[index];
            tab.editor.replace_content(&content);
            tab.editor.document_mut().mark_saved();
            tab.external_change = false;
            self.saved[index] = content;
        }
        Ok(())
    }

    /// Puente EditSink (§8.6): inserta `text` en la posición `at`.
    ///
    /// Un [`opm_document::EditError`] (posición inválida) es improbable
    /// con el puente GTK y se ignora documentadamente: sin panics en la UI.
    pub fn insert_active(&mut self, at: Position, text: &str) {
        let _ = self.tabs[self.active]
            .editor
            .replace(Range::new(at, at), text);
    }

    /// Reemplaza `range` por `text` en el tab activo (búsqueda §40.1): una
    /// transacción del historial único (§8.5). `false` = rango inválido (sin
    /// panics en la UI).
    pub fn replace_active(&mut self, range: Range, text: &str) -> bool {
        let replaced = self.tabs[self.active].editor.replace(range, text).is_ok();
        if replaced {
            self.resync_saved_flag();
        }
        replaced
    }

    /// Reemplazo masivo §40.1 sobre el tab activo como **UNA sola
    /// transacción** (§8.5: un solo undo la revierte entera). `false` =
    /// rangos inválidos o solapados (sin panics en la UI).
    pub fn replace_all_active(&mut self, edits: &[(Range, String)]) -> bool {
        let replaced = self.tabs[self.active].editor.replace_all(edits).is_ok();
        if replaced {
            self.resync_saved_flag();
        }
        replaced
    }

    /// Puente EditSink (§8.6): borra el rango `[from, to]`. Un
    /// [`opm_document::EditError`] improbable se ignora documentadamente:
    /// sin panics en la UI.
    pub fn delete_active(&mut self, from: Position, to: Position) {
        let _ = self.tabs[self.active]
            .editor
            .replace(Range::new(from, to), "");
    }

    /// Puente EditSink (§8.6): fija la selección del tab activo. Un
    /// [`opm_document::EditError`] improbable se ignora documentadamente:
    /// sin panics en la UI.
    pub fn set_selection_active(&mut self, selection: Selection) {
        let _ = self.tabs[self.active].editor.set_selection(selection);
    }

    /// Re-objetiva las pestañas afectadas por un rename/movimiento §16
    /// (ISSUE-023): `from` y sus descendientes pasan a `to`. El buffer no se
    /// toca (§6.1); solo cambia el archivo asignado. Las huellas de guardado
    /// se re-llavan con las rutas (P2-9: sin eso, el siguiente guardado en
    /// la ruta nueva reportaría un falso cambio externo §12.7).
    ///
    /// La comparación y el sufijo son por segmentos con separadores
    /// normalizados (P1): `folder\file.md` sigue bajo `folder` y el sufijo
    /// se re-pega sin `join("")` — esa barra final rompería el siguiente
    /// guardado (ENOTDIR) cuando el rename es directo de un fichero.
    pub fn retarget_path(&mut self, from: &Path, to: &Path) {
        for tab in &mut self.tabs {
            let new_path = tab
                .path
                .as_deref()
                .and_then(|path| paths::retargeted(path, from, to));
            let Some(new_path) = new_path else {
                continue;
            };
            let old = tab.path.take().expect("afectado ⇒ con path");
            tab.editor.document_mut().set_path(new_path.clone());
            tab.path = Some(new_path.clone());
            // El movimiento es una operación propia (ISSUE-023/026): no es un
            // cambio externo §12.7.
            tab.external_change = false;
            self.fs.retarget_stamp(&old, &new_path);
        }
    }

    /// El archivo `path` (o una carpeta que lo contiene) se eliminó §14.9.
    ///
    /// El llamador YA pidió confirmación explícita cuando había pestañas
    /// sucias dentro (§3.7 / DECISIONS: «Discard these changes and delete?»);
    /// tras esa confirmación TODAS las pestañas bajo `path` se descartan
    /// (P2-14, paridad SPA `handleDeletePath`): el prompt promete descartar,
    /// así que el buffer sucio NO se conserva desasociado como «Sin título
    /// N» — la confirmación (no el silencio) es la garantía.
    ///
    /// Las huellas de guardado de las rutas borradas se descartan con ellas
    /// (P2-9).
    ///
    /// Invariante §37: siempre queda ≥1 tab.
    ///
    /// La selección es por segmentos con separadores normalizados (P1):
    /// `folder\file.md` también queda bajo `folder` al borrar.
    pub fn path_deleted(&mut self, path: &Path) {
        // P2-9: la huella de un fichero borrado propio no decide sobre nada.
        self.fs.forget_stamps_under(path);
        let old_active = self.active;
        let mut index = 0;
        let mut removed_before = 0;
        while index < self.tabs.len() {
            let affected = self.tabs[index]
                .path
                .as_ref()
                .is_some_and(|p| paths::is_descendant(p, path));
            if !affected {
                index += 1;
                continue;
            }
            if index < old_active {
                removed_before += 1;
            }
            self.tabs.remove(index);
            self.saved.remove(index);
        }
        if self.tabs.is_empty() {
            self.new_tab();
        } else {
            // El activo sobreviviente mantiene su posición relativa; el
            // activo descartado pasa a su sucesor (acotado al final).
            self.active = (old_active - removed_before).min(self.tabs.len() - 1);
        }
    }

    /// Tab vacío «Sin título N» con el siguiente número (§37).
    fn blank_tab(&mut self) -> Tab {
        let untitled_number = self.next_untitled;
        self.next_untitled += 1;
        Tab {
            editor: Editor::new(self.fs.create_new()),
            mode: EditMode::Markdown,
            path: None,
            untitled_number,
            external_change: false,
        }
    }

    /// Añade un tab al final y lo activa; devuelve su índice.
    fn push_tab(&mut self, tab: Tab, saved: String) -> usize {
        self.tabs.push(tab);
        self.saved.push(saved);
        self.active = self.tabs.len() - 1;
        self.active
    }

    /// Tras un guardado exitoso actualiza la base «contenido en disco» (§6.2)
    /// y limpia el aviso de cambio externo (§12.7: el guardado lo resuelve).
    fn record_saved(&mut self, index: usize, result: &Result<(), FsError>) {
        if result.is_ok() {
            self.saved[index] = self.tabs[index].editor.document().content().to_string();
            self.tabs[index].external_change = false;
        }
    }

    /// El flag `modified` del `Document` se marca en cada edición y solo
    /// `mark_saved` lo limpia; tras undo/redo se re-sincroniza con la realidad
    /// «buffer == contenido guardado» (§6.2) para que el `●` de §37 se
    /// corresponda con cambios sin guardar de verdad.
    fn resync_saved_flag(&mut self) {
        let index = self.active;
        let tab = &mut self.tabs[index];
        let document = tab.editor.document_mut();
        if document.is_modified() && document.content() == self.saved[index].as_str() {
            document.mark_saved();
        }
    }
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Directorio temporal de test, eliminado al salir.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("opm-tabs-{}-{}", std::process::id(), nanos));
            fs::create_dir_all(&path).unwrap();
            Self(path)
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
    fn abrir_y_guardar_preserva_el_contenido_byte_a_byte() {
        // §12.3 guardado explícito; §12.6: LF permanece LF.
        let dir = TestDir::new();
        let path = dir.file("archivo.md");
        fs::write(&path, "# Título\n\ncuerpo\n").unwrap();

        let mut tabs = TabManager::new();
        let index = tabs.open(&path).unwrap();
        assert_eq!(tabs.active_index(), index);
        assert_eq!(
            tabs.active().editor.document().content(),
            "# Título\n\ncuerpo\n"
        );
        assert_eq!(tabs.active().path, Some(path.clone()));
        // §37: sin `●` recién abierto.
        assert_eq!(tabs.title(index), "archivo.md");

        // «cuerpo» → «cuerpo editado» (posición semántica, §8.6).
        tabs.insert_active(Position::new(2, 6), " editado");
        assert_eq!(tabs.title(index), "● archivo.md");
        assert!(tabs.is_modified(index));

        tabs.save_active().unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            "# Título\n\ncuerpo editado\n".as_bytes().to_vec()
        );
        assert!(!tabs.is_modified(index));
        assert_eq!(tabs.title(index), "archivo.md");
    }

    #[test]
    fn save_sin_path_da_missing_path_y_save_as_habilita_save() {
        // §12.4: sin archivo asignado toca «Guardar como».
        let dir = TestDir::new();
        let path = dir.file("nuevo.md");
        let mut tabs = TabManager::new();
        tabs.insert_active(Position::new(0, 0), "hola\n");
        assert!(matches!(tabs.save_active(), Err(FsError::MissingPath)));

        tabs.save_active_as(&path).unwrap();
        assert_eq!(tabs.active().path, Some(path.clone()));
        assert_eq!(fs::read_to_string(&path).unwrap(), "hola\n");
        assert!(!tabs.is_modified(0));
        assert_eq!(tabs.title(0), "nuevo.md");

        // Tras Save As, un Save normal ya funciona (§12.3).
        tabs.insert_active(Position::new(1, 0), "más\n");
        assert_eq!(tabs.title(0), "● nuevo.md");
        tabs.save_active().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hola\nmás\n");
    }

    #[test]
    fn retarget_y_borrado_reobjetivan_pestanas() {
        // ISSUE-023 criterio 5: rename/move re-objetiva `Tab::path` sin tocar
        // el buffer (§6.1); el borrado (§14.9) cierra las pestañas afectadas
        // — el llamador YA pidió confirmación si estaban editadas (P2-14,
        // paridad SPA `handleDeletePath`: tras «Discard these changes and
        // delete?» el buffer se descarta de verdad, §3.7).
        let dir = TestDir::new();
        let a = dir.file("cap/a.md");
        let b = dir.file("b.md");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::write(&a, "contenido a\n").unwrap();
        fs::write(&b, "contenido b\n").unwrap();

        let mut tabs = TabManager::new();
        tabs.open(&a).unwrap();
        let index_b = tabs.open(&b).unwrap();
        tabs.insert_active(Position::new(0, 11), "!"); // «contenido b!\n»

        // Movimiento `cap` → `capitulos` (§16 ya aplicado al filesystem).
        tabs.retarget_path(&dir.file("cap"), &dir.file("capitulos"));
        let movido = tabs
            .tabs()
            .iter()
            .find(|t| t.path.as_ref().is_some_and(|p| p.ends_with("a.md")))
            .unwrap();
        assert_eq!(movido.path, Some(dir.file("capitulos/a.md")));
        assert_eq!(
            movido.editor.document().path(),
            Some(dir.file("capitulos/a.md").as_path())
        );
        assert_eq!(movido.editor.document().content(), "contenido a\n");
        assert_eq!(tabs.tabs()[index_b].path, Some(dir.file("b.md")));

        // Borrado de un archivo limpio: su pestaña se cierra.
        let antes = tabs.len();
        tabs.path_deleted(&dir.file("capitulos/a.md"));
        assert_eq!(tabs.len(), antes - 1);

        // Borrado de un archivo editado: tras la confirmación del llamador
        // el buffer se DESCARTA con su pestaña (P2-14: el prompt «Discard
        // these changes and delete?» lo promete; no vive como «Sin título»).
        let antes_sucias = tabs.len();
        tabs.path_deleted(&dir.file("b.md"));
        assert_eq!(tabs.len(), antes_sucias - 1);
        assert!(
            !tabs
                .tabs()
                .iter()
                .any(|t| t.editor.document().content() == "contenido b!\n"),
            "P2-14: el buffer sucio confirmado se descarta, no se conserva"
        );
        assert!(tabs.title(tabs.active_index()).contains("Sin título"));
        assert!(tabs.len() >= 1, "§37: siempre queda al menos un tab");
    }

    #[test]
    fn path_deleted_cierra_las_formas_mixtas_y_respeta_los_hermanos() {
        // P1 (paridad con el fix Python): la selección de borrado compara
        // por segmentos con separadores normalizados — una pestaña
        // serializada `folder\file.md` cierra con `folder`, mientras
        // `folderx` NO es descendiente (prefijo crudo ≠ segmento).
        let dir = TestDir::new();
        fs::create_dir_all(dir.file("folder")).unwrap();
        fs::create_dir_all(dir.file("folderx")).unwrap();
        let dentro = dir.file("folder/file.md");
        let vecino = dir.file("folderx/other.md");
        fs::write(&dentro, "uno\n").unwrap();
        fs::write(&vecino, "dos\n").unwrap();

        let mut tabs = TabManager::new();
        let index_dentro = tabs.open(&dentro).unwrap();
        tabs.open(&vecino).unwrap();

        // MISMA ruta de la pestaña serializada con `\` (estado persistido
        // en Windows): sintética, no hace falta que exista en disco.
        let mixta = PathBuf::from(
            dentro
                .to_string_lossy()
                .replacen("folder/file.md", "folder\\file.md", 1),
        );
        tabs.tabs[index_dentro].path = Some(mixta.clone());

        tabs.path_deleted(&dir.file("folder"));

        assert!(
            !tabs
                .tabs()
                .iter()
                .any(|tab| tab.path.as_deref() == Some(mixta.as_path())),
            "P1: la pestaña serializada folder\\file.md se cierra con folder"
        );
        assert!(
            tabs.tabs()
                .iter()
                .any(|tab| tab.path.as_deref() == Some(vecino.as_path())),
            "P1: folderx NO es descendiente de folder y su pestaña sobrevive"
        );
        assert!(tabs.len() >= 1, "§37: siempre queda al menos un tab");
    }

    #[test]
    fn retarget_path_con_forma_mixta_repega_el_sufijo() {
        // P1: `cap\a.md` (serialización Windows persistida) sigue bajo
        // `cap`; tras mover `cap`→`capitulos` la pestaña queda en
        // `capitulos/a.md` — con el `strip_prefix` nativo el sufijo se
        // perdía y la pestaña apuntaría a la carpeta destino sin fichero.
        let dir = TestDir::new();
        fs::create_dir_all(dir.file("cap")).unwrap();
        let real = dir.file("cap/a.md");
        fs::write(&real, "contenido\n").unwrap();

        let mut tabs = TabManager::new();
        let index = tabs.open(&real).unwrap();
        let mixta = PathBuf::from(real.to_string_lossy().replacen("cap/a.md", "cap\\a.md", 1));
        tabs.tabs[index].path = Some(mixta);

        tabs.retarget_path(&dir.file("cap"), &dir.file("capitulos"));

        assert_eq!(
            tabs.tabs()[index].path,
            Some(dir.file("capitulos/a.md")),
            "el sufijo se re-pega por segmentos"
        );
        assert_eq!(
            tabs.tabs()[index].editor.document().path(),
            Some(dir.file("capitulos/a.md").as_path()),
            "§12.4: el Document espejea el path del tab"
        );
        assert_eq!(
            tabs.tabs()[index].editor.document().content(),
            "contenido\n",
            "§6.1: el buffer no se toca"
        );
    }

    #[test]
    fn aviso_cambio_externo_12_7_sin_recargar_buffer() {
        // §12.7 (ISSUE-026): el cambio externo se marca en el título (§3.7)
        // sin tocar el buffer; guardar lo resuelve.
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "original\n").unwrap();
        let mut tabs = TabManager::new();
        let index = tabs.open(&path).unwrap();
        let contenido = tabs.active().editor.document().content().to_string();

        // Evento sobre el archivo abierto: marca sin recarga.
        tabs.mark_external(&path);
        assert_eq!(tabs.title(index), "⚠ doc.md");
        assert_eq!(
            tabs.active().editor.document().content(),
            contenido,
            "§12.7: el buffer nunca se recarga solo"
        );

        // Modificado + aviso conviven (§37).
        tabs.insert_active(Position::new(0, 8), "X");
        assert_eq!(tabs.title(index), "●⚠ doc.md");

        // El guardado resuelve el aviso.
        tabs.save_active().unwrap();
        assert_eq!(tabs.title(index), "doc.md");

        // Un evento sobre la carpeta afecta a las pestañas hijas.
        tabs.mark_external(&dir.0);
        assert_eq!(tabs.title(index), "⚠ doc.md");
    }

    #[test]
    fn rename_propio_y_guardar_no_produce_falso_cambio_externo() {
        // P2-9: el rename propio re-llava la huella (TabManager → LocalFs);
        // guardar el buffer sucio en la ruta renombrada NO pide sobrescritura
        // externa (§12.7: solo los cambios DE VERDAD externos bloquean).
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "original\n").unwrap();

        let mut tabs = TabManager::new();
        tabs.open(&path).unwrap();
        tabs.insert_active(Position::new(0, 8), "!"); // «original!\n» sucio

        // El rename en disco lo hace `opm_fs::rename_project_path` en
        // producción; aquí basta `fs::rename` + la re-objetivación que hace
        // el explorador vía `TabManager::retarget_path`.
        let renamed = dir.file("renombrado.md");
        fs::rename(&path, &renamed).unwrap();
        tabs.retarget_path(&path, &renamed);
        assert_eq!(tabs.active().path, Some(renamed.clone()));

        // Sin re-llavar la huella esto fallaría con `ExternalModification`.
        tabs.save_active().unwrap();
        assert_eq!(fs::read_to_string(&renamed).unwrap(), "original!\n");
        assert_eq!(tabs.title(tabs.active_index()), "renombrado.md");
    }

    #[test]
    fn mark_external_changes_detecta_discos_sin_recargar_buffers() {
        // P2-13: el Refresh del explorador marca con ⚠ los ficheros abiertos
        // que cambiaron en disco (huella del LocalFs, §12.7) sin tocar
        // ningún buffer — la detección vive en producción.
        let dir = TestDir::new();
        let a = dir.file("a.md");
        let b = dir.file("b.md");
        fs::write(&a, "a\n").unwrap();
        fs::write(&b, "b\n").unwrap();

        let mut tabs = TabManager::new();
        let index_a = tabs.open(&a).unwrap();
        let index_b = tabs.open(&b).unwrap();

        // Solo `a` cambia fuera de OPM (longitud distinta: detectable con
        // mtime de baja resolución).
        fs::write(&a, "cambiado externamente\n").unwrap();

        tabs.mark_external_changes();
        assert!(
            tabs.title(index_a).contains('⚠'),
            "a marcada: {}",
            tabs.title(index_a)
        );
        assert!(
            !tabs.title(index_b).contains('⚠'),
            "b intacta: {}",
            tabs.title(index_b)
        );
        assert_eq!(
            tabs.tabs()[index_a].editor.document().content(),
            "a\n",
            "§12.7: el buffer nunca se recarga solo"
        );
    }

    #[test]
    fn cambio_externo_bloquea_save_y_confirmado_sobrescribe() {
        // §12.7: nunca sobrescrituras silenciosas; §3.7: solo con confirmación
        // explícita se sobrescribe el cambio externo.
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "original\n").unwrap();

        let mut tabs = TabManager::new();
        let index = tabs.open(&path).unwrap();
        tabs.insert_active(Position::new(0, 8), "X"); // «originalX\n»

        // Otro programa cambia el fichero (longitud distinta: detectable
        // incluso con mtime de baja resolución).
        fs::write(&path, "externo\n").unwrap();
        let error = tabs.save_active();
        assert!(matches!(&error, Err(FsError::ExternalModification { path: p }) if *p == path));
        // El disco queda intacto y el buffer no se pierde (§12.7).
        assert_eq!(fs::read_to_string(&path).unwrap(), "externo\n");
        assert_eq!(tabs.active().editor.document().content(), "originalX\n");
        assert!(tabs.is_modified(index));

        // §3.7: confirmación explícita → sobrescribe con el buffer.
        tabs.save_active_confirmed().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "originalX\n");
        assert!(!tabs.is_modified(index));
        assert_eq!(tabs.title(index), "doc.md");
    }

    #[test]
    fn pestanas_titulos_editado_y_cierre_conserva_un_tab() {
        // §37: «Sin título N» con N único y consecutivo desde 1, «●» solo en
        // el tab editado y siempre queda ≥1 tab.
        let mut tabs = TabManager::new();
        assert_eq!(tabs.title(0), "Sin título 1");

        // `new_tab` ×3 al empezar con 1 tab: índices 1..3 y títulos
        // «Sin título 2..4» (el 1 es el inicial).
        assert_eq!([tabs.new_tab(), tabs.new_tab(), tabs.new_tab()], [1, 2, 3]);
        assert_eq!(tabs.len(), 4);
        assert_eq!(tabs.title(0), "Sin título 1");
        assert_eq!(tabs.title(1), "Sin título 2");
        assert_eq!(tabs.title(2), "Sin título 3");
        assert_eq!(tabs.title(3), "Sin título 4");

        // Editar un tab marca `●` solo en ese tab (§37).
        tabs.set_active(1);
        tabs.insert_active(Position::new(0, 0), "x");
        assert_eq!(tabs.title(0), "Sin título 1");
        assert_eq!(tabs.title(1), "● Sin título 2");
        assert_eq!(tabs.title(2), "Sin título 3");
        assert_eq!(tabs.title(3), "Sin título 4");

        // Cerrar el activo deja ≥1 tab y el activo pasa al siguiente.
        tabs.close_active();
        assert_eq!(tabs.len(), 3);
        assert_eq!(tabs.active_index(), 1);
        assert_eq!(tabs.title(1), "Sin título 3");

        // Cerrar el último crea un tab nuevo «Sin título N» (§37).
        tabs.close_active();
        tabs.close_active();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.title(0), "Sin título 1");
        tabs.close_active();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.title(0), "Sin título 5");
    }

    #[test]
    fn undo_redo_revierten_buffer_y_modificado_y_modo_conserva_historial() {
        // §8.5: el historial vive en el Editor y cambiar de modo no lo
        // reinicia; §6.2: undo devuelve el indicador a «sin cambios».
        let mut tabs = TabManager::new();
        tabs.insert_active(Position::new(0, 0), "hola");
        assert_eq!(tabs.active().editor.document().content(), "hola");
        assert!(tabs.is_modified(0));

        // undo revierte buffer y `is_modified`.
        assert!(tabs.undo_active());
        assert_eq!(tabs.active().editor.document().content(), "");
        assert!(!tabs.is_modified(0));

        // redo reaplica.
        assert!(tabs.redo_active());
        assert_eq!(tabs.active().editor.document().content(), "hola");
        assert!(tabs.is_modified(0));

        // §8.6: cambiar de modo conserva la selección semántica.
        let seleccion = Selection {
            anchor: Position::new(0, 1),
            head: Position::new(0, 3),
        };
        tabs.set_selection_active(seleccion);
        assert!(matches!(tabs.toggle_mode_active(), EditMode::Visual));
        assert_eq!(tabs.active().editor.selection(), seleccion);
        assert!(matches!(tabs.toggle_mode_active(), EditMode::Markdown));
        assert_eq!(tabs.active().editor.selection(), seleccion);

        // §8.5: cambiar de modo no vacía el historial.
        assert!(matches!(tabs.toggle_mode_active(), EditMode::Visual));
        assert!(tabs.undo_active());
        assert_eq!(tabs.active().editor.document().content(), "");
        assert!(!tabs.is_modified(0));
        assert!(!tabs.undo_active());
    }

    #[test]
    fn reload_from_disk_recarga_en_silencio_la_pestana_limpia() {
        // §12.7: la recarga es solo para escrituras PROPIAS (reemplazo masivo
        // §40.2); la escritura nuestra no avisa como externa: el buffer pasa
        // a ser el de disco, sin «●» y con el banderín `external_change`
        // limpio.
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "original\n").unwrap();
        let mut tabs = TabManager::new();
        let index = tabs.open(&path).unwrap();

        // Escritura propia en disco + aviso del watcher (§46) que la recarga
        // debe borrar.
        fs::write(&path, "reemplazado\n").unwrap();
        tabs.mark_external(&path);
        assert_eq!(tabs.title(index), "⚠ doc.md");

        tabs.reload_from_disk(&path).unwrap();
        assert_eq!(tabs.active().editor.document().content(), "reemplazado\n");
        assert!(!tabs.is_modified(index), "§37: sin ●");
        assert_eq!(tabs.title(index), "doc.md", "sin ● ni ⚠");

        // El guardado posterior no ve «cambio externo» (§12.7): la recarga
        // actualizó la huella del archivo.
        tabs.insert_active(Position::new(0, 0), "# ");
        tabs.save_active().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# reemplazado\n");
    }

    #[test]
    fn reload_from_disk_no_toca_la_pestana_modificada() {
        // §12.7: una pestaña con cambios sin guardar NUNCA se recarga ni se
        // sobrescribe; el integrador la marca con `mark_external` y el
        // usuario decide al guardar (§3.7).
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "disco\n").unwrap();
        let mut tabs = TabManager::new();
        let index = tabs.open(&path).unwrap();
        tabs.insert_active(Position::new(0, 5), "!"); // «disco!\n»
        tabs.mark_external(&path);

        fs::write(&path, "otro\n").unwrap();
        tabs.reload_from_disk(&path).unwrap();

        assert_eq!(
            tabs.active().editor.document().content(),
            "disco!\n",
            "§12.7: el buffer con cambios no se toca"
        );
        assert!(tabs.is_modified(index), "§37: sigue con ●");
        assert_eq!(tabs.title(index), "●⚠ doc.md");

        // Un `path` sin pestaña abierta es un no-op (no abre ni cierra).
        let abiertas = tabs.len();
        tabs.reload_from_disk(&dir.file("otro.md")).unwrap();
        assert_eq!(tabs.len(), abiertas);
    }

    #[test]
    fn reload_from_disk_conserva_historial_y_cursor_8_5_8_6() {
        // §8.5: la recarga por escritura propia no es transacción del
        // historial compartido por los modos — no gana entradas ni lo
        // reinicia — y §8.6 conserva el cursor (posición semántica).
        let dir = TestDir::new();
        let path = dir.file("doc.md");
        fs::write(&path, "uno\ndos\n").unwrap();
        let mut tabs = TabManager::new();
        let index = tabs.open(&path).unwrap();

        // Sin ediciones: la recarga no puede ganar transacciones.
        fs::write(&path, "uno\ntres\n").unwrap();
        tabs.reload_from_disk(&path).unwrap();
        assert!(!tabs.active().editor.can_undo(), "recarga ≠ transacción");
        assert!(!tabs.active().editor.can_redo());

        // Historial real (edición guardada §12.3: pestaña limpia con undo).
        tabs.insert_active(Position::new(1, 2), "X"); // «trXes»
        tabs.save_active().unwrap();
        tabs.set_selection_active(Selection::collapsed(Position::new(0, 1)));
        assert!(!tabs.is_modified(index));
        assert!(tabs.active().editor.can_undo());

        fs::write(&path, "cuatro\ncinco\n").unwrap();
        tabs.reload_from_disk(&path).unwrap();

        assert_eq!(tabs.active().editor.document().content(), "cuatro\ncinco\n");
        // §8.5: el historial queda exactamente como estaba.
        assert!(
            tabs.active().editor.can_undo(),
            "el historial no se reinicia"
        );
        assert!(!tabs.active().editor.can_redo());
        // §8.6: el cursor se conserva y el índice de líneas es el del
        // contenido nuevo (insertar en (0,1) cae donde toca).
        assert_eq!(
            tabs.active().editor.selection(),
            Selection::collapsed(Position::new(0, 1))
        );
        tabs.insert_active(Position::new(0, 1), "Z");
        assert_eq!(
            tabs.active().editor.document().content(),
            "cZuatro\ncinco\n"
        );
    }
}
