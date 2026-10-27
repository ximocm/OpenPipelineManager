//! Diálogo crear/editar paso (§6, port de `CreateStepDialog.tsx`).
//!
//! Dos capas:
//!
//! - **Modelo puro** ([`StepDraft`], [`ValueDraft`], [`OutputDraft`] y las
//!   funciones `to_value_spec`/`parse_default`/`sanitize_identifier`/…):
//!   id auto `step_<n+1>` saneado con `opm_fs::safe_segment`, guardia de
//!   default booleano inválido que BLOQUEA el envío, chips `{keyword}` con
//!   inserción por caret, «Pipeline var» (`UPPERCASE_VAR={key}`), «From
//!   output» (default = ruta relativa desde el wd del paso consumidor al
//!   output del origen + `source_step`/`source_output` + dependencia
//!   automática) y el armado final del [`PipelineStep`] (edit → prefilled).
//! - **Capa GTK** ([`open_step_dialog`]): ventana modal con formulario de
//!   filas dinámicas (Inputs/Options/Outputs), chips vivos y pie con la
//!   guardia o el working directory resuelto. Al guardar, el integrador
//!   llama a `store.create_step` / `store.update_step` (la propagación de
//!   rename la trae el core).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;

use opm_pipeline::{FieldType, OutputSpec, PipelineStep, Value, ValueSpec};

use super::step_panel::value_to_text;

// ======================= Modelo puro =======================================

/// Las 7 tipologías del SPA, en su orden de UI.
pub const FIELD_TYPES: &[FieldType] = &[
    FieldType::Text,
    FieldType::Integer,
    FieldType::Decimal,
    FieldType::Boolean,
    FieldType::Select,
    FieldType::File,
    FieldType::Folder,
];

const FIELD_TYPE_LABELS: &[&str] = &[
    "Text",
    "Integer",
    "Decimal",
    "Boolean",
    "Selector (predefined options)",
    "File",
    "Folder",
];

/// Etiqueta de menú del tipo (paridad `fieldTypeLabels`).
pub fn field_type_label(kind: FieldType) -> &'static str {
    let index = FIELD_TYPES
        .iter()
        .position(|candidate| *candidate == kind)
        .unwrap_or(0);
    FIELD_TYPE_LABELS[index]
}

fn type_index(kind: FieldType) -> usize {
    FIELD_TYPES
        .iter()
        .position(|candidate| *candidate == kind)
        .unwrap_or(0)
}

/// Etiquetas del desplegable de tipo (paridad SPA
/// `fieldTypes.map(type => fieldTypeLabels[type])`).
fn field_type_labels() -> Vec<&'static str> {
    FIELD_TYPES.iter().map(|kind| field_type_label(*kind)).collect()
}

fn type_at(index: u32) -> FieldType {
    FIELD_TYPES
        .get(index as usize)
        .copied()
        .unwrap_or(FieldType::Text)
}

/// Draft editable de un input o parámetro (fila del diálogo).
#[derive(Clone, Debug, PartialEq)]
pub struct ValueDraft {
    pub key: String,
    pub label: String,
    pub kind: FieldType,
    pub required: bool,
    pub default_text: String,
    /// Opciones del select en CSV (`"fast, accurate"`).
    pub choices: String,
    pub min: String,
    pub max: String,
    pub source_step: Option<String>,
    pub source_output: Option<String>,
    /// Default booleano guardado no booleano (guardia del SPA: bloquea el
    /// envío hasta elegir un valor válido).
    pub invalid_boolean: Option<String>,
}

/// Draft editable de un output.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputDraft {
    pub key: String,
    pub label: String,
    pub path: String,
}

/// Draft completo del paso (crear o editar).
#[derive(Clone, Debug, PartialEq)]
pub struct StepDraft {
    pub id: String,
    pub name: String,
    pub description: String,
    pub environment: String,
    pub working_directory: String,
    pub command: String,
    pub inputs: Vec<ValueDraft>,
    pub parameters: Vec<ValueDraft>,
    pub outputs: Vec<OutputDraft>,
    pub dependencies: Vec<String>,
    /// `Some(id)` = modo edición (el integrador usará `update_step`).
    pub original_id: Option<String>,
    /// Id por defecto (`step_<n+1>` en crear; el propio id en editar).
    pub suggested_id: String,
    /// Nombre por defecto (`Step <n+1>` en crear).
    pub suggested_name: String,
}

impl StepDraft {
    /// Formulario de creación con id `step_<n+1>` y nombre `Step <n+1>`
    /// (paridad SPA: `suggestedIndex = steps.length`).
    pub fn for_create(steps: &[PipelineStep]) -> Self {
        let number = steps.len() + 1;
        let suggested_id = format!("step_{number}");
        Self {
            id: suggested_id.clone(),
            name: format!("Step {number}"),
            description: String::new(),
            environment: String::new(),
            working_directory: String::new(),
            command: String::new(),
            inputs: Vec::new(),
            parameters: Vec::new(),
            outputs: Vec::new(),
            dependencies: Vec::new(),
            original_id: None,
            suggested_id,
            suggested_name: format!("Step {number}"),
        }
    }

    /// Formulario de edición prefilled (el id original es el fallback del
    /// saneado, como en el SPA).
    pub fn for_edit(step: &PipelineStep) -> Self {
        Self {
            id: step.id.clone(),
            name: step.name.clone(),
            description: step.description.clone(),
            environment: step.environment.clone(),
            working_directory: step.working_directory.clone(),
            command: step.command.clone(),
            inputs: step.inputs.iter().map(value_draft_from_spec).collect(),
            parameters: step.parameters.iter().map(value_draft_from_spec).collect(),
            outputs: step.outputs.iter().map(output_draft_from_spec).collect(),
            dependencies: step.dependencies.clone(),
            original_id: Some(step.id.clone()),
            suggested_id: step.id.clone(),
            suggested_name: step.name.clone(),
        }
    }

    /// Id saneado (`safe_segment`, §6) con fallback al sugerido cuando no
    /// hay nada utilizable tecleado.
    pub fn clean_id(&self) -> String {
        clean_step_id(&self.id, &self.suggested_id)
    }

    /// Working directory resuelto (vacío → `steps/<id>/work`, §6).
    pub fn resolved_wd(&self) -> String {
        let wd = self.working_directory.trim();
        if !wd.is_empty() {
            return wd.to_owned();
        }
        format!("steps/{}/work", self.clean_id())
    }

    /// ¿Alguna fila booleana tiene un default guardado no booleano?
    pub fn has_invalid_boolean(&self) -> bool {
        self.inputs
            .iter()
            .chain(self.parameters.iter())
            .any(|draft| draft.kind == FieldType::Boolean && draft.invalid_boolean.is_some())
    }

    /// Chips `{keyword}`: claves saneadas de inputs/options/outputs, sin
    /// duplicados y sin vacíos (paridad SPA).
    pub fn command_keywords(&self) -> Vec<String> {
        let mut keywords: Vec<String> = Vec::new();
        let mut push = |raw: &str| {
            let clean = sanitize_identifier(raw);
            if !clean.is_empty() && !keywords.iter().any(|seen| seen == &clean) {
                keywords.push(clean);
            }
        };
        for draft in self.inputs.iter().chain(self.parameters.iter()) {
            push(&draft.key);
        }
        for output in &self.outputs {
            push(&output.key);
        }
        keywords
    }

    /// Armado final del paso. `None` si la guardia booleana bloquea (§6).
    pub fn build(&self) -> Option<PipelineStep> {
        if self.has_invalid_boolean() {
            return None;
        }
        let id = self.clean_id();
        let name = {
            let name = self.name.trim();
            if name.is_empty() {
                id.clone()
            } else {
                name.to_owned()
            }
        };
        Some(PipelineStep {
            id,
            name,
            description: self.description.trim().to_owned(),
            command: self.command.trim().to_owned(),
            environment: self.environment.trim().to_owned(),
            working_directory: self.resolved_wd(),
            inputs: self.inputs.iter().filter_map(to_value_spec).collect(),
            outputs: self.outputs.iter().filter_map(to_output_spec).collect(),
            parameters: self.parameters.iter().filter_map(to_value_spec).collect(),
            dependencies: self.dependencies.clone(),
        })
    }
}

/// Unión de una dependencia (paridad SPA `new Set([...deps, source])`):
/// devuelve `true` si la añadió. Núcleo compartido por el diálogo «From
/// output» (§6), sus pruebas de modelo y el cierre del picker.
pub fn ensure_dependency(dependencies: &mut Vec<String>, step_id: &str) -> bool {
    if dependencies.iter().any(|dependency| dependency == step_id) {
        false
    } else {
        dependencies.push(step_id.to_owned());
        true
    }
}

