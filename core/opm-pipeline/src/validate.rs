//! Validation — port of `backend/app/services/validation.py`.
//!
//! Every rule reproduces the Python severity, message text and `field`
//! value pinned by `backend/tests/test_validation.py`. Path rules run
//! through the shared `opm-fs` helpers (`resolve_pipeline_path`,
//! `relative_project_path`), which port `pipeline_paths.py` verbatim.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::model::{
    FieldType, IssueSeverity, OutputSpec, PipelineConfig, PipelineStep, ValidationIssue, ValueSpec,
};
use crate::pyfmt::{py_float, py_str};
use crate::value::Value;

/// Names referenced by `{...}` placeholders in `text`
/// (regex `{([A-Za-z_][A-Za-z0-9_-]*)}`), unique, in order of first
/// appearance.
pub fn placeholders(text: &str) -> Vec<String> {
    placeholder_names(text)
}

/// Effective values of a step: defaults, overridden by submitted params,
/// overridden by output paths (params can never override outputs).
pub fn effective_values(
    step: &PipelineStep,
    params: &BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    let mut values = step.default_values();
    for (key, value) in params {
        values.insert(key.clone(), value.clone());
    }
    for (key, value) in step.output_values() {
        values.insert(key.clone(), value.clone());
    }
    values
}

/// Port of `ValidationService.validate_pipeline` (the `ok_checker` parameter
/// of the Python method is dead and is not carried over).
pub fn validate_pipeline(
    pipeline: &PipelineConfig,
    project: &Path,
    params: &HashMap<String, BTreeMap<String, Value>>,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    // Duplicate step ids (one blocker per duplicated id, first-seen order).
    let mut order: Vec<&str> = Vec::new();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for step in &pipeline.steps {
        let count = counts.entry(step.id.as_str()).or_insert(0);
        *count += 1;
        if *count == 1 {
            order.push(step.id.as_str());
        }
    }
    for id in order {
        if counts[id] > 1 {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Blocker,
                message: format!("Duplicate step id: {}", id),
                step_id: Some(id.to_owned()),
                field: Some("id".to_owned()),
            });
        }
    }

    // F3: ids con separadores de ruta o NUL — imposibles de crear vía la UI
    // (`normalize_step_id` → `safe_segment`), pero sí llegables en un YAML
    // importado o un snapshot manipulado: bloqueo visible que nombra el paso,
    // antes de que ningún artefactual (log/done) o workspace use el id.
    for step in &pipeline.steps {
        if step.id.contains('\0') {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                "id",
                format!("Step id contains an invalid null byte: {}", step.id),
            ));
        } else if step.id.contains('/') || step.id.contains('\\') {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                "id",
                format!("Step id contains a path separator: {}", step.id),
            ));
        }
    }

    // Id -> step, LAST occurrence wins (Python `step_by_id` dict semantics).
    let mut step_map: HashMap<&str, &PipelineStep> = HashMap::new();
    for step in &pipeline.steps {
        step_map.insert(step.id.as_str(), step);
    }

    issues.extend(detect_cycles(pipeline, &step_map));

    for step in &pipeline.steps {
        issues.extend(source_ref_issues(step, &step_map));
        let (mut step_params, source_issues) = source_input_values(pipeline, step, project);
        issues.extend(source_issues);
        if let Some(submitted) = params.get(step.id.as_str()) {
            for (key, value) in submitted {
                step_params.insert(key.clone(), value.clone());
            }
        }
        issues.extend(validate_step(step, project, &step_params));

        for dependency in &step.dependencies {
            if !step_map.contains_key(dependency.as_str()) {
                issues.push(step_issue(
                    IssueSeverity::Blocker,
                    step,
                    "dependencies",
                    format!("Dependency does not exist: {}", dependency),
                ));
            }
        }
    }

    issues
}

