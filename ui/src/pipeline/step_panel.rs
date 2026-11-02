//! Panel derecho de detalle del paso (§6, port de `StepPanel.tsx`).
//!
//! - Estado vacío «No step selected»; con paso: nombre + Edit/Run, pastilla
//!   de estado, meta Environment (`default` cuando vacío) + Command,
//!   editores de Inputs y Parameters (valor = `params[key] ?? default ?? ''`),
//!   lista Outputs, sección Validation y el error inline `state.message`.
//! - Boolean → checkbox y select → dropdown commitean **de inmediato**;
//!   número/texto commitean **al perder el foco** (número vía parse `f64`,
//!   § del enunciado), con min/max como atributos del campo.
//! - Cada campo lleva debajo el mensaje de validación de su `key` (paridad
//!   con el `<small>` del SPA).
//!
//! [`StepPanel::select`] reconstruye el árbol (cambio de paso);
//! [`StepPanel::refresh`] actualiza SOLO pastilla, mensajes y validación —
//! nunca destruye el campo con el que el usuario está editando.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;

use opm_pipeline::{FieldType, PipelineStep, ValidationIssue, ValueSpec};
use opm_store::StepDetail;
use opm_pipeline::Value;

/// Acciones del panel hacia el integrador.
pub struct StepCallbacks {
    /// Abre el diálogo de edición del paso.
    pub on_edit: Rc<dyn Fn(PipelineStep)>,
    /// Ejecuta el paso (§6: habilitado en reposo, A10).
    pub on_run: Rc<dyn Fn(String)>,
    /// Commitea un valor de parámetro inmediato (`store.set_params`).
    pub on_param: Rc<dyn Fn(String, String, Value)>,
}

/// Panel de detalle del paso seleccionado.
pub struct StepPanel {
    root: gtk::ScrolledWindow,
    content: gtk::Box,
    running: Cell<bool>,
    current: RefCell<Option<String>>,
    /// Labels de validación por campo `(paso, key)` para refresco suave.
    field_issues: RefCell<Vec<(String, String, gtk::Label)>>,
    /// Botones Edit/Run del paso mostrado (`None` en el estado vacío).
    actions: RefCell<Option<(gtk::Button, gtk::Button)>>,
    /// Secciones que el refresco suave reconstruye.
    pill: RefCell<Option<(gtk::Label, String)>>,
    validation_box: RefCell<Option<gtk::Box>>,
    message: RefCell<Option<(gtk::Label, String)>>,
    name_label: RefCell<Option<gtk::Label>>,
    callbacks: StepCallbacks,
}

impl StepPanel {
    pub fn new(callbacks: StepCallbacks) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        content.set_margin_start(10);
        content.set_margin_end(10);

        let root = gtk::ScrolledWindow::new();
        root.set_child(Some(&content));
        root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        root.set_size_request(300, -1);
        root.add_css_class("opm-step-panel");