/// Saneado de identificadores del SPA (`sanitizeIdentifier`): espacios →
/// `_`, fuera de `[\w.-]` → `_`, colapsa `_`, recorta `._` por los bordes;
/// vacío si no queda nada (el llamador decide el fallback).
pub fn sanitize_identifier(value: &str) -> String {
    let spaced = value.trim().split_whitespace().collect::<Vec<_>>().join("_");
    let replaced: String = spaced
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
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
    collapsed
        .trim_matches(|c| c == '.' || c == '_')
        .to_owned()
}

/// Id del paso: `opm_fs::safe_segment` (§6) con fallback `default` cuando
/// no hay ni un solo carácter utilizable (paridad SPA `|| defaultId`).
pub fn clean_step_id(raw: &str, default: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || !trimmed
            .trimmed_chars_has_utilizable()
    {
        return default.to_owned();
    }
    opm_fs::safe_segment(trimmed)
}

trait TrimmableExt {
    fn trimmed_chars_has_utilizable(&self) -> bool;
}

impl TrimmableExt for &str {
    fn trimmed_chars_has_utilizable(&self) -> bool {
        self.chars().any(|c| c.is_alphanumeric() || c == '-')
    }
}

/// `toEnvironmentVariable` del SPA: `[A-Za-z0-9_]`, colapsa `_`, mayúsculas,
/// `VAR_` si empieza por dígito.
pub fn to_environment_variable(key: &str) -> String {
    let mapped: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let mut collapsed = mapped.replace("__", "_");
    while collapsed.contains("__") {
        collapsed = collapsed.replace("__", "_");
    }
    if collapsed.starts_with(|c: char| c.is_ascii_digit()) {
        format!("VAR_{collapsed}")
    } else {
        collapsed
    }
}

/// `parseDefaultValue` del SPA: vacío → sin default; booleano solo
/// `true`/`false`; `integer` vía parseInt (prefijo entero); `decimal` vía
/// Number finito; file/folder conserva el texto SIN recortar.
pub fn parse_default(kind: FieldType, text: &str) -> Option<Value> {
    let clean = text.trim();
    if clean.is_empty() {
        return None;
    }
    match kind {
        FieldType::Boolean => match clean {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        FieldType::Integer => parse_int_prefix(clean).map(Value::Int),
        FieldType::Decimal => clean
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(Value::Float),
        FieldType::File | FieldType::Folder => Some(Value::Str(text.to_owned())),
        FieldType::Text | FieldType::Select => Some(Value::Str(clean.to_owned())),
    }
}

/// `parseInt` de JS: prefijo con signo y dígitos (`"3.5"` → `3`,
/// `"3abc"` → `3`, `"abc"` → `None`).
fn parse_int_prefix(text: &str) -> Option<i64> {
    let mut digits = String::new();
    let mut chars = text.chars();
    match chars.next() {
        Some(sign @ ('-' | '+')) => digits.push(sign),
        Some(first) if first.is_ascii_digit() => digits.push(first),
        Some(_) => return None,
        None => return None,
    }
    for c in chars {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            break;
        }
    }
    if digits.len() <= 1 && !digits.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok()
}

/// Fila draft → [`ValueSpec`]: `None` si la clave saneada queda vacía (la
/// fila se excluye, paridad SPA `filter(Boolean)`).
pub fn to_value_spec(draft: &ValueDraft) -> Option<ValueSpec> {
    let key = sanitize_identifier(&draft.key);
    if key.is_empty() {
        return None;
    }
    let label = {
        let label = draft.label.trim();
        if label.is_empty() {
            key.clone()
        } else {
            label.to_owned()
        }
    };
    let default = parse_default(draft.kind, &draft.default_text);
    let options = if draft.kind == FieldType::Select {
        draft
            .choices
            .split(',')
            .map(|choice| choice.trim().to_owned())
            .filter(|choice| !choice.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    let numeric = matches!(draft.kind, FieldType::Integer | FieldType::Decimal);
    let min = numeric
        .then(|| draft.min.trim())
        .filter(|text| !text.is_empty())
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|number| number.is_finite());
    let max = numeric
        .then(|| draft.max.trim())
        .filter(|text| !text.is_empty())
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|number| number.is_finite());
    let linked = match (&draft.source_step, &draft.source_output) {
        (Some(step), Some(output)) if !step.is_empty() && !output.is_empty() => {
            (Some(step.clone()), Some(output.clone()))
        }
        _ => (None, None),
    };
    Some(ValueSpec {
        key,
        label: Some(label),
        kind: draft.kind,
        required: draft.required,
        default,
        options,
        min,
        max,
        description: String::new(),
        source_step: linked.0,
        source_output: linked.1,
    })
}

/// Fila draft → [`OutputSpec`]: `None` si la ruta está vacía (paridad SPA).
pub fn to_output_spec(draft: &OutputDraft) -> Option<OutputSpec> {
    if draft.path.trim().is_empty() {
        return None;
    }
    Some(OutputSpec {
        key: sanitize_identifier(&draft.key),
        label: {
            let label = draft.label.trim();
            if label.is_empty() {
                None
            } else {
                Some(label.to_owned())
            }
        },
        path: draft.path.clone(),
    })
}

/// Spec → draft (prefilled de edición). Detecta el default booleano no
/// booleano de la guardia (paridad `toValueDraft`).
pub fn value_draft_from_spec(spec: &ValueSpec) -> ValueDraft {
    let rendered = spec.default.as_ref().map(value_to_text);
    let invalid_boolean = match (&spec.kind, &spec.default) {
        (FieldType::Boolean, Some(value)) => match value {
            Value::Bool(_) | Value::Null => None,
            other => Some(value_to_text(other)),
        },
        _ => None,
    };
    let default_text = match invalid_boolean {
        Some(_) => String::new(),
        None => rendered.unwrap_or_default(),
    };
    ValueDraft {
        key: spec.key.clone(),
        label: spec.label.clone().unwrap_or_default(),
        kind: spec.kind,
        required: spec.required,
        default_text,
        choices: spec.options.join(", "),
        min: spec.min.map(format_number).unwrap_or_default(),
        max: spec.max.map(format_number).unwrap_or_default(),
        source_step: spec.source_step.clone(),
        source_output: spec.source_output.clone(),
        invalid_boolean,
    }
}

fn format_number(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        number.to_string()
    }
}

/// OutputSpec → draft (prefilled de edición).
pub fn output_draft_from_spec(spec: &OutputSpec) -> OutputDraft {
    OutputDraft {
        key: spec.key.clone(),
        label: spec.label.clone().unwrap_or_default(),
        path: spec.path.clone(),
    }
}

/// Índice del dropdown booleano CON la opción inválida en la posición 0 →
/// índice en el modelo normal (0 «No default», 1 «True», 2 «False»).
///
/// P1-2: la traducción y el cambio de modelo son UNA sola operación; con el
/// modelo de 4 items intacto, la posición mostrada «True» (2) se leería en
/// el modelo normal como «False» (y se guardaría `false`). `0` («Invalid
/// stored value») y `u32::MAX` (sin selección) no traducen: la guardia sigue
/// armada hasta que el usuario elija un valor válido.
pub fn translate_invalid_boolean_index(selected: u32) -> Option<u32> {
    match selected {
        1..=3 => Some(selected - 1),
        _ => None,
    }
}

/// Texto del default booleano leído en el dropdown (`selected`) según el
/// modelo vigente: `has_invalid` = el modelo lleva la opción inválida en la
/// posición 0 (P1-2: leer y escribir deben correr sobre el MISMO modelo).
pub fn boolean_default_text_at(selected: u32, has_invalid: bool) -> String {
    let base = if has_invalid {
        selected.checked_sub(1)
    } else if selected == u32::MAX {
        None
    } else {
        Some(selected)
    };
    match base {
        Some(1) => "true".to_owned(),
        Some(2) => "false".to_owned(),
        _ => String::new(),
    }
}

/// Inserción de chip/variable en el comando por caret (offsets en CHARS,
/// como los de `gtk::TextIter::offset`). Devuelve el texto nuevo y el caret.
pub fn insert_at(current: &str, start: usize, end: usize, inserted: &str) -> (String, usize) {
    let chars: Vec<char> = current.chars().collect();
    let start = start.min(chars.len());
    let end = end.min(chars.len()).max(start);
    let mut out: String = chars[..start].iter().collect();
    out.push_str(inserted);
    let caret = start + inserted.chars().count();
    out.extend(chars[end..].iter());
    (out, caret)
}

/// Una opción del picker «From output».
#[derive(Clone, Debug, PartialEq)]
pub struct OutputChoice {
    pub step_id: String,
    pub output_key: String,
    pub output_path: String,
    /// `"<paso> -> <path>"` (paridad SPA).
    pub label: String,
    /// Ruta relativa desde el wd consumidor hasta el output, calculada con
    /// los helpers de `opm-fs` (P2-15: la misma aritmética que persiste el
    /// store al guardar el enlace).
    pub default_value: String,
}

