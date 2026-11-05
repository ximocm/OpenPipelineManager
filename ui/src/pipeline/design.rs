//! Documento de diseño (pipeline-storage): el `PipelineDoc` de la librería
//! editado EN MEMORIA por una ventana de diseño, sin carpeta de trabajo.
//!
//! - Las mutaciones estructurales (crear/editar paso, dependencias, layout)
//!   viven SOLO aquí y marcan el documento como sucio; nada toca disco hasta
//!   que el usuario guarda en la librería (`Save`/`Save As` con elección
//!   explícita ante nombres ocupados) o exporta.
//! - La selección de run por paso (`StepState.selected`) es estado de vista
//!   del diseño: no forma parte del documento ni lo ensucia.
//! - La validación de diseño recalcula con [`opm_pipeline::validate_pipeline`]
//!   sobre la raíz de la librería y DESCARTA los issues que solo un-folder
//!   puede satisfacer (existencia de working directory/ficheros de entrada):
//!   en diseño no existe todavía ningún directorio de trabajo.
//!
//! Modelo puro: testeable sin display ni main loop de GTK.

use std::collections::HashMap;
use std::path::Path;

use opm_fs::safe_segment;
use opm_pipeline::{
    validate_pipeline, FieldType, PipelineConfig, PipelineStep, ValidationIssue,
};
use opm_store::{
    PipelineDoc, Position, ProjectSnapshot, SaveMode, StepDetail, StepState, StoreError,
    PipelineLibrary,
};

/// Estado de diseño de UNA ventana: documento de librería + bandera de
/// cambios sin guardar + nombre bajo el que se guardó (si ya se guardó).
pub struct DesignState {
    doc: PipelineDoc,
    /// Selección de run por paso (vista, no persistida en el documento).
    states: HashMap<String, StepState>,
    dirty: bool,
    /// Stem de librería tras un `Open`/`Save` — `None` = nunca guardado.
    saved_name: Option<String>,
}

impl DesignState {
    /// Diseño nuevo y vacío con `name` (sin guardar en la librería).
    pub fn new(name: &str) -> Self {
        Self {
            doc: PipelineDoc::new(name, PipelineConfig::default()),
            states: HashMap::new(),
            dirty: false,
            saved_name: None,
        }
    }

    /// Diseño cargado de la librería (`saved_name` = stem del documento).
    pub fn from_doc(doc: PipelineDoc, saved_name: Option<String>) -> Self {
        Self {
            doc,
            states: HashMap::new(),
            dirty: false,
            saved_name,
        }
    }