        let panel = Rc::new(Self {
            root,
            content,
            running: Cell::new(false),
            current: RefCell::new(None),
            field_issues: RefCell::new(Vec::new()),
            actions: RefCell::new(None),
            pill: RefCell::new(None),
            validation_box: RefCell::new(None),
            message: RefCell::new(None),
            name_label: RefCell::new(None),
            callbacks,
        });
        // Estado vacío inicial (§6 «No step selected»): `refresh(None)` con
        // `current == None` no reconstruye nada, así que el primer pintado
        // es responsabilidad del constructor.
        panel.rebuild(None);
        panel
    }

    pub fn widget(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    /// Paso seleccionado actualmente.
    pub fn current_step(&self) -> Option<String> {
        self.current.borrow().clone()
    }

    /// Habilita/deshabilita Edit/Run (§11 A12: acciones estructurales
    /// deshabilitadas mientras corre; A10: Run habilitado en reposo).
    pub fn set_running(&self, running: bool) {
        self.running.set(running);
        if let Some((edit, run)) = self.actions.borrow().as_ref() {
            edit.set_sensitive(!running);
            run.set_sensitive(!running);
        }
    }

    /// Selección de paso (construcción completa; `None` → estado vacío).
    pub fn select(&self, detail: Option<&StepDetail>) {
        self.current
            .replace(detail.map(|detail| detail.step.id.clone()));
        self.rebuild(detail);
    }

    /// Refresco de contenido sin tocar los editores (mismo paso).
    pub fn refresh(&self, detail: Option<&StepDetail>) {
        let same_step = detail.map(|detail| detail.step.id.clone()) == *self.current.borrow();
        if !same_step {
            self.select(detail);
            return;
        }
        let Some(detail) = detail else { return };
        // Pastilla de estado.
        if let Some((label, step)) = self.pill.borrow().as_ref() {
            if step == &detail.step.id {
                set_pill(label, &detail.state.status);
            }
        }
        // Título (el nombre puede cambiar tras editar).
        if let Some(label) = self.name_label.borrow().as_ref() {
            label.set_text(&detail.step.name);
        }
        // Mensajes de validación por campo.
        let mut fields = self.field_issues.borrow_mut();
        for (step, key, label) in fields.iter_mut() {
            if step != &detail.step.id {
                continue;
            }
            let issue = issue_for(&detail.validation, key);
            set_issue_label(label, issue);
        }
        drop(fields);
        // Sección Validation.
        if let Some((_, step)) = self.pill.borrow().as_ref() {
            if step == &detail.step.id {
                if let Some(box_) = self.validation_box.borrow().as_ref() {
                    fill_validation(box_, &detail.validation);
                }
                if let Some((label, step)) = self.message.borrow().as_ref() {
                    if step == &detail.step.id {
                        set_message(label, &detail.state.message);
                    }
                }
            }
        }
    }

    fn rebuild(&self, detail: Option<&StepDetail>) {
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        self.field_issues.borrow_mut().clear();
        self.actions.borrow_mut().take();
        self.pill.borrow_mut().take();
        self.validation_box.borrow_mut().take();
        self.message.borrow_mut().take();
        self.name_label.borrow_mut().take();

        let Some(detail) = detail else {
            // Estado vacío (§6): dentro de la caja del panel, bajo la
            // cabecera «Step» que fija el layout.
            let empty = gtk::Label::new(Some("No step selected"));
            empty.set_halign(gtk::Align::Start);
            empty.set_margin_top(8);
            empty.add_css_class("opm-empty");
            self.content.append(&empty);
            return;
        };

        let step = &detail.step;
        let running = self.running.get();

        // Cabecera: nombre + Edit/Run.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_bottom(4);
        let name = gtk::Label::new(Some(&step.name));
        name.set_halign(gtk::Align::Start);
        name.set_hexpand(true);
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.add_css_class("opm-step-name");
        let edit = gtk::Button::with_label("Edit");
        edit.set_tooltip_text(Some("Edit step"));
        let run = gtk::Button::with_label("Run");
        run.set_tooltip_text(Some("Run step"));
        edit.set_sensitive(!running);
        run.set_sensitive(!running);
        {
            let on_edit = Rc::clone(&self.callbacks.on_edit);
            let step = step.clone();
            edit.connect_clicked(move |_| on_edit(step.clone()));
        }
        {
            let on_run = Rc::clone(&self.callbacks.on_run);
            let id = step.id.clone();
            run.connect_clicked(move |_| on_run(id.clone()));
        }
        header.append(&name);
        header.append(&edit);
        header.append(&run);
        self.content.append(&header);
        *self.name_label.borrow_mut() = Some(name);
        *self.actions.borrow_mut() = Some((edit, run));

        // Pastilla de estado.
        let pill = gtk::Label::new(None);
        pill.set_halign(gtk::Align::Start);
        pill.set_margin_bottom(6);
        set_pill(&pill, &detail.state.status);
        self.content.append(&pill);
        *self.pill.borrow_mut() = Some((pill, step.id.clone()));

        // Meta: Environment + Command.
        let meta = gtk::Box::new(gtk::Orientation::Vertical, 2);
        meta.set_margin_bottom(8);
        meta.append(&meta_row("Environment", if step.environment.is_empty() { "default" } else { &step.environment }));
        meta.append(&meta_row("Command", if step.command.is_empty() { "No command" } else { &step.command }));
        self.content.append(&meta);

        // Inputs y Parameters.
        self.content
            .append(&self.build_section("Inputs", &step.id, &step.inputs, detail));
        self.content.append(&self.build_section(
            "Parameters",
            &step.id,
            &step.parameters,
            detail,
        ));

        // Outputs.
        let outputs = gtk::Box::new(gtk::Orientation::Vertical, 4);
        outputs.set_margin_top(8);
        outputs.append(&heading("Outputs"));
        if step.outputs.is_empty() {
            outputs.append(&empty_label());
        } else {
            for output in &step.outputs {
                let label = gtk::Label::new(Some(&output.path));
                label.set_halign(gtk::Align::Start);
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.add_css_class("opm-mono");
                label.set_tooltip_text(Some(&output.path));
                outputs.append(&label);
            }
        }
        self.content.append(&outputs);

        // Validation.
        let validation = gtk::Box::new(gtk::Orientation::Vertical, 4);
        validation.set_margin_top(8);
        validation.append(&heading("Validation"));
        fill_validation(&validation, &detail.validation);
        self.content.append(&validation);
        *self.validation_box.borrow_mut() = Some(validation);

        // Error inline `state.message`.
        let message = gtk::Label::new(None);
        message.set_halign(gtk::Align::Start);
        message.set_xalign(0.0);
        message.set_wrap(true);
        message.set_margin_top(8);
        message.add_css_class("opm-inline-error");
        set_message(&message, &detail.state.message);
        self.content.append(&message);
        *self.message.borrow_mut() = Some((message, step.id.clone()));
    }

    /// Sección de editores («None» vacío, paridad SPA).
    fn build_section(
        &self,
        title: &str,
        step_id: &str,
        specs: &[ValueSpec],
        detail: &StepDetail,
    ) -> gtk::Box {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 6);
        section.set_margin_top(8);
        section.append(&heading(title));
        if specs.is_empty() {
            section.append(&empty_label());
            return section;
        }
        for spec in specs {
            let value = detail
                .params
                .get(&spec.key)
                .or(spec.default.as_ref())
                .cloned()
                .unwrap_or(Value::Null);
            let issue = issue_for(&detail.validation, &spec.key);
            let field = self.build_field(step_id, spec, &value, issue);
            section.append(&field);
        }
        section
    }

    /// Un campo: etiqueta + editor (checkbox/dropdown/entrada) + mensaje de
    /// validación debajo.
    fn build_field(
        &self,
        step_id: &str,
        spec: &ValueSpec,
        value: &Value,
        issue: Option<&ValidationIssue>,
    ) -> gtk::Box {
        let field = gtk::Box::new(gtk::Orientation::Vertical, 2);
        field.add_css_class("opm-field");

        let label = gtk::Label::new(Some(
            spec.label.as_deref().filter(|text| !text.is_empty()).unwrap_or(&spec.key),
        ));
        label.set_halign(gtk::Align::Start);
        label.set_xalign(0.0);
        field.append(&label);

        let commit = {
            let on_param = Rc::clone(&self.callbacks.on_param);
            let step_id = step_id.to_owned();
            let key = spec.key.clone();
            Rc::new(move |value: Value| on_param(step_id.clone(), key.clone(), value))
        };

        match spec.kind {
            FieldType::Boolean => {
                let check = gtk::CheckButton::new();
                check.set_active(matches!(value, Value::Bool(true)));
                check.set_halign(gtk::Align::Start);
                {
                    let commit = Rc::clone(&commit);
                    check.connect_toggled(move |check| commit(Value::Bool(check.is_active())));
                }
                field.append(&check);
            }
            FieldType::Select => {
                let options: Vec<&str> = spec.options.iter().map(String::as_str).collect();
                let dropdown = gtk::DropDown::from_strings(&options);
                dropdown.set_halign(gtk::Align::Start);
                // El valor actual puede no estar en las opciones (sin
                // default): se muestra la primera sin commitear hasta que
                // el usuario elija (paridad SPA: sin onChange no hay commit).
                // El listener se conecta DESPUÉS de fijar la selección.
                let current = value_to_text(value);
                let initial = options
                    .iter()
                    .position(|option| *option == current)
                    .map(|index| index as u32)
                    .unwrap_or(0);
                dropdown.set_selected(if options.is_empty() { u32::MAX } else { initial });
                {
                    let commit = Rc::clone(&commit);
                    let options = spec.options.clone();
                    dropdown.connect_selected_notify(move |dropdown| {
                        let index = dropdown.selected();
                        if let Some(option) = options.get(index as usize) {
                            commit(Value::Str(option.clone()));
                        }
                    });
                }
                field.append(&dropdown);
            }
            _ => {
                let entry = gtk::Entry::new();
                entry.set_text(&value_to_text(value));
                entry.set_hexpand(true);
                if let (Some(min), Some(max)) = (spec.min, spec.max) {
                    entry.set_tooltip_text(Some(&format!("min: {min}, max: {max}")));
                } else if let Some(min) = spec.min {
                    entry.set_tooltip_text(Some(&format!("min: {min}")));
                } else if let Some(max) = spec.max {
                    entry.set_tooltip_text(Some(&format!("max: {max}")));
                }
                // Número/texto: commit al perder el foco (§ del enunciado) o
                // con Enter. Con `min`/`max` declarados el valor se acota al
                // commitear (P2-16: el widget nunca emite fuera de rango y
                // muestra lo realmente guardado); la autoridad sobre lo
                // válido siguen siendo los blockers de validación.
                let numeric = matches!(spec.kind, FieldType::Integer | FieldType::Decimal);
                let bounds = if numeric { Some((spec.min, spec.max)) } else { None };
                let commit_entry: Rc<dyn Fn()> = {
                    let entry = entry.clone();
                    let commit = Rc::clone(&commit);
                    Rc::new(move || {
                        let text = entry.text().to_string();
                        let value = clamp_to_bounds(parse_entry(numeric, &text), bounds);
                        if bounds.is_some() {
                            let shown = value_to_text(&value);
                            if shown != text {
                                entry.set_text(&shown);
                            }
                        }
                        commit(value);
                    })
                };
                {
                    let commit_entry = Rc::clone(&commit_entry);
                    entry.connect_activate(move |_| commit_entry());
                }
                {
                    let focus = gtk::EventControllerFocus::new();
                    focus.connect_leave(move |_| commit_entry());
                    entry.add_controller(focus);
                }
                field.append(&entry);
            }
        }

        let issue_label = gtk::Label::new(None);
        issue_label.set_halign(gtk::Align::Start);
        issue_label.set_xalign(0.0);
        issue_label.set_wrap(true);
        issue_label.add_css_class("opm-field-issue");
        set_issue_label(&issue_label, issue);
        field.append(&issue_label);
        self.field_issues
            .borrow_mut()
            .push((step_id.to_owned(), spec.key.clone(), issue_label));

        field
    }
}