impl OutputChoice {
    /// `source_output`: la clave del output, o su path si la clave está
    /// vacía (paridad SPA `outputKey || outputPath`).
    pub fn source_output(&self) -> String {
        if self.output_key.is_empty() {
            self.output_path.clone()
        } else {
            self.output_key.clone()
        }
    }
}

/// Opciones «From output»: outputs de los pasos ANTERIORES al actual (todos
/// en modo crear), con la ruta relativa desde `target_wd` hasta cada output
/// calculada por LOS MISMOS helpers con los que el store persiste el enlace
/// (`resolve_pipeline_path` + `relative_project_path`, P2-15: la vista
/// previa y lo guardado son la misma aritmética, separadores incluidos).
/// `root` es la raíz del proyecto; sin ella no hay opciones (el diálogo solo
/// abre con proyecto abierto).
pub fn output_choices(
    root: Option<&Path>,
    steps: &[PipelineStep],
    current_id: Option<&str>,
    target_wd: &str,
) -> Vec<OutputChoice> {
    let Some(root) = root else {
        return Vec::new();
    };
    let current_index = current_id
        .and_then(|id| steps.iter().position(|step| step.id == id))
        .unwrap_or(steps.len());
    let previous = &steps[..current_index];
    let mut choices = Vec::new();
    for step in previous {
        for output in &step.outputs {
            let default_value = match source_relative_path(
                root,
                target_wd,
                &step.working_directory,
                &output.path,
            ) {
                Ok(value) => value,
                // Fuente insegura (absoluto, `..` fuera, whitespace-only…):
                // la validación la bloquea como «Unsafe input source path»,
                // así que el picker no la ofrece (P2-15).
                Err(_) => continue,
            };
            choices.push(OutputChoice {
                step_id: step.id.clone(),
                output_key: output.key.clone(),
                output_path: output.path.clone(),
                label: format!("{} -> {}", step.id, output.path),
                default_value,
            });
        }
    }
    choices
}

/// Cadena canónica wd consumidor → wd fuente → path del output → relativa:
/// LA misma composición que usan `opm-pipeline::validate` y `opm-store` para
/// persistir los enlaces de input (P2-15: una sola aritmética de rutas).
fn source_relative_path(
    root: &Path,
    target_working_directory: &str,
    source_working_directory: &str,
    source_output_path: &str,
) -> Result<String, opm_fs::PathError> {
    let target = opm_fs::resolve_pipeline_path(root, target_working_directory, root)?;
    let source_base = opm_fs::resolve_pipeline_path(root, source_working_directory, root)?;
    let source = opm_fs::resolve_pipeline_path(root, source_output_path, &source_base)?;
    opm_fs::relative_project_path(root, &target, &source)
}

// ======================= Capa GTK ==========================================

/// Sección de filas de valor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Inputs,
    Parameters,
}

/// Una fila de input/opción con sus widgets.
struct ValueRow {
    container: gtk::Box,
    key: gtk::Entry,
    label: gtk::Entry,
    kind: gtk::DropDown,
    /// Tipo mostrado (para detectar transiciones con booleano).
    last_kind: Cell<usize>,
    default_entry: gtk::Entry,
    default_dd: gtk::DropDown,
    choices: gtk::Entry,
    min: gtk::Entry,
    max: gtk::Entry,
    required: gtk::CheckButton,
    var_btn: gtk::Button,
    remove_btn: gtk::Button,
    from_output: Option<gtk::DropDown>,
    /// `(source_step, source_output)` del picker «From output».
    source: RefCell<Option<(String, String)>>,
    invalid: RefCell<Option<String>>,
}

const BOOLEAN_TYPE_INDEX: usize = 3; // orden de FIELD_TYPES

impl ValueRow {
    /// Construye los widgets de la fila (sin wiring: lo hace el diálogo).
    fn new(with_from_output: bool) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
        container.add_css_class("opm-step-option-row");

        let main = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let key = gtk::Entry::new();
        key.set_width_chars(10);
        key.set_placeholder_text(Some("key"));
        let label = gtk::Entry::new();
        label.set_width_chars(10);
        label.set_placeholder_text(Some("label"));
        let kind_labels = field_type_labels();
        let kind = gtk::DropDown::from_strings(&kind_labels);
        let default_entry = gtk::Entry::new();
        default_entry.set_width_chars(9);
        let default_dd = gtk::DropDown::from_strings(&["No default", "True", "False"]);
        let var_btn = gtk::Button::with_label("Pipeline var");
        var_btn.set_tooltip_text(Some("Insert VARIABLE={key} in command"));
        let required = gtk::CheckButton::new();
        required.set_tooltip_text(Some("Required"));
        let remove_btn = gtk::Button::with_label("✕");
        remove_btn.set_tooltip_text(Some("Remove"));
        remove_btn.add_css_class("opm-danger-button");

        main.append(&cell("Key", &key));
        main.append(&cell("Label", &label));
        main.append(&cell("Type", &kind));
        main.append(&cell("Default", &default_entry));
        main.append(&cell("Default", &default_dd));
        main.append(&var_btn);
        main.append(&cell("Required", &required));
        main.append(&remove_btn);
        container.append(&main);

        let extras = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let choices = gtk::Entry::new();
        choices.set_placeholder_text(Some("fast, accurate, custom"));
        choices.set_hexpand(true);
        let min = gtk::Entry::new();
        min.set_width_chars(7);
        min.set_tooltip_text(Some("Min"));
        let max = gtk::Entry::new();
        max.set_width_chars(7);
        max.set_tooltip_text(Some("Max"));
        let from_output = with_from_output.then(|| {
            let dropdown = gtk::DropDown::from_strings(&["Select previous output"]);
            dropdown
        });

        let choices_cell = cell("Predefined options", &choices);
        extras.append(&choices_cell);
        extras.append(&cell("Min", &min));
        extras.append(&cell("Max", &max));
        if let Some(dropdown) = &from_output {
            extras.append(&cell("From output", dropdown));
        }
        container.append(&extras);

        let row = Rc::new(Self {
            container,
            key,
            label,
            kind,
            last_kind: Cell::new(0),
            default_entry,
            default_dd,
            choices,
            min,
            max,
            required,
            var_btn,
            remove_btn,
            from_output,
            source: RefCell::new(None),
            invalid: RefCell::new(None),
        });
        row.sync_visibility();
        row
    }

    /// Carga un draft en los widgets (prefilled de edición o de filas
    /// preservadas).
    fn load(self: &Rc<Self>, draft: &ValueDraft) {
        self.key.set_text(&draft.key);
        self.label.set_text(&draft.label);
        let index = type_index(draft.kind);
        self.kind.set_selected(index as u32);
        self.last_kind.set(index);
        self.required.set_active(draft.required);
        self.default_entry.set_text(&draft.default_text);
        self.choices.set_text(&draft.choices);
        self.min.set_text(&draft.min);
        self.max.set_text(&draft.max);
        *self.source.borrow_mut() = match (&draft.source_step, &draft.source_output) {
            (Some(step), Some(output)) => Some((step.clone(), output.clone())),
            _ => None,
        };
        if let Some(invalid) = &draft.invalid_boolean {
            *self.invalid.borrow_mut() = Some(invalid.clone());
            let items = [
                format!("Invalid stored value: {invalid}"),
                "No default".to_owned(),
                "True".to_owned(),
                "False".to_owned(),
            ];
            let refs: Vec<&str> = items.iter().map(String::as_str).collect();
            self.default_dd.set_model(Some(&gtk::StringList::new(&refs)));
            self.default_dd.set_selected(0);
        } else {
            let selected = match draft.default_text.as_str() {
                "true" => 1,
                "false" => 2,
                _ => 0,
            };
            self.default_dd.set_selected(selected);
        }
        self.sync_visibility();
    }

    /// Visibilidad por tipo (paridad SPA: opciones solo en select, min/max
    /// solo en números, «From output» solo en file/folder, default booleano
    /// como dropdown).
    fn sync_visibility(&self) {
        let index = self.kind.selected() as usize;
        self.default_entry.set_visible(index != BOOLEAN_TYPE_INDEX);
        self.default_dd.set_visible(index == BOOLEAN_TYPE_INDEX);
        self.choices.set_visible(index == 4);
        if let Some(parent) = self.choices.parent() {
            parent.set_visible(index == 4);
        }
        let numeric = index == 1 || index == 2;
        self.min.set_visible(numeric);
        self.max.set_visible(numeric);
        if let Some(parent) = self.min.parent() {
            parent.set_visible(numeric);
        }
        if let Some(parent) = self.max.parent() {
            parent.set_visible(numeric);
        }
        if let Some(dropdown) = &self.from_output {
            let fileish = index == 5 || index == 6;
            dropdown.set_visible(fileish);
            if let Some(parent) = dropdown.parent() {
                parent.set_visible(fileish);
            }
        }
        self.var_btn.set_sensitive(!sanitize_identifier(&self.key.text()).is_empty());
    }

    fn kind(&self) -> FieldType {
        type_at(self.kind.selected())
    }

    fn has_invalid_boolean(&self) -> bool {
        self.kind() == FieldType::Boolean && self.invalid.borrow().is_some()
    }

    /// Texto del default booleano desde el dropdown (con la opción inválida
    /// en la posición 0 cuando existe) — traducción pura en
    /// [`boolean_default_text_at`] (P1-2: testeable sin GTK).
    fn boolean_default_text(&self) -> String {
        boolean_default_text_at(self.default_dd.selected(), self.invalid.borrow().is_some())
    }

    fn to_draft(&self) -> ValueDraft {
        ValueDraft {
            key: self.key.text().to_string(),
            label: self.label.text().to_string(),
            kind: self.kind(),
            required: self.required.is_active(),
            default_text: if self.kind() == FieldType::Boolean {
                self.boolean_default_text()
            } else {
                self.default_entry.text().to_string()
            },
            choices: self.choices.text().to_string(),
            min: self.min.text().to_string(),
            max: self.max.text().to_string(),
            source_step: self.source.borrow().as_ref().map(|(step, _)| step.clone()),
            source_output: self
                .source
                .borrow()
                .as_ref()
                .map(|(_, output)| output.clone()),
            invalid_boolean: self.invalid.borrow().clone(),
        }
    }

    /// Vacía el modelo booleano (sin la opción inválida).
    fn reset_boolean_model(&self) {
        let items = ["No default", "True", "False"];
        self.default_dd
            .set_model(Some(&gtk::StringList::new(&items)));
        self.default_dd.set_selected(0);
    }

    /// Fija las opciones del picker «From output».
    fn set_choices(&self, choices: &[OutputChoice]) {
        let Some(dropdown) = &self.from_output else {
            return;
        };
        let mut labels = vec!["Select previous output"];
        labels.extend(choices.iter().map(|choice| choice.label.as_str()));
        dropdown.set_model(Some(&gtk::StringList::new(&labels)));
        dropdown.set_selected(0);
    }
}