fn validate_step(
    step: &PipelineStep,
    project: &Path,
    step_params: &BTreeMap<String, Value>,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let values = effective_values(step, step_params);

    // Rules: unsafe working directory / missing working directory.
    let working_directory = match opm_fs::resolve_pipeline_path(project, &step.working_directory, project) {
        Ok(resolved) => Some(resolved),
        Err(exc) => {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                "working_directory",
                format!("Unsafe working directory: {}", exc),
            ));
            None
        }
    };
    if let Some(resolved) = &working_directory {
        if !resolved.exists() {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                "working_directory",
                format!("Working directory does not exist: {}", step.working_directory),
            ));
        }
    }
    // `step_base = working_directory or project_path.resolve()`; opm-fs
    // resolves the base again internally, exactly like Python's `_resolve_path`.
    let step_base = working_directory.unwrap_or_else(|| project.to_path_buf());

    issues.extend(declared_key_issues(step));

    for spec in &step.inputs {
        let value = values.get(&spec.key);
        if matches!(spec.kind, FieldType::File | FieldType::Folder) {
            if has_value(value) {
                let raw = py_str(value.expect("has_value guarantees Some"));
                match opm_fs::resolve_pipeline_path(project, &raw, &step_base) {
                    Err(exc) => {
                        issues.push(step_issue(
                            IssueSeverity::Blocker,
                            step,
                            &spec.key,
                            format!("Unsafe input path: {}", exc),
                        ));
                        continue;
                    }
                    Ok(candidate) => {
                        // Source-linked inputs skip the existence check.
                        if non_empty(&spec.source_step).is_some()
                            && non_empty(&spec.source_output).is_some()
                        {
                            continue;
                        }
                        let exists = candidate.exists();
                        let type_matches = if spec.kind == FieldType::Folder {
                            candidate.is_dir()
                        } else {
                            candidate.is_file()
                        };
                        if !exists || !type_matches {
                            issues.push(step_issue(
                                if spec.required {
                                    IssueSeverity::Blocker
                                } else {
                                    IssueSeverity::Warning
                                },
                                step,
                                &spec.key,
                                format!("{} does not exist: {}", label_or_key(spec), raw),
                            ));
                        }
                    }
                }
            } else if spec.required {
                issues.push(step_issue(
                    IssueSeverity::Blocker,
                    step,
                    &spec.key,
                    format!("Required input is missing: {}", label_or_key(spec)),
                ));
            } else {
                issues.push(step_issue(
                    IssueSeverity::Warning,
                    step,
                    &spec.key,
                    format!("Optional input is not set: {}", label_or_key(spec)),
                ));
            }
        } else {
            issues.extend(value_spec_issues(step, spec, value, "input"));
        }
    }

    for output in &step.outputs {
        if output.path.trim().is_empty() {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                field_or_outputs(output),
                format!("Output path is missing: {}", key_or_output(output)),
            ));
            continue;
        }
        if let Err(exc) = opm_fs::resolve_pipeline_path(project, &output.path, &step_base) {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                field_or_outputs(output),
                format!("Unsafe output path: {}", exc),
            ));
        }
    }

    for spec in &step.parameters {
        issues.extend(value_spec_issues(step, spec, values.get(&spec.key), "parameter"));
    }

    // F1 (extensión de la regla de `command`): un placeholder dentro de un
    // contexto de shell que la sustitución no cotiza (comentario `#`,
    // `$()`/`${...}`/backticks o cuerpo de heredoc) es bloqueo — el motor lo
    // rechazaría igualmente antes de ejecutar (`ExecError::UnsupportedContext`).
    if let Some(name) = crate::unsupported_context_placeholder(&step.command) {
        issues.push(step_issue(
            IssueSeverity::Blocker,
            step,
            "command",
            format!(
                "Placeholder sits in a shell context OPM cannot substitute safely (comment, $() or heredoc): {}",
                name
            ),
        ));
    }

    // Placeholders in the command must be declared and must have a value.
    let mut declared: HashSet<&str> = HashSet::new();
    for spec in step.inputs.iter().chain(step.parameters.iter()) {
        declared.insert(spec.key.as_str());
    }
    for output in &step.outputs {
        if !output.key.is_empty() {
            declared.insert(output.key.as_str());
        }
    }
    let mut names = placeholder_names(&step.command);
    names.sort();
    for name in names {
        if !declared.contains(name.as_str()) {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                "command",
                format!("Placeholder has no input or parameter: {}", name),
            ));
        } else if !has_value(values.get(&name)) {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                &name,
                format!("Placeholder has no value: {}", name),
            ));
        }
    }

    issues
}