    pub fn doc(&self) -> &PipelineDoc {
        &self.doc
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn saved_name(&self) -> Option<&str> {
        self.saved_name.as_deref()
    }

    /// Nombre mostrable: el stem guardado, si no el nombre del documento.
    pub fn display_name(&self) -> &str {
        self.saved_name
            .as_deref()
            .unwrap_or_else(|| self.doc.name.as_str())
    }

    /// Tras guardar: nombre fijado, documento limpio.
    pub fn mark_saved(&mut self, stem: &str) {
        self.doc.name = stem.to_owned();
        self.saved_name = Some(stem.to_owned());
        self.dirty = false;
    }

    /// Documento listo para escribir en la librería: clon con `name = stem`
    /// y una entrada de layout por cada paso (SIN re-cadenar dependencias:
    /// el grafo que ve el usuario es exactamente el que se guarda).
    pub fn document_for_save(&self, stem: &str) -> PipelineDoc {
        let mut doc = self.doc.clone();
        doc.name = stem.to_owned();
        for (index, step) in doc.pipeline.steps.iter().enumerate() {
            doc.layout
                .entry(step.id.clone())
                .or_insert_with(|| default_step_position(index));
        }
        doc
    }

    // --- lecturas -----------------------------------------------------------

    pub fn steps(&self) -> Vec<PipelineStep> {
        self.doc.pipeline.steps.clone()
    }

    pub fn step(&self, step_id: &str) -> Option<PipelineStep> {
        self.doc
            .pipeline
            .steps
            .iter()
            .find(|step| step.id == step_id)
            .cloned()
    }

    pub fn state_of(&self, step_id: &str) -> StepState {
        self.states.get(step_id).cloned().unwrap_or_default()
    }

    /// Vista del diseño para el lienzo/contadores (SIN carpeta de trabajo:
    /// `project_path` vacío y `params` sin valores — los valores son de la
    /// ejecución; en diseño manda el default declarado).
    pub fn snapshot(&self, validation: Vec<ValidationIssue>) -> ProjectSnapshot {
        ProjectSnapshot {
            project_path: String::new(),
            pipeline: Some(self.doc.pipeline.clone()),
            params: HashMap::new(),
            state: self.states.clone(),
            visual_layout: self.doc.layout.clone().into_iter().collect(),
            validation,
        }
    }

    /// Detalle del paso para el panel: valores = defaults declarados
    /// (`params` vacío), estado de vista y validación filtrada al paso.
    pub fn step_detail(
        &self,
        step_id: &str,
        validation: &[ValidationIssue],
    ) -> Option<StepDetail> {
        let step = self.step(step_id)?;
        let validation = validation
            .iter()
            .filter(|issue| issue.step_id.as_deref() == Some(step_id))
            .cloned()
            .collect();
        Some(StepDetail {
            step,
            params: Default::default(),
            state: self.state_of(step_id),
            validation,
        })
    }

    // --- mutaciones (todas ensucian el documento, salvo la selección) ------

    /// Crea un paso (reglas espejo de `ProjectStore::create_step`): id
    /// normalizado, sin duplicados y dependencias por defecto al último paso.
    pub fn create_step(&mut self, step: PipelineStep) -> Result<(), StoreError> {
        let step_id = normalize_step_id(&step.id)?;
        if self
            .doc
            .pipeline
            .steps
            .iter()
            .any(|existing| existing.id == step_id)
        {
            return Err(StoreError::StepExists(step_id));
        }
        let dependencies = if step.dependencies.is_empty() {
            self.doc
                .pipeline
                .steps
                .last()
                .map(|last| vec![last.id.clone()])
                .unwrap_or_default()
        } else {
            step.dependencies.clone()
        };
        let normalized = normalized_step(&step, &step_id, &dependencies);
        self.doc.pipeline.steps.push(normalized);
        let index = self.doc.pipeline.steps.len() - 1;
        self.doc
            .layout
            .entry(step_id.clone())
            .or_insert_with(|| default_step_position(index));
        self.dirty = true;
        Ok(())
    }

    /// Edita un paso; un rename propaga a dependencias y enlaces
    /// `source_step` y migra layout/estado (espejo de `update_step`, sin
    /// artefactos de ejecución: en diseño no hay `run.json` ni `.done`).
    pub fn update_step(&mut self, old_id: &str, step: PipelineStep) -> Result<(), StoreError> {
        let current_index = self
            .doc
            .pipeline
            .steps
            .iter()
            .position(|item| item.id == old_id)
            .ok_or_else(|| StoreError::UnknownStep(old_id.to_string()))?;
        let new_step_id = normalize_step_id(&step.id)?;
        if self
            .doc
            .pipeline
            .steps
            .iter()
            .any(|item| item.id == new_step_id && item.id != old_id)
        {
            return Err(StoreError::StepExists(new_step_id));
        }
        let normalized = normalized_step(&step, &new_step_id, &step.dependencies);
        self.doc.pipeline.steps[current_index] = normalized;
        if new_step_id != old_id {
            for item in &mut self.doc.pipeline.steps {
                for dependency in &mut item.dependencies {
                    if dependency == old_id {
                        *dependency = new_step_id.clone();
                    }
                }
                for input in &mut item.inputs {
                    if input.source_step.as_deref() == Some(old_id) {
                        input.source_step = Some(new_step_id.clone());
                    }
                }
            }
            let state = self.states.remove(old_id).unwrap_or_default();
            self.states.insert(new_step_id.clone(), state);
            let position = self
                .doc
                .layout
                .remove(old_id)
                .unwrap_or_else(|| default_step_position(current_index));
            self.doc.layout.insert(new_step_id.clone(), position);
        } else {
            self.states.entry(new_step_id.clone()).or_default();
            self.doc
                .layout
                .entry(new_step_id.clone())
                .or_insert_with(|| default_step_position(current_index));
        }
        self.dirty = true;
        Ok(())
    }

    /// Reemplaza las dependencias de un paso (dedup, sin auto-dependencia ni
    /// referencias desconocidas — espejo de `set_dependencies`).
    pub fn set_dependencies(
        &mut self,
        step_id: &str,
        dependencies: Vec<String>,
    ) -> Result<(), StoreError> {
        self.step(step_id)
            .ok_or_else(|| StoreError::UnknownStep(step_id.to_string()))?;
        let known: Vec<&str> = self
            .doc
            .pipeline
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .collect();
        let normalized = dedup(&dependencies);
        if normalized.iter().any(|dependency| dependency == step_id) {
            return Err(StoreError::SelfDependency);
        }
        if let Some(unknown) = normalized
            .iter()
            .find(|dependency| !known.contains(&dependency.as_str()))
        {
            return Err(StoreError::UnknownDependency(unknown.clone()));
        }
        let step = self
            .doc
            .pipeline
            .steps
            .iter_mut()
            .find(|item| item.id == step_id)
            .expect("step exists");
        step.dependencies = normalized;
        self.dirty = true;
        Ok(())
    }

    /// Posición de un nodo movido (SOLO el nodo dado; ensucia el layout).
    pub fn set_layout(&mut self, step_id: &str, x: f64, y: f64) {
        self.doc.layout.insert(step_id.to_owned(), Position { x, y });
        self.dirty = true;
    }

    /// Alterna la selección de run (vista del diseño; NO ensucia el documento
    /// — la selección vive en `run.json` de la carpeta de trabajo).
    pub fn toggle_selected(&mut self, step_id: &str) -> StepState {
        let state = self.states.entry(step_id.to_owned()).or_default();
        state.selected = !state.selected;
        state.clone()
    }

    /// Crear (sin `original_id`) o editar (con `original_id`), delegando en
    /// [`Self::create_step`] / [`Self::update_step`].
    pub fn submit(
        &mut self,
        original_id: Option<&str>,
        step: PipelineStep,
    ) -> Result<(), StoreError> {
        match original_id {
            Some(old_id) => self.update_step(old_id, step),
            None => self.create_step(step),
        }
    }
}

/// Validación de DISEÑO: igual que la del store pero descartando los issues
/// que solo existen dentro de una carpeta de trabajo (existencia del working
/// directory y de ficheros/carpetas de entrada declarados). El resto —
/// ids duplicados, ciclos, dependencias desconocidas, rutas inseguras,
/// placeholders sin valor, declaraciones incompletas — es feedback de diseño.
pub fn design_validation(pipeline: &PipelineConfig, root: &Path) -> Vec<ValidationIssue> {
    let issues = validate_pipeline(pipeline, root, &HashMap::new());
    issues
        .into_iter()
        .filter(|issue| !work_folder_only_issue(issue, pipeline))
        .collect()
}

/// ¿Este issue solo lo satisface una carpeta de trabajo (existencia de
/// ficheros)? Identificado por SU CAMPO Y el prefijo exacto del mensaje
/// (`label_or_key` + `: `), nunca por substring suelto: «Dependency does not
/// exist»/«Input source … does not exist» son estructurales y se conservan.
fn work_folder_only_issue(issue: &ValidationIssue, pipeline: &PipelineConfig) -> bool {
    if issue.field.as_deref() == Some("working_directory")
        && issue.message.starts_with("Working directory does not exist: ")
    {
        return true;
    }
    let (Some(step_id), Some(field)) = (issue.step_id.as_deref(), issue.field.as_deref()) else {
        return false;
    };
    let Some(step) = pipeline.steps.iter().find(|step| step.id == step_id) else {
        return false;
    };
    step.inputs.iter().any(|spec| {
        matches!(spec.kind, FieldType::File | FieldType::Folder)
            && spec.key == field
            && issue.message.starts_with(&format!("{} does not exist: ", label_or_key(spec)))
    })
}

/// Etiqueta visible de un campo (misma regla que el panel y `validate`).
fn label_or_key(spec: &opm_pipeline::ValueSpec) -> &str {
    spec.label
        .as_deref()
        .filter(|label| !label.is_empty())
        .unwrap_or(&spec.key)
}

/// Stem de librería para un nombre de usuario (espejo de
/// `opm_store::library_stem`, que es privado): recorta, quita un sufijo
/// `.pipeline` redundante, sanea a un segmento y rechaza el nombre vacío.
pub fn library_stem(name: &str) -> Result<String, StoreError> {
    let trimmed = name.trim();
    let stem = trimmed
        .strip_suffix(".pipeline")
        .map(str::trim)
        .unwrap_or(trimmed);
    if stem.is_empty() {
        return Err(StoreError::Message("Pipeline name is required".to_string()));
    }
    Ok(safe_segment(stem))
}

/// Carga una definición desde un fichero: `.pipeline` nativo (JSON con
/// `format_version` comprobado) o YAML/JSON de intercambio
/// (`opm_pipeline::parse_pipeline` + envoltorio [`PipelineDoc`]). El nombre
/// de librería lo aporta el llamador (el fallback de «Update from library»
/// y el path de sobrescritura de Import).
pub fn definition_from_file(path: &Path, name: &str) -> Result<PipelineDoc, StoreError> {
    let text = std::fs::read_to_string(path).map_err(StoreError::Io)?;
    if path.extension().and_then(|ext| ext.to_str()) == Some("pipeline") {
        let doc: PipelineDoc = serde_json::from_str(&text)
            .map_err(|err| StoreError::Message(format!("{}: {err}", path.display())))?;
        if doc.format_version != opm_store::FORMAT_VERSION {
            return Err(StoreError::Message(format!(
                "{}: unsupported pipeline format version {} (expected {})",
                path.display(),
                doc.format_version,
                opm_store::FORMAT_VERSION
            )));
        }
        return Ok(doc);
    }
    let pipeline = opm_pipeline::parse_pipeline(&path.to_string_lossy(), &text)
        .map_err(|err| StoreError::Message(err.to_string()))?;
    let mut doc = PipelineDoc::new(name, pipeline);
    // MISMA normalización que `PipelineLibrary::import_file`
    // (`normalize_for_import`: enlaces implícitos + cadena lineal): el
    // fallback de «Update from library» y el path de sobrescritura de
    // Import producen exactamente el documento que produciría import_file.
    doc.normalize_for_import();
    Ok(doc)
}

/// Resultado de un primer guardado con `CreateNew`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Guardado en `<stem>.pipeline`.
    Saved,
    /// El nombre ya existe en la librería: elección explícita del usuario.
    NameExists,
}