/// Una fila de output.
struct OutputRow {
    container: gtk::Box,
    key: gtk::Entry,
    label: gtk::Entry,
    path: gtk::Entry,
    var_btn: gtk::Button,
    remove_btn: gtk::Button,
}

impl OutputRow {
    fn new() -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
        container.add_css_class("opm-step-option-row");
        let main = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let key = gtk::Entry::new();
        key.set_width_chars(10);
        key.set_placeholder_text(Some("result_file"));
        let label = gtk::Entry::new();
        label.set_width_chars(10);
        label.set_placeholder_text(Some("Result file"));
        let path = gtk::Entry::new();
        path.set_placeholder_text(Some("outputs/result.txt"));
        path.set_hexpand(true);
        let var_btn = gtk::Button::with_label("Pipeline var");
        var_btn.set_tooltip_text(Some("Insert VARIABLE={key} in command"));
        let remove_btn = gtk::Button::with_label("✕");
        remove_btn.set_tooltip_text(Some("Remove output"));
        remove_btn.add_css_class("opm-danger-button");

        main.append(&cell("Key", &key));
        main.append(&cell("Label", &label));
        main.append(&cell("Path", &path));
        main.append(&var_btn);
        main.append(&remove_btn);
        container.append(&main);

        Rc::new(Self {
            container,
            key,
            label,
            path,
            var_btn,
            remove_btn,
        })
    }

    fn load(&self, draft: &OutputDraft) {
        self.key.set_text(&draft.key);
        self.label.set_text(&draft.label);
        self.path.set_text(&draft.path);
        self.sync_key();
    }

    fn sync_key(&self) {
        self.var_btn
            .set_sensitive(!sanitize_identifier(&self.key.text()).is_empty());
    }

    fn to_draft(&self) -> OutputDraft {
        OutputDraft {
            key: self.key.text().to_string(),
            label: self.label.text().to_string(),
            path: self.path.text().to_string(),
        }
    }
}

/// Celda `Etiqueta` + widget apilados verticalmente (estilo `field` SPA).
fn cell(label: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let vertical = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let caption = gtk::Label::new(Some(label));
    caption.set_halign(gtk::Align::Start);
    caption.set_xalign(0.0);
    caption.add_css_class("opm-dim");
    vertical.append(&caption);
    vertical.append(widget);
    vertical
}

/// Estado/ventana del diálogo. Los callbacks de los widgets capturan la
/// referencia DÉBIL (§11 A12: sin ciclos `Rc`).
struct DialogState {
    window: gtk::Window,
    steps: Vec<PipelineStep>,
    /// Raíz del proyecto (P2-15): con ella las opciones «From output» se
    /// calculan con los helpers de `opm-fs` como el store.
    root: Option<PathBuf>,
    original_id: Option<String>,
    suggested_id: String,
    dependencies: RefCell<Vec<String>>,
    id_entry: gtk::Entry,
    name_entry: gtk::Entry,
    description_entry: gtk::Entry,
    environment_entry: gtk::Entry,
    wd_entry: gtk::Entry,
    command: gtk::TextView,
    chips: gtk::FlowBox,
    footer: gtk::Label,
    submit: gtk::Button,
    inputs_box: gtk::Box,
    parameters_box: gtk::Box,
    outputs_box: gtk::Box,
    input_rows: RefCell<Vec<Rc<ValueRow>>>,
    parameter_rows: RefCell<Vec<Rc<ValueRow>>>,
    output_rows: RefCell<Vec<Rc<OutputRow>>>,
    choices: RefCell<Vec<OutputChoice>>,
    on_submit: Rc<dyn Fn(Option<String>, PipelineStep)>,
}

impl DialogState {
    fn resolved_wd(&self) -> String {
        let wd = self.wd_entry.text();
        let wd = wd.trim();
        if !wd.is_empty() {
            return wd.to_owned();
        }
        let id = clean_step_id(&self.id_entry.text(), &self.suggested_id);
        format!("steps/{id}/work")
    }

    /// Pie: guardia booleana o working directory resuelto (paridad SPA) +
    /// sensibilidad del botón de envío.
    fn refresh_guard(&self) {
        let guard = self
            .input_rows
            .borrow()
            .iter()
            .chain(self.parameter_rows.borrow().iter())
            .any(|row| row.has_invalid_boolean());
        self.submit.set_sensitive(!guard);
        if guard {
            self.footer
                .set_text("Choose a valid boolean default before saving.");
        } else {
            self.footer.set_text(&self.resolved_wd());
        }
    }

    /// Recalcula las opciones «From output» (dependen del wd consumidor) y
    /// las reparte a las filas de input.
    fn refresh_choices(&self) {
        let choices = output_choices(
            self.root.as_deref(),
            &self.steps,
            self.original_id.as_deref(),
            &self.resolved_wd(),
        );
        for row in self.input_rows.borrow().iter() {
            row.set_choices(&choices);
        }
        *self.choices.borrow_mut() = choices;
    }

    /// Chips `{keyword}` vivos desde las teclas actuales de las filas: el
    /// cómputo es el de [`StepDraft::command_keywords`] sobre el estado
    /// actual del formulario (una sola fuente con el envío). Cada chip
    /// inserta su placeholder en el comando (P2-7; captura `Weak`, sin ciclo
    /// diálogo → chip → diálogo).
    fn rebuild_chips(self: &Rc<Self>) {
        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        for keyword in self.current_draft().command_keywords() {
            let button = gtk::Button::with_label(&format!("{{{keyword}}}"));
            button.add_css_class("opm-chip");
            {
                let weak = Rc::downgrade(self);
                let text = format!("{{{keyword}}}");
                button.connect_clicked(move |_| {
                    if let Some(state) = weak.upgrade() {
                        state.insert_command(&text);
                    }
                });
            }
            self.chips.insert(&button, -1);
        }
    }

    /// Inserta `text` en el caret/selección del comando (paridad SPA
    /// `insertCommandText`: en el caret, sin espacios automáticos).
    fn insert_command(&self, text: &str) {
        let buffer = self.command.buffer();
        let (start, end) = match buffer.selection_bounds() {
            Some((first, second)) => (first.offset(), second.offset()),
            None => {
                let offset = buffer.iter_at_mark(&buffer.get_insert()).offset();
                (offset, offset)
            }
        };
        let current = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .to_string();
        let (next, caret) = insert_at(&current, start.max(0) as usize, end.max(0) as usize, text);
        buffer.set_text(&next);
        let caret = buffer.iter_at_offset(caret as i32);
        buffer.place_cursor(&caret);
        self.command.grab_focus();
    }