/// Rules for input/parameter keys and cross-declaration key uniqueness
/// (`_validate_declared_keys`).
fn declared_key_issues(step: &PipelineStep) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    for spec in step.inputs.iter().chain(step.parameters.iter()) {
        if spec.key.trim().is_empty() {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                "key",
                "Input or parameter key cannot be empty or whitespace-only",
            ));
        }
    }
    for output in &step.outputs {
        if !output.key.is_empty() && output.key.trim().is_empty() {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                "key",
                "Output key cannot be empty or whitespace-only when provided",
            ));
        }
    }

    // One entry per non-blank key across inputs + parameters + outputs,
    // first-seen order (Python `Counter` insertion order).
    let mut counts: Vec<(&str, usize)> = Vec::new();
    let keys = step
        .inputs
        .iter()
        .map(|spec| spec.key.as_str())
        .chain(step.parameters.iter().map(|spec| spec.key.as_str()))
        .chain(step.outputs.iter().map(|output| output.key.as_str()));
    for key in keys {
        if key.trim().is_empty() {
            continue;
        }
        match counts.iter_mut().find(|(seen, _)| *seen == key) {
            Some((_, count)) => *count += 1,
            None => counts.push((key, 1)),
        }
    }
    for (key, count) in counts {
        if count > 1 {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                key,
                format!("Duplicate declared key: {}", key),
            ));
        }
    }

    issues
}

/// Value rules shared by non-file inputs and all parameters
/// (`_validate_value_spec`). `kind` is `"input"` or `"parameter"`.
fn value_spec_issues(
    step: &PipelineStep,
    spec: &ValueSpec,
    value: Option<&Value>,
    kind: &str,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let label = label_or_key(spec);

    let min_finite = spec.min.map_or(true, f64::is_finite);
    let max_finite = spec.max.map_or(true, f64::is_finite);
    if !min_finite {
        issues.push(step_issue(
            IssueSeverity::Blocker,
            step,
            &spec.key,
            format!("Minimum must be finite: {}", label),
        ));
    }
    if !max_finite {
        issues.push(step_issue(
            IssueSeverity::Blocker,
            step,
            &spec.key,
            format!("Maximum must be finite: {}", label),
        ));
    }
    if let (Some(min), Some(max)) = (spec.min, spec.max) {
        if min_finite && max_finite && min > max {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                &spec.key,
                format!("Minimum cannot be greater than maximum: {}", label),
            ));
        }
    }

    if spec.required && !has_value(value) {
        issues.push(step_issue(
            IssueSeverity::Blocker,
            step,
            &spec.key,
            format!("Required {} is missing: {}", kind, label),
        ));
        return issues;
    }
    let Some(value) = value else {
        return issues;
    };
    if !has_value(Some(value)) {
        return issues;
    }

    match spec.kind {
        FieldType::Integer | FieldType::Decimal => {
            if matches!(value, Value::Bool(_)) {
                issues.push(step_issue(
                    IssueSeverity::Blocker,
                    step,
                    &spec.key,
                    format!("Value must be numeric: {}", label),
                ));
                return issues;
            }
            let numeric = match value {
                Value::Int(int) => *int as f64,
                Value::Float(float) => *float,
                Value::Str(text) => match parse_python_float(text) {
                    Some(numeric) => numeric,
                    None => {
                        issues.push(step_issue(
                            IssueSeverity::Blocker,
                            step,
                            &spec.key,
                            format!("Value must be numeric: {}", label),
                        ));
                        return issues;
                    }
                },
                _ => {
                    issues.push(step_issue(
                        IssueSeverity::Blocker,
                        step,
                        &spec.key,
                        format!("Value must be numeric: {}", label),
                    ));
                    return issues;
                }
            };
            if !numeric.is_finite() {
                issues.push(step_issue(
                    IssueSeverity::Blocker,
                    step,
                    &spec.key,
                    format!("Value must be finite: {}", label),
                ));
                return issues;
            }
            if spec.kind == FieldType::Integer && numeric.fract() != 0.0 {
                issues.push(step_issue(
                    IssueSeverity::Blocker,
                    step,
                    &spec.key,
                    format!("Value must be an integer: {}", label),
                ));
            }
            if min_finite {
                if let Some(min) = spec.min {
                    if numeric < min {
                        issues.push(step_issue(
                            IssueSeverity::Blocker,
                            step,
                            &spec.key,
                            format!("Value is below minimum {}: {}", py_float(min), label),
                        ));
                    }
                }
            }
            if max_finite {
                if let Some(max) = spec.max {
                    if numeric > max {
                        issues.push(step_issue(
                            IssueSeverity::Blocker,
                            step,
                            &spec.key,
                            format!("Value is above maximum {}: {}", py_float(max), label),
                        ));
                    }
                }
            }
        }
        FieldType::Select => {
            if !spec.options.is_empty() {
                let in_options = match value {
                    Value::Str(text) => spec.options.iter().any(|option| option == text),
                    _ => false,
                };
                if !in_options {
                    issues.push(step_issue(
                        IssueSeverity::Blocker,
                        step,
                        &spec.key,
                        format!("Invalid option for {}: {}", label, py_str(value)),
                    ));
                }
            }
        }
        FieldType::Boolean => {
            // DECISIONS.md strict booleans: only native true/false.
            if !matches!(value, Value::Bool(_)) {
                issues.push(step_issue(
                    IssueSeverity::Blocker,
                    step,
                    &spec.key,
                    format!("Value must be a boolean: {}", label),
                ));
            }
        }
        FieldType::Text | FieldType::File | FieldType::Folder => {}
    }

    issues
}