/// Primer guardado de un diseño nuevo (`CreateNew`): un nombre ocupado NO se
/// pisa jamás en silencio — el llamador ofrece Sobrescribir / Guardar como /
/// Cancelar y vuelve a llamar con [`PipelineLibrary::save`].
pub fn save_new(
    library: &PipelineLibrary,
    stem: &str,
    doc: &PipelineDoc,
) -> Result<SaveOutcome, StoreError> {
    match library.save(stem, doc, SaveMode::CreateNew) {
        Ok(()) => Ok(SaveOutcome::Saved),
        Err(StoreError::NameExists(_)) => Ok(SaveOutcome::NameExists),
        Err(err) => Err(err),
    }
}

// --- helpers espejo de opm-store (in-memory, sin persistencia) -------------

/// `dedup` con orden de primera aparición.
fn dedup(items: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item.clone());
        }
    }
    out
}

/// `storage.py:_normalize_step_id`.
fn normalize_step_id(step_id: &str) -> Result<String, StoreError> {
    let requested = step_id.trim();
    if requested.is_empty() {
        return Err(StoreError::StepIdRequired);
    }
    Ok(safe_segment(requested))
}

/// `storage.py:_normalized_step`.
fn normalized_step(step: &PipelineStep, step_id: &str, dependencies: &[String]) -> PipelineStep {
    let working_directory = if step.working_directory.is_empty() {
        format!("steps/{step_id}/work")
    } else {
        step.working_directory.clone()
    };
    let trimmed_name = step.name.trim();
    let name = if trimmed_name.is_empty() {
        step_id.to_string()
    } else {
        trimmed_name.to_string()
    };
    let mut normalized = step.clone();
    normalized.id = step_id.to_string();
    normalized.name = name;
    normalized.environment = step.environment.trim().to_string();
    normalized.working_directory = working_directory;
    normalized.dependencies = dedup(dependencies);
    normalized
}