    fn insert_pipeline_var(&self, key: &str) {
        let clean = sanitize_identifier(key);
        if clean.is_empty() {
            return;
        }
        let variable = to_environment_variable(&clean);
        self.insert_command(&format!("{variable}={{{clean}}}"));
    }

    fn add_value_row(self: &Rc<Self>, section: Section, kind: FieldType, draft: Option<&ValueDraft>) {
        let with_from_output = section == Section::Inputs;
        let row = ValueRow::new(with_from_output);
        row.default_dd.set_selected(if kind == FieldType::Boolean {
            1
        } else {
            0
        });
        if let Some(draft) = draft {
            row.load(draft);
        } else {
            // Fila nueva: tipo por defecto (input = file, opción = text).
            let index = type_index(kind);
            row.kind.set_selected(index as u32);
            row.last_kind.set(index);
            row.sync_visibility();
        }
        row.set_choices(&self.choices.borrow());

        let (box_, rows) = match section {
            Section::Inputs => (&self.inputs_box, &self.input_rows),
            Section::Parameters => (&self.parameters_box, &self.parameter_rows),
        };
        box_.append(&row.container);
        rows.borrow_mut().push(Rc::clone(&row));

        self.wire_value_row(section, &row);
        self.rebuild_chips();
        self.refresh_guard();
    }