/// Source-link rules (`_validate_source_refs`): both ends present, source
/// step and output resolvable.
fn source_ref_issues(step: &PipelineStep, step_map: &HashMap<&str, &PipelineStep>) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    for spec in &step.inputs {
        let source_step = non_empty(&spec.source_step);
        let source_output = non_empty(&spec.source_output);
        if source_step.is_none() && source_output.is_none() {
            continue;
        }
        let (Some(source_step), Some(source_output)) = (source_step, source_output) else {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                &spec.key,
                format!(
                    "Input source must include source_step and source_output: {}",
                    spec.key
                ),
            ));
            continue;
        };
        let Some(source) = step_map.get(source_step).copied() else {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                &spec.key,
                format!("Input source step does not exist: {}", source_step),
            ));
            continue;
        };
        if find_source_output(source, source_output).is_none() {
            issues.push(step_issue(
                IssueSeverity::Blocker,
                step,
                &spec.key,
                format!("Input source output does not exist: {}", source_output),
            ));
        }
    }
    issues
}

/// Resolved project-relative values for source-linked inputs
/// (`_source_input_values`); unsafe chains become blockers.
fn source_input_values(
    pipeline: &PipelineConfig,
    step: &PipelineStep,
    project: &Path,
) -> (BTreeMap<String, Value>, Vec<ValidationIssue>) {
    let mut values = BTreeMap::new();
    let mut issues = Vec::new();

    for spec in &step.inputs {
        let (Some(source_step_id), Some(source_output_id)) =
            (non_empty(&spec.source_step), non_empty(&spec.source_output))
        else {
            continue;
        };
        let Some(source_step) = pipeline.step_by_id(source_step_id) else {
            continue;
        };
        let Some(source_output) = find_source_output(source_step, source_output_id) else {
            continue;
        };

        // Python: `relative_project_path(project, step.working_directory,
        // source_step.working_directory, source_output.path)` — same chain in
        // opm-fs helpers.
        let resolved = (|| {
            let target = opm_fs::resolve_pipeline_path(project, &step.working_directory, project)?;
            let source_base =
                opm_fs::resolve_pipeline_path(project, &source_step.working_directory, project)?;
            let source = opm_fs::resolve_pipeline_path(project, &source_output.path, &source_base)?;
            opm_fs::relative_project_path(project, &target, &source)
        })();
        match resolved {
            Ok(relative) => {
                values.insert(spec.key.clone(), Value::Str(relative));
            }
            Err(exc) => {
                issues.push(step_issue(
                    IssueSeverity::Blocker,
                    step,
                    &spec.key,
                    format!("Unsafe input source path: {}", exc),
                ));
            }
        }
    }

    (values, issues)
}