/// Fila `Etiqueta <code valor>` de la meta.
fn meta_row(name: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let label = gtk::Label::new(Some(name));
    label.set_halign(gtk::Align::Start);
    label.set_size_request(110, -1);
    label.add_css_class("opm-dim");
    let value_label = gtk::Label::new(Some(value));
    value_label.set_halign(gtk::Align::Start);
    value_label.set_xalign(0.0);
    value_label.set_hexpand(true);
    value_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    value_label.set_tooltip_text(Some(value));
    value_label.add_css_class("opm-mono");
    row.append(&label);
    row.append(&value_label);
    row
}

/// Título de sección (h3 del SPA).
fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    label.add_css_class("opm-section-title");
    label
}

/// «None» / «Clear» de las secciones vacías.
fn empty_label() -> gtk::Label {
    let label = gtk::Label::new(Some("None"));
    label.set_halign(gtk::Align::Start);
    label.add_css_class("opm-empty");
    label
}

/// Pastilla de estado: texto + clase CSS por estado (§6: pending gray,
/// running amber, ok green, error red, cancelled slate; `scheduled` comparte
/// gris con pending).
fn set_pill(label: &gtk::Label, status: &opm_store::StepStatus) {
    use opm_store::StepStatus;
    let (text, class) = match status {
        StepStatus::Pending => ("pending", "opm-pill-pending"),
        StepStatus::Scheduled => ("scheduled", "opm-pill-pending"),
        StepStatus::Running => ("running", "opm-pill-running"),
        StepStatus::Ok => ("ok", "opm-pill-ok"),
        StepStatus::Error => ("error", "opm-pill-error"),
        StepStatus::Cancelled => ("cancelled", "opm-pill-cancelled"),
    };
    for old in [
        "opm-pill-pending",
        "opm-pill-running",
        "opm-pill-ok",
        "opm-pill-error",
        "opm-pill-cancelled",
    ] {
        label.remove_css_class(old);
    }
    label.add_css_class("opm-pill");
    label.add_css_class(class);
    label.set_text(text);
}