    fn add_output_row(self: &Rc<Self>, draft: Option<&OutputDraft>) {
        let row = OutputRow::new();
        if let Some(draft) = draft {
            row.load(draft);
        }
        self.outputs_box.append(&row.container);
        self.output_rows.borrow_mut().push(Rc::clone(&row));

        // Tecla viva → chips + sensibilidad de «Pipeline var» (P2-8: al
        // teclear o borrar la clave se re-evalúa `sync_key`; capturas
        // débiles para que quitar la fila suelte sus handlers, P2-4).
        {
            let weak = Rc::downgrade(self);
            let weak_row = Rc::downgrade(&row);
            row.key.connect_changed(move |_| {
                if let Some(handle) = weak_row.upgrade() {
                    handle.sync_key();
                }
                if let Some(state) = weak.upgrade() {
                    state.rebuild_chips();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            let weak_row = Rc::downgrade(&row);
            row.var_btn.connect_clicked(move |_| {
                let (Some(state), Some(handle)) = (weak.upgrade(), weak_row.upgrade()) else {
                    return;
                };
                let key = handle.key.text().to_string();
                state.insert_pipeline_var(&key);
            });
        }
        // Quitar output (P2-4: solo `Weak` — la fila muere con sus handlers).
        {
            let weak = Rc::downgrade(self);
            let weak_row = Rc::downgrade(&row);
            row.remove_btn.connect_clicked(move |_| {
                let (Some(state), Some(row)) = (weak.upgrade(), weak_row.upgrade()) else {
                    return;
                };
                state.remove_output_row(&row);
            });
        }
        row.sync_key();
        self.rebuild_chips();
    }

    /// Wiring de una fila de valor (tecla, tipo, defaults, picker, var,
    /// remove). Los closures llevan `Weak` (§11 A12).
    fn wire_value_row(self: &Rc<Self>, section: Section, row: &Rc<ValueRow>) {
        // Tecla: chips + sensibilidad del botón de variable (P2-4: `Weak`,
        // la fila muere con sus handlers).
        {
            let weak = Rc::downgrade(self);
            let weak_row = Rc::downgrade(row);
            row.key.connect_changed(move |_| {
                if let Some(handle) = weak_row.upgrade() {
                    handle.sync_visibility();
                }
                if let Some(state) = weak.upgrade() {
                    state.rebuild_chips();
                }
            });
        }
        // Tipo: transiciones con booleano limpian default+guardia, y hay
        // que re-sincronizar visibilidades (paridad SPA).
        {
            let weak = Rc::downgrade(self);
            let weak_row = Rc::downgrade(row);
            row.kind.connect_selected_notify(move |dropdown| {
                let Some(handle) = weak_row.upgrade() else {
                    return;
                };
                let new = dropdown.selected() as usize;
                let old = handle.last_kind.get();
                handle.last_kind.set(new);
                if old == BOOLEAN_TYPE_INDEX || new == BOOLEAN_TYPE_INDEX {
                    handle.default_entry.set_text("");
                    *handle.invalid.borrow_mut() = None;
                    handle.reset_boolean_model();
                }
                handle.sync_visibility();
                if let Some(state) = weak.upgrade() {
                    state.refresh_guard();
                }
            });
        }
        // Default booleano: al elegir un valor válido se limpia la guardia,
        // se sustituye el modelo de 4 items por el normal traduciendo el
        // índice y se refresca el pie — TODO en la misma operación (P1-2:
        // con el modelo inválido intacto, «True» se leía como «False» y se
        // guardaba `false`).
        {
            let weak = Rc::downgrade(self);
            let weak_row = Rc::downgrade(row);
            row.default_dd.connect_selected_notify(move |dropdown| {
                let (Some(state), Some(handle)) = (weak.upgrade(), weak_row.upgrade()) else {
                    return;
                };
                if handle.invalid.borrow().is_none() {
                    return;
                }
                let Some(target) = translate_invalid_boolean_index(dropdown.selected()) else {
                    // 0 («Invalid stored value») o sin selección: la guardia
                    // sigue armada.
                    return;
                };
                *handle.invalid.borrow_mut() = None;
                handle.reset_boolean_model();
                handle.default_dd.set_selected(target);
                state.refresh_guard();
            });
        }
        // «From output»: default relativo + source + dependencia automática.
        if let Some(dropdown) = row.from_output.clone() {
            let weak = Rc::downgrade(self);
            let weak_row = Rc::downgrade(row);
            dropdown.connect_selected_notify(move |dropdown| {
                let index = dropdown.selected();
                if index == 0 || index == u32::MAX {
                    return;
                }
                let (Some(state), Some(row)) = (weak.upgrade(), weak_row.upgrade()) else {
                    return;
                };
                let choice = state
                    .choices
                    .borrow()
                    .get(index as usize - 1)
                    .cloned();
                let Some(choice) = choice else {
                    return;
                };
                row.default_entry.set_text(&choice.default_value);
                *row.source.borrow_mut() = Some((choice.step_id.clone(), choice.source_output()));
                // Dependencia automática (§6): unión idempotente.
                let mut dependencies = state.dependencies.borrow_mut();
                ensure_dependency(&mut dependencies, &choice.step_id);
                drop(dependencies);
                dropdown.set_selected(0);
            });
        }
        // «Pipeline var».
        {
            let weak = Rc::downgrade(self);
            let weak_row = Rc::downgrade(row);
            row.var_btn.connect_clicked(move |_| {
                let (Some(state), Some(handle)) = (weak.upgrade(), weak_row.upgrade()) else {
                    return;
                };
                let key = handle.key.text().to_string();
                state.insert_pipeline_var(&key);
            });
        }
        // Quitar fila.
        {
            let weak = Rc::downgrade(self);
            let weak_row = Rc::downgrade(row);
            row.remove_btn.connect_clicked(move |_| {
                let (Some(state), Some(row)) = (weak.upgrade(), weak_row.upgrade()) else {
                    return;
                };
                state.remove_value_row(section, &row);
            });
        }
    }

    fn remove_value_row(self: &Rc<Self>, section: Section, row: &Rc<ValueRow>) {
        let rows = match section {
            Section::Inputs => &self.input_rows,
            Section::Parameters => &self.parameter_rows,
        };
        let mut rows = rows.borrow_mut();
        if let Some(index) = rows.iter().position(|candidate| Rc::ptr_eq(candidate, row)) {
            rows.remove(index);
            drop(rows);
            let box_ = match section {
                Section::Inputs => &self.inputs_box,
                Section::Parameters => &self.parameters_box,
            };
            box_.remove(&row.container);
            self.rebuild_chips();
            self.refresh_guard();
        }
    }

    fn remove_output_row(self: &Rc<Self>, row: &Rc<OutputRow>) {
        let mut rows = self.output_rows.borrow_mut();
        if let Some(index) = rows.iter().position(|candidate| Rc::ptr_eq(candidate, row)) {
            rows.remove(index);
            drop(rows);
            self.outputs_box.remove(&row.container);
            self.rebuild_chips();
        }
    }

    /// Estado actual del formulario como [`StepDraft`] (compartido entre el
    /// envío y los chips `{keyword}`: una sola fuente de verdad, §6).
    fn current_draft(&self) -> StepDraft {
        StepDraft {
            id: self.id_entry.text().to_string(),
            name: self.name_entry.text().to_string(),
            description: self.description_entry.text().to_string(),
            environment: self.environment_entry.text().to_string(),
            working_directory: self.wd_entry.text().to_string(),
            command: self
                .command
                .buffer()
                .text(
                    &self.command.buffer().start_iter(),
                    &self.command.buffer().end_iter(),
                    true,
                )
                .to_string(),
            inputs: self
                .input_rows
                .borrow()
                .iter()
                .map(|row| row.to_draft())
                .collect(),
            parameters: self
                .parameter_rows
                .borrow()
                .iter()
                .map(|row| row.to_draft())
                .collect(),
            outputs: self
                .output_rows
                .borrow()
                .iter()
                .map(|row| row.to_draft())
                .collect(),
            dependencies: self.dependencies.borrow().clone(),
            original_id: self.original_id.clone(),
            suggested_id: self.suggested_id.clone(),
            suggested_name: String::new(),
        }
    }

    /// Recogida + envío: `build()` devuelve `None` con la guardia activa
    /// (el pie ya muestra el mensaje y el botón está deshabilitado).
    fn submit(self: &Rc<Self>) {
        let draft = self.current_draft();
        let Some(step) = draft.build() else {
            self.refresh_guard();
            return;
        };
        let original = self.original_id.clone();
        let on_submit = Rc::clone(&self.on_submit);
        self.window.destroy();
        on_submit(original, step);
    }
}

/// Abre el diálogo de creación (`draft.original_id == None`) o de edición
/// (`Some`). `on_submit(original_id, paso)` recibe el paso armado; el
/// integrador llama a `store.create_step` / `store.update_step`.
pub fn open_step_dialog(
    parent: &impl IsA<gtk::Window>,
    draft: StepDraft,
    steps: Vec<PipelineStep>,
    root: Option<PathBuf>,
    on_submit: Rc<dyn Fn(Option<String>, PipelineStep)>,
) {
    let editing = draft.original_id.is_some();
    let window = gtk::Window::builder()
        .title(if editing { "Edit step" } else { "Create step" })
        .modal(true)
        .transient_for(parent.upcast_ref::<gtk::Window>())
        .default_width(700)
        .default_height(640)
        .resizable(true)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    // Cabecera.
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some(if editing {
        "Edit step"
    } else {
        "Create step"
    }));
    title.set_halign(gtk::Align::Start);
    title.set_hexpand(true);
    title.add_css_class("title-2");
    let close = gtk::Button::with_label("✕");
    close.set_tooltip_text(Some("Close"));
    header.append(&title);
    header.append(&close);
    vbox.append(&header);

    // Formulario.
    let form = gtk::Grid::new();
    form.set_row_spacing(8);
    form.set_column_spacing(10);
    let id_entry = gtk::Entry::new();
    id_entry.set_hexpand(true);
    let name_entry = gtk::Entry::new();
    name_entry.set_hexpand(true);
    let environment_entry = gtk::Entry::new();
    environment_entry.set_placeholder_text(Some("conda:analysis"));
    environment_entry.set_hexpand(true);
    let wd_entry = gtk::Entry::new();
    wd_entry.set_hexpand(true);
    let description_entry = gtk::Entry::new();
    description_entry.set_hexpand(true);
    form.attach(&cell("Step id", &id_entry), 0, 0, 1, 1);
    form.attach(&cell("Name", &name_entry), 1, 0, 1, 1);
    form.attach(&cell("Environment", &environment_entry), 0, 1, 1, 1);
    form.attach(&cell("Working directory", &wd_entry), 1, 1, 1, 1);
    form.attach(&cell("Description", &description_entry), 0, 2, 2, 1);
    vbox.append(&form);

    // Comando + chips.
    let command_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let command_caption = gtk::Label::new(Some("Command"));
    command_caption.set_halign(gtk::Align::Start);
    command_caption.add_css_class("opm-dim");
    let command = gtk::TextView::new();
    command.set_wrap_mode(gtk::WrapMode::WordChar);
    command.set_monospace(true);
    command.set_left_margin(4);
    command.set_right_margin(4);
    let command_scroll = gtk::ScrolledWindow::new();
    command_scroll.set_child(Some(&command));
    command_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    command_scroll.set_min_content_height(64);
    command.set_tooltip_text(Some("python script.py --mode {mode}"));
    let chips = gtk::FlowBox::new();
    chips.set_selection_mode(gtk::SelectionMode::None);
    chips.set_min_children_per_line(6);
    chips.set_max_children_per_line(12);
    chips.set_homogeneous(false);
    command_box.append(&command_caption);
    command_box.append(&command_scroll);
    command_box.append(&chips);
    vbox.append(&command_box);

    // Secciones con scroll.
    let sections_scroll = gtk::ScrolledWindow::new();
    sections_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sections_scroll.set_vexpand(true);
    let sections = gtk::Box::new(gtk::Orientation::Vertical, 12);

    let inputs_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    sections.append(&value_section(
        "Inputs",
        "Add input",
        &inputs_box,
        Some((Section::Inputs, FieldType::File)),
    ));
    let parameters_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    sections.append(&value_section(
        "Options",
        "Add option",
        &parameters_box,
        Some((Section::Parameters, FieldType::Text)),
    ));
    let outputs_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    sections.append(&value_section(
        "Outputs",
        "Add output",
        &outputs_box,
        None,
    ));
    sections_scroll.set_child(Some(&sections));
    vbox.append(&sections_scroll);

    // Pie.
    let footer_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let footer = gtk::Label::new(None);
    footer.set_halign(gtk::Align::Start);
    footer.set_xalign(0.0);
    footer.set_hexpand(true);
    footer.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    footer.add_css_class("opm-dim");
    let cancel = gtk::Button::with_label("Cancel");
    let submit = gtk::Button::with_label(if editing {
        "Save step"
    } else {
        "Create step"
    });
    submit.add_css_class("suggested-action");
    footer_row.append(&footer);
    footer_row.append(&cancel);
    footer_row.append(&submit);
    vbox.append(&footer_row);

    window.set_child(Some(&vbox));

    let state = Rc::new(DialogState {
        window: window.clone(),
        steps,
        root,
        original_id: draft.original_id.clone(),
        suggested_id: draft.suggested_id.clone(),
        dependencies: RefCell::new(draft.dependencies.clone()),
        id_entry: id_entry.clone(),
        name_entry: name_entry.clone(),
        description_entry: description_entry.clone(),
        environment_entry: environment_entry.clone(),
        wd_entry: wd_entry.clone(),
        command: command.clone(),
        chips,
        footer,
        submit: submit.clone(),
        inputs_box,
        parameters_box,
        outputs_box,
        input_rows: RefCell::new(Vec::new()),
        parameter_rows: RefCell::new(Vec::new()),
        output_rows: RefCell::new(Vec::new()),
        choices: RefCell::new(Vec::new()),
        on_submit,
    });

    // Wiring global.
    // Retención fuerte del estado: los handlers usan `Weak` (§11 A12, sin
    // ciclos), así que ALGO debe mantener el `Rc` vivo mientras la ventana
    // exista — el guardián se vacía en `destroy`, al cerrar, y con él caen
    // el estado y el callback `on_submit`.
    let alive = Rc::new(RefCell::new(Some(Rc::clone(&state))));
    window.connect_destroy({
        let alive = Rc::clone(&alive);
        move |_| {
            *alive.borrow_mut() = None;
        }
    });
    {
        let weak = Rc::downgrade(&state);
        submit.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.submit();
            }
        });
    }
    // P2-7: la activación del FlowBox (Enter / activación de fila) inserta
    // el chip activado — mismo destino que su clic, captura `Weak`.
    {
        let weak = Rc::downgrade(&state);
        state.chips.connect_child_activated(move |_, child| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let text = child
                .child()
                .and_downcast::<gtk::Button>()
                .and_then(|button| button.child())
                .and_downcast::<gtk::Label>()
                .map(|label| label.text().to_string())
                .filter(|label| label.starts_with('{') && label.ends_with('}'));
            if let Some(text) = text {
                state.insert_command(&text);
            }
        });
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.destroy());
    }
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.destroy());
    }
    // wd/id vivos: pie + opciones «From output» + placeholder del wd.
    {
        let weak = Rc::downgrade(&state);
        wd_entry.connect_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                state.refresh_guard();
                state.refresh_choices();
            }
        });
    }
    {
        let weak = Rc::downgrade(&state);
        id_entry.connect_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                let placeholder = state.resolved_wd();
                state.wd_entry.set_placeholder_text(Some(&placeholder));
                state.refresh_guard();
                state.refresh_choices();
            }
        });
    }
    // Botones «Add …».
    for (add_button, action) in [
        (
            find_button(&vbox, "Add input"),
            Some((Section::Inputs, FieldType::File)),
        ),
        (
            find_button(&vbox, "Add option"),
            Some((Section::Parameters, FieldType::Text)),
        ),
        (find_button(&vbox, "Add output"), None),
    ] {
        let Some(button) = add_button else { continue };
        let weak = Rc::downgrade(&state);
        button.connect_clicked(move |_| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            match action {
                Some((section, kind)) => state.add_value_row(section, kind, None),
                None => state.add_output_row(None),
            }
        });
    }

    // Prefilled + pie inicial.
    state.id_entry.set_text(&draft.id);
    state.name_entry.set_text(&draft.name);
    state.description_entry.set_text(&draft.description);
    state.environment_entry.set_text(&draft.environment);
    state.wd_entry.set_text(&draft.working_directory);
    state.command.buffer().set_text(&draft.command);
    state.wd_entry.set_placeholder_text(Some(&state.resolved_wd()));
    for input in &draft.inputs {
        state.add_value_row(Section::Inputs, input.kind, Some(input));
    }
    for parameter in &draft.parameters {
        state.add_value_row(Section::Parameters, parameter.kind, Some(parameter));
    }
    for output in &draft.outputs {
        state.add_output_row(Some(output));
    }
    state.refresh_choices();
    state.rebuild_chips();
    state.refresh_guard();

    window.present();
}