/// Dependency cycles (`_detect_cycles`): first re-entry into a visiting node
/// reports `Dependency cycle detected: a -> b -> a`.
///
/// F8: DFS iterativo — la pila mutable ES el trail (una única copia lineal,
/// sin trail por nivel): una cadena de miles de pasos desde un fichero sin
/// validar no desborda la pila ni memoriza cuadráticamente. La ruta del
/// mensaje es la raíz del DFS actual hasta el nodo reentrante, idéntica a la
/// de la versión recursiva.
fn detect_cycles(
    pipeline: &PipelineConfig,
    step_map: &HashMap<&str, &PipelineStep>,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut visiting: HashSet<&str> = HashSet::new();
    let mut visited: HashSet<&str> = HashSet::new();
    // Pila: (id del nodo en el camino, índice de su siguiente dependencia).
    let mut stack: Vec<(&str, usize)> = Vec::new();

    for step in &pipeline.steps {
        let root = step.id.as_str();
        if visited.contains(root) {
            continue;
        }
        stack.push((root, 0));
        visiting.insert(root);
        loop {
            let Some(&(id, next)) = stack.last() else {
                break;
            };
            let Some(current) = step_map.get(id).copied() else {
                // Inalcanzable: solo entran nodos presentes en el mapa (los
                // ids desconocidos se filtran antes de apilar, abajo).
                stack.pop();
                visiting.remove(id);
                continue;
            };
            if next >= current.dependencies.len() {
                stack.pop();
                visiting.remove(id);
                visited.insert(id);
                continue;
            }
            stack.last_mut().expect("node on the path").1 = next + 1;
            let dependency = current.dependencies[next].as_str();
            if visiting.contains(dependency) {
                // Reentrada: el trail ES la pila (raíz → actual) + el id.
                let mut cycle: Vec<&str> = stack.iter().map(|(node, _)| *node).collect();
                cycle.push(dependency);
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Blocker,
                    message: format!("Dependency cycle detected: {}", cycle.join(" -> ")),
                    step_id: Some(dependency.to_owned()),
                    field: Some("dependencies".to_owned()),
                });
                continue;
            }
            if visited.contains(dependency) {
                continue;
            }
            if step_map.get(dependency).is_none() {
                // Dependencia inexistente: otra regla ya la reporta.
                continue;
            }
            stack.push((dependency, 0));
            visiting.insert(dependency);
        }
    }
    issues
}

/// `has_value` (Python): not None, not `""`, not `[]`. An empty object or
/// `false`/`0` counts as present.
fn has_value(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Str(text)) => !text.is_empty(),
        Some(Value::Seq(items)) => !items.is_empty(),
        Some(_) => true,
    }
}

/// Python truthiness of an optional string field: only `None` and `""` are
/// falsy (a whitespace-only value is still truthy).
fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|text| !text.is_empty())
}

fn label_or_key(spec: &ValueSpec) -> &str {
    spec.label
        .as_deref()
        .filter(|label| !label.is_empty())
        .unwrap_or(&spec.key)
}

fn field_or_outputs(output: &OutputSpec) -> &str {
    if output.key.is_empty() {
        "outputs"
    } else {
        &output.key
    }
}

fn key_or_output(output: &OutputSpec) -> &str {
    if output.key.is_empty() {
        "output"
    } else {
        &output.key
    }
}

/// Output matched by key or by path (`output.key == ref or output.path == ref`).
fn find_source_output<'a>(source_step: &'a PipelineStep, reference: &str) -> Option<&'a OutputSpec> {
    source_step
        .outputs
        .iter()
        .find(|output| output.key == reference || output.path == reference)
}

/// `float(value)` semantics for submitted strings: Python strips surrounding
/// whitespace and accepts `nan`/`inf`/`5.`/`.5` (all of which Rust's parser
/// also accepts after the trim).
fn parse_python_float(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok()
}

/// Unique `{name}` placeholders in order of first appearance (regex
/// `{([A-Za-z_][A-Za-z0-9_-]*)}`).
fn placeholder_names(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut names: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() {
            let first = bytes[i + 1];
            if first.is_ascii_alphabetic() || first == b'_' {
                let mut j = i + 2;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'-')
                {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'}' {
                    let name = &text[i + 1..j];
                    if !names.iter().any(|existing| existing == name) {
                        names.push(name.to_owned());
                    }
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    names
}

fn step_issue(
    severity: IssueSeverity,
    step: &PipelineStep,
    field: impl Into<String>,
    message: impl Into<String>,
) -> ValidationIssue {
    ValidationIssue {
        severity,
        message: message.into(),
        step_id: Some(step.id.clone()),
        field: Some(field.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::placeholder_names;

    #[test]
    fn placeholder_regex_shape() {
        assert_eq!(
            placeholder_names("echo {a} {b_2} {c-d} {_x}"),
            vec!["a", "b_2", "c-d", "_x"]
        );
    }

    #[test]
    fn placeholders_are_unique_and_ordered() {
        assert_eq!(
            placeholder_names("{z} {a} {z}"),
            vec!["z", "a"]
        );
    }

    #[test]
    fn rejects_non_placeholders() {
        // Digits cannot start a name; braces must close; no nested openers.
        assert!(placeholder_names("echo {1x}").is_empty());
        assert!(placeholder_names("echo {unclosed").is_empty());
        assert!(placeholder_names("echo {}").is_empty());
        assert_eq!(placeholder_names("echo {{a}}"), vec!["a"]);
        assert_eq!(placeholder_names("awk '{print $1}'"), Vec::<String>::new());
        assert_eq!(placeholder_names("{a{b}"), vec!["b"]);
    }
}