/// Issue con `field == key` (paridad SPA: `issues.find(...)`).
fn issue_for<'a>(
    issues: &'a [ValidationIssue],
    key: &str,
) -> Option<&'a ValidationIssue> {
    issues.iter().find(|issue| issue.field.as_deref() == Some(key))
}

fn set_issue_label(label: &gtk::Label, issue: Option<&ValidationIssue>) {
    match issue {
        Some(issue) => {
            label.set_text(&issue.message);
            let class = match issue.severity {
                opm_pipeline::IssueSeverity::Blocker => "opm-issue-blocker",
                opm_pipeline::IssueSeverity::Warning => "opm-issue-warning",
            };
            label.remove_css_class("opm-issue-blocker");
            label.remove_css_class("opm-issue-warning");
            label.add_css_class(class);
            label.set_visible(true);
        }
        None => {
            label.set_text("");
            label.set_visible(false);
        }
    }
}

/// Sección Validation: «Clear» o las filas de issue (port de `Issue`).
fn fill_validation(box_: &gtk::Box, issues: &[ValidationIssue]) {
    while let Some(child) = box_.first_child() {
        box_.remove(&child);
    }
    if issues.is_empty() {
        let label = gtk::Label::new(Some("Clear"));
        label.set_halign(gtk::Align::Start);
        label.add_css_class("opm-empty");
        box_.append(&label);
        return;
    }
    for issue in issues {
        let label = gtk::Label::new(Some(&issue.message));
        label.set_halign(gtk::Align::Start);
        label.set_xalign(0.0);
        label.set_wrap(true);
        label.add_css_class(match issue.severity {
            opm_pipeline::IssueSeverity::Blocker => "opm-issue-blocker",
            opm_pipeline::IssueSeverity::Warning => "opm-issue-warning",
        });
        box_.append(&label);
    }
}