/// Sección `Etiqueta + botón Add + lista de filas`.
fn value_section(
    title: &str,
    add_label: &str,
    rows_box: &gtk::Box,
    add_action: Option<(Section, FieldType)>,
) -> gtk::Box {
    let _ = add_action;
    let section = gtk::Box::new(gtk::Orientation::Vertical, 6);
    section.set_margin_top(4);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some(title));
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);
    label.add_css_class("opm-section-title");
    let add = gtk::Button::with_label(add_label);
    add.set_widget_name(&format!("opm-add-{add_label}").replace(' ', "-"));
    head.append(&label);
    head.append(&add);
    section.append(&head);
    section.append(rows_box);
    section
}

/// Localiza el botón «Add …» por su nombre de widget (construido en
/// [`value_section`]).
fn find_button(root: &impl IsA<gtk::Widget>, add_label: &str) -> Option<gtk::Button> {
    let wanted = format!("opm-add-{add_label}").replace(' ', "-");
    let mut queue: Vec<gtk::Widget> = vec![root.upcast_ref::<gtk::Widget>().clone()];
    while let Some(widget) = queue.pop() {
        if widget.widget_name() == wanted {
            if let Ok(button) = widget.clone().downcast::<gtk::Button>() {
                return Some(button);
            }
        }
        if let Some(child) = widget.first_child() {
            queue.push(child);
        }
        let mut sibling = widget.next_sibling();
        while let Some(next) = sibling {
            sibling = next.next_sibling();
            queue.push(next);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, wd: &str) -> PipelineStep {
        PipelineStep {
            id: id.to_owned(),
            name: format!("Step {id}"),
            description: String::new(),
            command: String::new(),
            environment: String::new(),
            working_directory: wd.to_owned(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            parameters: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    /// Guardia del booleano: default guardado no booleano BLOQUEA `build`;
    /// al validarlo (o cambiar de tipo, que la limpia) pasa (§6).
    #[test]
    fn guardia_de_default_booleano() {
        let mut draft = StepDraft::for_create(&[]);
        draft.inputs.push(ValueDraft {
            kind: FieldType::Boolean,
            key: "flag".into(),
            label: String::new(),
            required: false,
            default_text: String::new(),
            choices: String::new(),
            min: String::new(),
            max: String::new(),
            source_step: None,
            source_output: None,
            invalid_boolean: Some("42".into()),
        });
        assert!(draft.has_invalid_boolean());
        assert!(draft.build().is_none(), "el envío queda bloqueado");

        // Default válido → pasa y se serializa como bool nativo.
        draft.inputs[0].invalid_boolean = None;
        draft.inputs[0].default_text = "true".into();
        assert!(!draft.has_invalid_boolean());
        let built = draft.build().expect("guardia desactivada");
        assert_eq!(built.inputs[0].default, Some(Value::Bool(true)));

        // Un flag inválido en fila NO booleana no bloquea (paridad SPA).
        draft.inputs[0].kind = FieldType::Text;
        draft.inputs[0].invalid_boolean = Some("42".into());
        assert!(!draft.has_invalid_boolean());
        assert!(draft.build().is_some());
    }

    /// P1-2: traducción del índice del dropdown booleano — desde el modelo
    /// CON la opción inválida y en el modelo normal. Leer y escribir deben
    /// correr sobre el MISMO modelo: con el modelo de 4 items sin sustituir,
    /// la posición mostrada «True» (2) se leía como «false».
    #[test]
    fn traduccion_de_indice_booleano_invalido() {
        // Modelo inválido (4 items): 1 «No default», 2 «True», 3 «False».
        assert_eq!(translate_invalid_boolean_index(1), Some(0));
        assert_eq!(translate_invalid_boolean_index(2), Some(1));
        assert_eq!(translate_invalid_boolean_index(3), Some(2));
        // «Invalid stored value» (0) y sin selección: la guardia sigue.
        assert_eq!(translate_invalid_boolean_index(0), None);
        assert_eq!(translate_invalid_boolean_index(u32::MAX), None);

        // Lectura en el modelo inválido…
        assert_eq!(boolean_default_text_at(2, true), "true");
        assert_eq!(boolean_default_text_at(3, true), "false");
        assert_eq!(boolean_default_text_at(1, true), "");
        assert_eq!(boolean_default_text_at(0, true), "");

        // …y tras la traducción, en el normal: EL MISMO valor (regresión
        // P1-2: sin sustituir el modelo, selected=2 se guardaba como false).
        for invalid_selected in 1..=3u32 {
            let normal = translate_invalid_boolean_index(invalid_selected).unwrap();
            assert_eq!(
                boolean_default_text_at(invalid_selected, true),
                boolean_default_text_at(normal, false),
                "la traducción cambia el valor mostrado"
            );
        }

        // Estado normal de toda la vida.
        assert_eq!(boolean_default_text_at(1, false), "true");
        assert_eq!(boolean_default_text_at(2, false), "false");
        assert_eq!(boolean_default_text_at(0, false), "");
        assert_eq!(boolean_default_text_at(u32::MAX, false), "");
    }

    /// Spec booleano con default corrupto → draft con la guardia armada
    /// (prefilled de edición, paridad `toValueDraft`).
    #[test]
    fn spec_booleano_corrupto_activa_la_guardia() {
        let spec = ValueSpec {
            key: "flag".into(),
            label: Some("flag".into()),
            kind: FieldType::Boolean,
            required: false,
            default: Some(Value::Str("yes".into())),
            options: Vec::new(),
            min: None,
            max: None,
            description: String::new(),
            source_step: None,
            source_output: None,
        };
        let draft = value_draft_from_spec(&spec);
        assert_eq!(draft.invalid_boolean.as_deref(), Some("yes"));
        let mut step = step("s", ".");
        step.inputs.push(spec);
        let mut edit = StepDraft::for_edit(&step);
        assert!(edit.has_invalid_boolean());
        assert!(edit.build().is_none());
        // Corrigiendo el draft de edición → compila.
        edit.inputs[0].invalid_boolean = None;
        edit.inputs[0].default_text = "false".into();
        assert!(edit.build().is_some());
    }

    /// «From output»: ruta relativa desde el wd consumidor (pinned:
    /// `../output/result.txt`), `source_step`/`source_output` y dependencia
    /// automática (§6).
    #[test]
    fn from_output_relativo_y_dependencia_auto() {
        let mut produce = step("produce", ".");
        produce.outputs.push(OutputSpec {
            key: "result".into(),
            label: None,
            path: "output/result.txt".into(),
        });
        let consume = step("consume", "analysis");
        let steps = vec![produce, consume];

        let choices = output_choices(Some(Path::new("/proy")), &steps, Some("consume"), "analysis");
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].label, "produce -> output/result.txt");
        assert_eq!(choices[0].default_value, "../output/result.txt");
        assert_eq!(choices[0].source_output(), "result");

        let mut draft = StepDraft::for_edit(&steps[1]);
        draft.inputs.push(ValueDraft {
            key: "result_file".into(),
            label: String::new(),
            kind: FieldType::File,
            required: true,
            default_text: String::new(),
            choices: String::new(),
            min: String::new(),
            max: String::new(),
            source_step: None,
            source_output: None,
            invalid_boolean: None,
        });
        // La unión de la dependencia es idempotente (paridad SPA) y el
        // diálogo aplica el mismo núcleo (`ensure_dependency`) que fija
        // default + `source_step`/`source_output` en la fila.
        assert!(ensure_dependency(&mut draft.dependencies, &choices[0].step_id));
        assert!(
            !ensure_dependency(&mut draft.dependencies, &choices[0].step_id),
            "la segunda vez ya estaba (unión)"
        );
        draft.inputs[0].default_text = choices[0].default_value.clone();
        draft.inputs[0].source_step = Some(choices[0].step_id.clone());
        draft.inputs[0].source_output = Some(choices[0].source_output());
        assert_eq!(draft.dependencies, vec!["produce".to_owned()]);

        let built = draft.build().unwrap();
        assert_eq!(
            built.inputs[0].default,
            Some(Value::Str("../output/result.txt".into()))
        );
        assert_eq!(built.inputs[0].source_step.as_deref(), Some("produce"));
        assert_eq!(built.inputs[0].source_output.as_deref(), Some("result"));
        assert_eq!(built.dependencies, vec!["produce".to_owned()]);

        // En modo crear (sin paso actual) se ven TODOS los outputs.
        assert_eq!(
            output_choices(Some(Path::new("/proy")), &steps, None, ".").len(),
            1
        );
        // Un paso posterior no ve outputs futuros.
        assert!(
            output_choices(Some(Path::new("/proy")), &steps, Some("produce"), ".").is_empty()
        );
    }

    /// P2-15: la vista previa «From output» usa LA cadena del store
    /// (`resolve_pipeline_path` + `relative_project_path`) — pin Python:
    /// `os.path.relpath("outputs/r.txt", start="steps/w/work")`; y sin raíz
    /// de proyecto no hay opciones.
    #[test]
    fn relativas_del_picker() {
        let mut produce = step("produce", ".");
        produce.outputs.push(OutputSpec {
            key: "r".into(),
            label: None,
            path: "outputs/r.txt".into(),
        });
        let steps = vec![produce, step("w", "steps/w/work")];

        let choices = output_choices(Some(Path::new("/proy")), &steps, Some("w"), "steps/w/work");
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].default_value, "../../../outputs/r.txt");
        assert_eq!(choices[0].source_output(), "r");

        // Raíz ausente (no debería pasar: el diálogo exige proyecto) → nada.
        assert!(output_choices(None, &steps, Some("w"), "steps/w/work").is_empty());

        // Fuente insegura (path absoluto) → la opción no se ofrece (la
        // validación la bloquearía como «Unsafe input source path»).
        let mut abs = step("bad", ".");
        abs.outputs.push(OutputSpec {
            key: "o".into(),
            label: None,
            path: "/etc/passwd".into(),
        });
        assert!(
            output_choices(Some(Path::new("/proy")), &[abs], None, ".").is_empty(),
            "las fuentes inseguras no se ofrecen"
        );
    }

    /// Inserción de chips por caret (offsets en chars) — selección incluida.
    #[test]
    fn insercion_de_chips_en_el_caret() {
        let (next, caret) = insert_at("echo", 4, 4, " {mode}");
        assert_eq!(next, "echo {mode}");
        assert_eq!(caret, 11);

        // Reemplaza la selección.
        let (next, caret) = insert_at("echo ABC", 5, 6, "x");
        assert_eq!(next, "echo xBC");
        assert_eq!(caret, 6);

        // Fuera de rango: se recorta sin panic.
        let (next, caret) = insert_at("hola", 99, 99, "!");
        assert_eq!(next, "hola!");
        assert_eq!(caret, 5);

        // «Pipeline var»: UPPER={key}.
        assert_eq!(to_environment_variable("my-key"), "MY_KEY");
        assert_eq!(to_environment_variable("1st"), "VAR_1ST");
        assert_eq!(to_environment_variable("mode"), "MODE");
    }

    /// Id auto `step_<n+1>` + saneado con `safe_segment` (§6) y fallback.
    #[test]
    fn ids_y_saneado() {
        let steps = vec![step("a", "."), step("b", ".")];
        let draft = StepDraft::for_create(&steps);
        assert_eq!(draft.id, "step_3");
        assert_eq!(draft.name, "Step 3");
        assert_eq!(draft.clean_id(), "step_3");

        // Edit: el id original es el fallback.
        let edit = StepDraft::for_edit(&steps[0]);
        assert_eq!(edit.original_id.as_deref(), Some("a"));
        assert_eq!(edit.clean_id(), "a");

        // Saneado: espacios y basura → safe_segment; sin nada → fallback.
        assert_eq!(clean_step_id("My Step!", "step_1"), "My_Step");
        assert_eq!(clean_step_id("!!!", "step_1"), "step_1");
        assert_eq!(clean_step_id("", "step_1"), "step_1");
        assert_eq!(clean_step_id("   ", "step_1"), "step_1");
        assert_eq!(clean_step_id("project!", "step_1"), "project");
        assert_eq!(sanitize_identifier("my key!"), "my_key");
        assert_eq!(sanitize_identifier("  ..dots..  "), "dots");
        assert_eq!(sanitize_identifier("!!"), "");
    }

    /// `parse_default` con las reglas del SPA (booleano, parseInt, finitos,
    /// file/folder sin recortar).
    #[test]
    fn parse_de_defaults() {
        assert_eq!(parse_default(FieldType::Boolean, "true"), Some(Value::Bool(true)));
        assert_eq!(parse_default(FieldType::Boolean, "nope"), None);
        assert_eq!(parse_default(FieldType::Integer, "3.5"), Some(Value::Int(3)));
        assert_eq!(parse_default(FieldType::Integer, "3abc"), Some(Value::Int(3)));
        assert_eq!(parse_default(FieldType::Integer, "x"), None);
        assert_eq!(parse_default(FieldType::Decimal, "2.25"), Some(Value::Float(2.25)));
        assert_eq!(parse_default(FieldType::Decimal, "NaN"), None);
        assert_eq!(
            parse_default(FieldType::File, "  a b.txt "),
            Some(Value::Str("  a b.txt ".into()))
        );
        assert_eq!(parse_default(FieldType::Text, " hi "), Some(Value::Str("hi".into())));
        assert_eq!(parse_default(FieldType::Text, "   "), None);
    }

    /// Arma del paso: nombre/id/wd con defaults, claves vacías fuera y
    /// outputs sin path fuera (paridad SPA).
    #[test]
    fn armado_del_paso() {
        let mut draft = StepDraft::for_create(&[]);
        draft.id = "  ".into();
        draft.name = String::new();
        draft.command = "  echo hi  ".into();
        draft.inputs.push(ValueDraft {
            key: "  ".into(),
            label: String::new(),
            kind: FieldType::Text,
            required: false,
            default_text: String::new(),
            choices: String::new(),
            min: String::new(),
            max: String::new(),
            source_step: None,
            source_output: None,
            invalid_boolean: None,
        });
        draft.outputs.push(OutputDraft {
            key: "res".into(),
            label: String::new(),
            path: "  ".into(),
        });
        draft.outputs.push(OutputDraft {
            key: "res ult".into(),
            label: String::new(),
            path: "outputs/r.txt".into(),
        });
        let built = draft.build().unwrap();
        assert_eq!(built.id, "step_1", "id vacío → sugerido");
        assert_eq!(built.name, "step_1", "nombre vacío → id");
        assert_eq!(built.working_directory, "steps/step_1/work");
        assert_eq!(built.command, "echo hi");
        assert!(built.inputs.is_empty(), "clave vacía → fila fuera");
        assert_eq!(built.outputs.len(), 1, "path vacío → fila fuera");
        assert_eq!(built.outputs[0].key, "res_ult");
        assert_eq!(built.outputs[0].label, None);
    }

    /// Chips: claves saneadas de inputs/params/outputs, sin duplicados.
    #[test]
    fn chips_de_palabras_clave() {
        let mut draft = StepDraft::for_create(&[]);
        draft.inputs.push(ValueDraft {
            key: "input file".into(),
            ..draft.inputs.first().cloned().unwrap_or(ValueDraft {
                key: String::new(),
                label: String::new(),
                kind: FieldType::Text,
                required: false,
                default_text: String::new(),
                choices: String::new(),
                min: String::new(),
                max: String::new(),
                source_step: None,
                source_output: None,
                invalid_boolean: None,
            })
        });
        draft.parameters.push(ValueDraft {
            key: "input_file".into(),
            ..draft.inputs[0].clone()
        });
        draft.outputs.push(OutputDraft {
            key: "input file".into(),
            label: String::new(),
            path: "x".into(),
        });
        assert_eq!(
            draft.command_keywords(),
            vec!["input_file".to_owned()],
            "dedup tras saneado (input file == input_file)"
        );
    }

    /// Montaje del diálogo (requiere sesión gráfica).
    #[test]
    #[ignore = "requiere sesión gráfica GTK"]
    fn dialogo_se_construye_y_prefilled() {
        assert!(gtk::init().is_ok());
        let parent = gtk::Window::new();
        let steps = vec![step("a", ".")];
        let draft = StepDraft::for_edit(&steps[0]);
        let submitted = Rc::new(Cell::new(None));
        let sink = Rc::clone(&submitted);
        open_step_dialog(&parent, draft, steps, None, Rc::new(move |original, step| {
            sink.set(Some((original, step)));
        }));
        parent.destroy();
        assert!(
            submitted.take().is_none(),
            "sin envío no hay callback"
        );
    }
}