/// Posición por defecto del paso `index` (mismas constantes que el core:
/// `x=80+i·330, y=120`).
fn default_step_position(index: usize) -> Position {
    Position {
        x: 80.0 + index as f64 * 330.0,
        y: 120.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opm_pipeline::{IssueSeverity, OutputSpec, Value, ValueSpec};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "opm-design-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn step(id: &str) -> PipelineStep {
        PipelineStep {
            id: id.to_owned(),
            name: id.to_owned(),
            description: String::new(),
            command: "echo {msg}".to_owned(),
            environment: String::new(),
            working_directory: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            parameters: vec![ValueSpec {
                key: "msg".to_owned(),
                label: None,
                kind: FieldType::Text,
                required: false,
                default: Some(Value::Str("hola".to_owned())),
                options: Vec::new(),
                min: None,
                max: None,
                description: String::new(),
                source_step: None,
                source_output: None,
            }],
            dependencies: Vec::new(),
        }
    }

    /// Estado vacío: sin cambios, sin nombre guardado.
    #[test]
    fn diseño_nuevo_limpio_y_sin_nombre() {
        let design = DesignState::new("mi-pipeline");
        assert!(!design.dirty());
        assert_eq!(design.saved_name(), None);
        assert_eq!(design.display_name(), "mi-pipeline");
        assert!(design.steps().is_empty());
    }

    /// Oculto-acoplamiento #8: la vista del diseño se construye SIN ninguna
    /// carpeta de trabajo ni store abierto — pipeline + layout + estados.
    #[test]
    fn snapshot_desde_documento_sin_carpeta_de_trabajo() {
        let mut design = DesignState::new("sin-raiz");
        design.create_step(step("s1")).unwrap();
        design.create_step(step("s2")).unwrap();
        design.toggle_selected("s1");

        let snapshot = design.snapshot(Vec::new());
        assert!(snapshot.project_path.is_empty(), "sin carpeta de trabajo");
        let pipeline = snapshot.pipeline.expect("pipeline presente");
        assert_eq!(
            pipeline.steps.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["s1", "s2"]
        );
        // s2 hereda la dependencia por defecto sobre el último paso.
        assert_eq!(pipeline.steps[1].dependencies, vec!["s1".to_owned()]);
        // Layout sembrado con la posición por defecto de cada paso
        // (x = 80 + i·330, mismas constantes que el core).
        assert_eq!(snapshot.visual_layout["s1"].x, 80.0);
        assert_eq!(snapshot.visual_layout["s2"].x, 410.0);
        // Selección de vista en el paso conmutado (estados perezosos: los
        // no conmutados ni siquiera tienen entrada).
        assert!(snapshot.state.get("s1").is_some_and(|state| state.selected));
        assert!(!snapshot.state.get("s2").is_some_and(|state| state.selected));

        let detail = design.step_detail("s1", &[]).unwrap();
        assert!(detail.params.is_empty(), "sin valores de ejecución");
        assert_eq!(detail.state.status, opm_store::StepStatus::Pending);
    }

    /// Crear/editar/dependencias/layout ensucian; la selección NO (no se
    /// persiste en el documento). `mark_saved` limpia.
    #[test]
    fn estado_sucio_solo_con_mutaciones_de_documento() {
        let mut design = DesignState::new("dirty");
        assert!(!design.dirty());

        design.toggle_selected("s1");
        assert!(!design.dirty(), "la selección de run no ensucia");

        design.create_step(step("s1")).unwrap();
        assert!(design.dirty(), "crear paso ensucia");

        design.mark_saved("dirty");
        assert!(!design.dirty());
        assert_eq!(design.saved_name(), Some("dirty"));

        design.set_layout("s1", 12.0, 34.0);
        assert!(design.dirty(), "mover un nodo ensucia el layout");

        design.mark_saved("dirty");
        design.set_dependencies("s1", vec![]).unwrap();
        assert!(design.dirty(), "cambiar dependencias ensucia");
    }

    /// Reglas espejo del store: id obligatorio, sin duplicados, rename con
    /// propagación a dependencias y migración de layout/estado.
    #[test]
    fn mutaciones_con_las_mismas_reglas_del_store() {
        let mut design = DesignState::new("reglas");
        let err = design.create_step(step("  ")).unwrap_err();
        assert!(matches!(err, StoreError::StepIdRequired), "{err:?}");

        design.create_step(step("s1")).unwrap();
        let err = design.create_step(step("s1")).unwrap_err();
        assert!(matches!(err, StoreError::StepExists(_)), "{err:?}");

        design.create_step(step("s2")).unwrap();
        design.toggle_selected("s2");
        let position = *design.doc().layout.get("s2").unwrap();

        // Rename: las referencias a s2 se propagan y layout/estado migran.
        let mut renamed = step("s2-renamed");
        renamed.dependencies = vec!["s1".to_owned()];
        design.update_step("s2", renamed).unwrap();
        assert!(design.step("s2").is_none());
        assert!(design.step("s2-renamed").is_some());
        assert_eq!(
            design.doc().layout.get("s2-renamed"),
            Some(&position),
            "el layout migra con el rename"
        );
        assert!(
            design.state_of("s2-renamed").selected,
            "el estado migra con el rename"
        );

        // Referencia `source_step` de OTRO paso renombrada.
        let mut consumer = step("s3");
        consumer.inputs.push(ValueSpec {
            key: "src".to_owned(),
            label: None,
            kind: FieldType::File,
            required: false,
            default: Some(Value::Str("out.txt".to_owned())),
            options: Vec::new(),
            min: None,
            max: None,
            description: String::new(),
            source_step: Some("s2-renamed".to_owned()),
            source_output: Some("out.txt".to_owned()),
        });
        design.create_step(consumer).unwrap();

        // Dependencias: dedup, auto-dependencia y referencias desconocidas.
        design
            .set_dependencies("s3", vec!["s1".to_owned(), "s1".to_owned()])
            .unwrap();
        assert_eq!(design.step("s3").unwrap().dependencies, vec!["s1".to_owned()]);
        let err = design.set_dependencies("s3", vec!["s3".to_owned()]).unwrap_err();
        assert!(matches!(err, StoreError::SelfDependency), "{err:?}");
        let err = design
            .set_dependencies("s3", vec!["fantasma".to_owned()])
            .unwrap_err();
        assert!(matches!(err, StoreError::UnknownDependency(_)), "{err:?}");
        let err = design
            .update_step("fantasma", step("x"))
            .unwrap_err();
        assert!(matches!(err, StoreError::UnknownStep(_)), "{err:?}");
    }

    /// Validación de diseño: conserva los issues estructurales y descarta los
    /// que solo una carpeta de trabajo puede satisfacer (existencias).
    #[test]
    fn validacion_de_diseño_descarta_solo_existencias() {
        let root = temp_dir("valid");
        let mut with_wd = step("s1");
        with_wd.working_directory = "steps/s1/work".to_owned();
        with_wd.inputs.push(ValueSpec {
            key: "data".to_owned(),
            label: Some("Data file".to_owned()),
            kind: FieldType::File,
            required: true,
            default: Some(Value::Str("Input/x.fa".to_owned())),
            options: Vec::new(),
            min: None,
            max: None,
            description: String::new(),
            source_step: None,
            source_output: None,
        });
        with_wd.dependencies = vec!["fantasma".to_owned()];
        let pipeline = PipelineConfig {
            steps: vec![with_wd.clone()],
        };

        let raw = validate_pipeline(&pipeline, &root, &HashMap::new());
        assert!(
            raw.iter().any(|issue| issue
                .message
                .starts_with("Working directory does not exist")),
            "sin filtrar aparece la existencia del wd: {raw:?}"
        );

        let issues = design_validation(&pipeline, &root);
        let messages: Vec<&str> = issues
            .iter()
            .map(|issue| issue.message.as_str())
            .collect();
        assert!(
            !messages
                .iter()
                .any(|message| message.starts_with("Working directory does not exist")),
            "el wd inexistente no es issue de diseño: {messages:?}"
        );
        assert!(
            !messages.iter().any(|message| message
                .starts_with("Data file does not exist: Input/x.fa")),
            "la existencia del fichero declarado no es issue de diseño: {messages:?}"
        );
        // Estructurales que SÍ son de diseño:
        assert!(
            issues.iter().any(|issue| issue.message
                == "Dependency does not exist: fantasma"
                && issue.severity == IssueSeverity::Blocker),
            "dependencia desconocida = issue de diseño: {messages:?}"
        );

        // Sin dependencia rota: la validación de diseño queda limpia.
        let mut ok = with_wd;
        ok.dependencies.clear();
        let pipeline = PipelineConfig { steps: vec![ok] };
        assert!(
            design_validation(&pipeline, &root).is_empty(),
            "declaración completa y sin issues estructurales"
        );

        // Un placeholder sin valor declarado sigue siendo feedback de diseño.
        let mut missing = step("s2");
        missing.command = "echo {otro}".to_owned();
        missing.parameters.clear();
        let pipeline = PipelineConfig { steps: vec![missing] };
        assert!(
            design_validation(&pipeline, &root)
                .iter()
                .any(|issue| issue.message.starts_with("Placeholder has no input")),
            "placeholder sin declarar se ve en diseño"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Guardar en la librería con elección explícita ante nombres ocupados
    /// (CreateNew → NameExists → Overwrite; nunca sobrescritura silenciosa).
    #[test]
    fn guardado_en_libreria_con_conflicto_explicito() {
        let root = temp_dir("library");
        let library = PipelineLibrary::new(&root);
        let mut design = DesignState::new("flujo");
        design.create_step(step("s1")).unwrap();

        let stem = library_stem("flujo").unwrap();
        assert_eq!(
            save_new(&library, &stem, &design.document_for_save(&stem)).unwrap(),
            SaveOutcome::Saved
        );
        design.mark_saved(&stem);
        assert!(!design.dirty());

        // Segundo diseño con el MISMO nombre: conflicto explícito.
        let mut rival = DesignState::new("flujo");
        rival.create_step(step("otro")).unwrap();
        let rival_doc = rival.document_for_save(&stem);
        assert_eq!(
            save_new(&library, &stem, &rival_doc).unwrap(),
            SaveOutcome::NameExists
        );
        assert!(rival.dirty(), "el conflicto no limpió el diseño");
        // El documento original sigue intacto en disco.
        let reloaded = library.load(&stem).unwrap();
        assert_eq!(reloaded.pipeline.steps[0].id, "s1");

        // Elección «Overwrite»: se pisa con consentimiento explícito.
        library.save(&stem, &rival_doc, SaveMode::Overwrite).unwrap();
        rival.mark_saved(&stem);
        let reloaded = library.load(&stem).unwrap();
        assert_eq!(reloaded.pipeline.steps[0].id, "otro");

        // Elección «Save As»: otro nombre, CreateNew de nuevo.
        let other = library_stem("flujo 2").unwrap();
        assert_eq!(
            save_new(&library, &other, &rival.document_for_save(&other)).unwrap(),
            SaveOutcome::Saved
        );
        assert_eq!(library.list(), vec!["flujo".to_owned(), "flujo_2".to_owned()]);

        // Nombres: vacío rechazado, `.pipeline` redundante quitado.
        assert!(library_stem("   ").is_err());
        assert_eq!(library_stem(" mío .pipeline ").unwrap(), "mío");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// El stem saneado coincide con el que usa el núcleo: el archivo creado
    /// es exactamente `<stem>.pipeline`.
    #[test]
    fn stem_saneado_coincide_con_el_archivo() {
        let root = temp_dir("stem");
        let library = PipelineLibrary::new(&root);
        let design = DesignState::new("My Pipeline");
        let stem = library_stem("My Pipeline").unwrap();
        assert_eq!(stem, "My_Pipeline");
        save_new(&library, &stem, design.doc()).unwrap();
        assert!(root.join("My_Pipeline.pipeline").is_file());
        assert_eq!(library.list(), vec!["My_Pipeline".to_owned()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `OutputSpec` queda referenciado para futuros ajustes del diálogo.
    #[test]
    fn modelo_sostiene_outputs_declarados() {
        let mut step = step("s1");
        step.outputs.push(OutputSpec {
            key: "out".to_owned(),
            label: None,
            path: "outputs/out.txt".to_owned(),
        });
        let mut design = DesignState::new("outs");
        design.create_step(step).unwrap();
        assert_eq!(design.step("s1").unwrap().outputs.len(), 1);
    }

    /// Carga de definiciones: `.pipeline` nativo con versión comprobada y
    /// YAML de intercambio envuelto en `PipelineDoc` (fallback de adopt).
    #[test]
    fn definition_from_file_nativa_o_yaml() {
        let root = temp_dir("load");

        // Nativa `.pipeline` (JSON versionado).
        let native = root.join("demo.pipeline");
        let doc = {
            let mut design = DesignState::new("demo");
            design.create_step(step("s1")).unwrap();
            design.document_for_save("demo")
        };
        std::fs::write(&native, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        let loaded = definition_from_file(&native, "ignorado").unwrap();
        assert_eq!(loaded.format_version, opm_store::FORMAT_VERSION);
        assert_eq!(loaded.pipeline.steps[0].id, "s1");

        // Versión no soportada → error visible que nombra el fichero.
        std::fs::write(
            &native,
            r#"{"format_version": 999, "name": "demo", "pipeline": {"steps": []}}"#,
        )
        .unwrap();
        let err = definition_from_file(&native, "demo").unwrap_err();
        assert!(err.to_string().contains("unsupported"), "{err}");

        // YAML de intercambio → envoltorio con el nombre dado y LA misma
        // normalización que `import_file` (cadena lineal de pasos sin
        // enlaces: paridad del loader con el import de la librería).
        let yaml = root.join("ext.yml");
        std::fs::write(
            &yaml,
            "steps:\n  - id: s1\n    name: uno\n    command: echo hi\n  \
             - id: s2\n    name: dos\n    command: echo hey\n",
        )
        .unwrap();
        let imported = definition_from_file(&yaml, "externo").unwrap();
        assert_eq!(imported.name, "externo");
        assert_eq!(imported.pipeline.steps[0].id, "s1");
        assert_eq!(
            imported.pipeline.steps[1].dependencies,
            vec!["s1".to_owned()],
            "paridad con import_file: pasos sin enlaces encadenados"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