/// Error inline `state.message` (oculto cuando está vacío).
fn set_message(label: &gtk::Label, message: &str) {
    if message.is_empty() {
        label.set_visible(false);
        label.set_text("");
    } else {
        label.set_text(message);
        label.set_visible(true);
    }
}

/// Valor del modelo → texto mostrado en el editor (paridad SPA `String(v)`).
pub fn value_to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => {
            if value.fract() == 0.0 && value.abs() < 1e15 {
                format!("{}", *value as i64)
            } else {
                value.to_string()
            }
        }
        Value::Str(text) => text.clone(),
        Value::Seq(items) => {
            let rendered: Vec<String> = items.iter().map(value_to_text).collect();
            format!("[{}]", rendered.join(", "))
        }
        Value::Map(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(key, value)| format!("{key}: {}", value_to_text(value)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
    }
}

/// Texto de entrada → valor commiteado (paridad con el `onBlur` del SPA):
/// número → parse `f64` (`""` → `0`; no finito → `Null`); resto → `Str`.
/// En `integer`, los enteros exactos se guardan como `Int`.
pub fn parse_entry(numeric: bool, text: &str) -> Value {
    if !numeric {
        return Value::Str(text.to_owned());
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Value::Int(0);
    }
    match trimmed.parse::<f64>() {
        Ok(number) if number.is_finite() => {
            if number.fract() == 0.0 && number.abs() < 9e15 {
                Value::Int(number as i64)
            } else {
                Value::Float(number)
            }
        }
        _ => Value::Null,
    }
}

/// P2-16: acota un valor numérico a las cotas `min`/`max` que el spec
/// declara (`bounds = None` sin cotas → sin cambios). `Null` (no parseable)
/// pasa tal cual — su caso lo decide la validación — y unas cotas
/// enteras contradictorias (`ceil(min) > floor(max)`) no acotan: la
/// autoridad sobre lo válido son los blockers de validación, no el widget.
pub fn clamp_to_bounds(value: Value, bounds: Option<(Option<f64>, Option<f64>)>) -> Value {
    let Some((min, max)) = bounds else {
        return value;
    };
    match value {
        Value::Int(number) => {
            let low = min.map(f64::ceil);
            let high = max.map(f64::floor);
            if matches!((low, high), (Some(low), Some(high)) if low > high) {
                return Value::Int(number);
            }
            let mut number = number;
            if let Some(low) = low {
                if (number as f64) < low {
                    number = low as i64;
                }
            }
            if let Some(high) = high {
                if (number as f64) > high {
                    number = high as i64;
                }
            }
            Value::Int(number)
        }
        Value::Float(number) => {
            let low = min.unwrap_or(f64::NEG_INFINITY);
            let high = max.unwrap_or(f64::INFINITY);
            if low > high {
                return Value::Float(number);
            }
            Value::Float(number.clamp(low, high))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valor mostrado: `params[key] ?? default ?? ''` (el caller resuelve el
    /// `??`); aquí la traducción a texto del SPA.
    #[test]
    fn valor_a_texto_como_el_spa() {
        assert_eq!(value_to_text(&Value::Null), "");
        assert_eq!(value_to_text(&Value::Bool(true)), "true");
        assert_eq!(value_to_text(&Value::Int(42)), "42");
        assert_eq!(value_to_text(&Value::Float(2.5)), "2.5");
        assert_eq!(value_to_text(&Value::Float(3.0)), "3");
        assert_eq!(value_to_text(&Value::Str("hola".into())), "hola");
        assert_eq!(value_to_text(&Value::Seq(vec![Value::Int(1)])), "[1]");
    }

    /// Commit numérico en blur (paridad `onBlur`): `""` → 0, entero exacto
    /// → `Int`, decimal → `Float`, no finito/no parseable → `Null`.
    #[test]
    fn parse_de_entradas_numericas() {
        assert_eq!(parse_entry(true, ""), Value::Int(0));
        assert_eq!(parse_entry(true, "  7 "), Value::Int(7));
        assert_eq!(parse_entry(true, "3.5"), Value::Float(3.5));
        assert_eq!(parse_entry(true, "NaN"), Value::Null);
        assert_eq!(parse_entry(true, "Infinity"), Value::Null);
        assert_eq!(parse_entry(true, "abc"), Value::Null);
        // Texto: se guarda tal cual (espacios incluidos).
        assert_eq!(parse_entry(false, " x "), Value::Str(" x ".into()));
    }

    /// P2-16: acotación de `min`/`max` al commitear (enteros con cotas
    /// enteras exactas; `Null` y cotas contradictorias intactos: la
    /// validación es la autoridad).
    #[test]
    fn acotacion_de_cotas_min_max() {
        let bounds = Some((Some(0.0), Some(10.0)));
        assert_eq!(clamp_to_bounds(Value::Int(42), bounds), Value::Int(10));
        assert_eq!(clamp_to_bounds(Value::Int(-3), bounds), Value::Int(0));
        assert_eq!(clamp_to_bounds(Value::Int(7), bounds), Value::Int(7));
        assert_eq!(
            clamp_to_bounds(Value::Float(10.5), bounds),
            Value::Float(10.0)
        );
        // Solo `max` (floor) y solo `min` (ceil).
        assert_eq!(
            clamp_to_bounds(Value::Int(3), Some((None, Some(2.5)))),
            Value::Int(2)
        );
        assert_eq!(
            clamp_to_bounds(Value::Float(0.25), Some((Some(0.5), None))),
            Value::Float(0.5)
        );
        // Sin cotas y valores no numéricos: sin cambios.
        assert_eq!(clamp_to_bounds(Value::Int(99), None), Value::Int(99));
        assert_eq!(clamp_to_bounds(Value::Null, bounds), Value::Null);
        // Cotas enteras contradictorias (ceil 2 > floor 1): no se acota.
        assert_eq!(
            clamp_to_bounds(Value::Int(3), Some((Some(1.5), Some(1.7)))),
            Value::Int(3)
        );
    }

    /// El panel vacío y el montaje completo (requiere sesión gráfica).
    #[test]
    #[ignore = "requiere sesión gráfica GTK"]
    fn panel_de_paso_se_construye() {
        assert!(gtk::init().is_ok());
        let panel = StepPanel::new(StepCallbacks {
            on_edit: Rc::new(|_step: PipelineStep| {}),
            on_run: Rc::new(|_id: String| {}),
            on_param: Rc::new(|_step: String, _key: String, _value: Value| {}),
        });
        panel.select(None);
        assert_eq!(panel.current_step(), None);
        let detail = StepDetail {
            step: PipelineStep {
                id: "s1".into(),
                name: "Step 1".into(),
                description: String::new(),
                command: "echo hi".into(),
                environment: String::new(),
                working_directory: ".".into(),
                inputs: vec![],
                outputs: vec![],
                parameters: vec![],
                dependencies: vec![],
            },
            params: Default::default(),
            state: Default::default(),
            validation: vec![],
        };
        panel.select(Some(&detail));
        assert_eq!(panel.current_step().as_deref(), Some("s1"));
        panel.set_running(true);
        panel.set_running(false);
        panel.refresh(Some(&detail));
        panel.select(None);
    }
}
