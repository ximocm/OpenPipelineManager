//! Command building — port of `execution.py`'s `build_command`,
//! `_substitute_placeholders`, `_quote_placeholder_value`,
//! `_apply_environment` and `shell_invocation`/`find_posix_shell`.

use std::collections::BTreeMap;
use std::ffi::OsString;

use opm_pipeline::{
    effective_values, placeholders, PipelineStep, ScanStep, ShellQuote as QuoteContext,
    ShellScanner, Value,
};

use crate::ExecError;

/// `ExecutionManager.build_command`: effective values, missing-placeholder
/// check (error names the keys), quote-context-aware substitution, then the
/// environment wrapper.
pub(crate) fn build_command(
    step: &PipelineStep,
    params: &BTreeMap<String, Value>,
) -> Result<String, ExecError> {
    let values = effective_values(step, params);
    let mut keys = placeholders(&step.command);
    keys.sort();
    keys.dedup();
    let missing: Vec<String> = keys
        .into_iter()
        .filter(|key| !has_value(values.get(key)))
        .collect();
    if !missing.is_empty() {
        return Err(ExecError::MissingPlaceholders(missing));
    }
    let substituted = substitute_placeholders(step.command.as_str(), &values)?;
    Ok(apply_environment(step.environment.as_str(), &substituted))
}

/// Python `has_value`: not None, not `""`, not `[]`.
fn has_value(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Str(text)) => !text.is_empty(),
        Some(Value::Seq(items)) => !items.is_empty(),
        Some(_) => true,
    }
}

/// Port of `_substitute_placeholders`, now driven by the shared
/// [`ShellScanner`] state machine (F1): comments, `$(...)`/`${...}`/
/// backticks and heredoc bodies neither corrupt the quote state of the
/// surrounding command nor get substituted — a placeholder found in one of
/// them returns [`ExecError::UnsupportedContext`] instead. Every ordinary
/// quoted/unquoted context keeps the exact Python quoting below.
pub(crate) fn substitute_placeholders(
    command: &str,
    values: &BTreeMap<String, Value>,
) -> Result<String, ExecError> {
    let chars: Vec<char> = command.chars().collect();
    let mut out = String::with_capacity(command.len());
    let mut scanner = ShellScanner::new(command);
    let mut cursor = 0;
    while let Some(step) = scanner.next_step() {
        match step {
            ScanStep::Copy(len) => {
                out.extend(chars[cursor..cursor + len].iter());
                cursor += len;
            }
            ScanStep::Substitute {
                name,
                len,
                quote,
            } => {
                cursor += len;
                let value = values.get(&name).unwrap_or(&Value::Null);
                out.push_str(&quote_value(&python_str(value), quote));
            }
            ScanStep::Unsupported { name } => {
                return Err(ExecError::UnsupportedContext { placeholder: name });
            }
        }
    }
    Ok(out)
}

/// `_quote_placeholder_value`.
fn quote_value(value: &str, context: QuoteContext) -> String {
    match context {
        QuoteContext::Single => value.replace('\'', "'\\''"),
        QuoteContext::Double => value
            .chars()
            .map(|character| match character {
                '$' | '`' | '"' | '\\' => format!("\\{character}"),
                other => other.to_string(),
            })
            .collect(),
        QuoteContext::Unquoted => shell_quote(value),
    }
}

/// Python `shlex.quote` (ASCII-safe characters stay bare, else single quotes
/// with the `'"'"'` escape dance).
pub(crate) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value.chars().all(is_shell_safe) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn is_shell_safe(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(
            character,
            '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
        )
}

/// Python `str(value)` for placeholder substitution (top-level values are
/// stringified, nested collection elements use repr-like formatting as in
/// `str(list)`/`str(dict)`).
pub(crate) fn python_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => {
            if number.is_nan() {
                "nan".to_string()
            } else {
                number.to_string()
            }
        }
        Value::Str(text) => text.clone(),
        Value::Seq(items) => format!(
            "[{}]",
            items
                .iter()
                .map(python_repr)
                .collect::<Vec<String>>()
                .join(", ")
        ),
        Value::Map(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(key, item)| format!("{}: {}", python_repr_str(key), python_repr(item)))
                .collect::<Vec<String>>()
                .join(", ")
        ),
    }
}

fn python_repr(value: &Value) -> String {
    match value {
        Value::Str(text) => python_repr_str(text),
        other => python_str(other),
    }
}

fn python_repr_str(text: &str) -> String {
    if text.contains('\'') && !text.contains('"') {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\'', "\\'"))
    }
}

/// Port of `ExecutionManager._apply_environment`.
pub(crate) fn apply_environment(environment: &str, command: &str) -> String {
    let environment = environment.trim();
    if environment.is_empty() {
        return command.to_string();
    }
    let (kind, has_separator, tail) = match environment.find(':') {
        Some(index) => (&environment[..index], true, &environment[index + 1..]),
        None => (environment, false, ""),
    };
    let env_value = if has_separator {
        tail.trim()
    } else {
        environment
    };
    if env_value.is_empty() {
        return command.to_string();
    }
    if has_separator && matches!(kind, "conda" | "mamba" | "micromamba") {
        return format!("{kind} run -n {} {command}", shell_quote(env_value));
    }
    if has_separator && kind == "module" {
        return format!("module load {} && {command}", shell_quote(env_value));
    }
    if has_separator && kind == "shell" {
        return format!("{env_value} && {command}");
    }
    format!("conda run -n {} {command}", shell_quote(environment))
}

/// The child argv: `/bin/sh -c` on POSIX (Python `shell=True`), git-bash
/// `bash -lc` on Windows (§11 A8: no shell at all → [`ExecError::NoPosixShell`]).
///
/// The `exec 2>&1;` prefix merges stderr into the stdout pipe inside the
/// child (the Python `stderr=STDOUT` behavior); only the ORIGINAL command
/// ever reaches `StepStarted` events and log headers.
pub(crate) fn shell_argv(command: &str) -> Result<Vec<OsString>, ExecError> {
    let wrapped = format!("exec 2>&1; {command}");
    #[cfg(unix)]
    {
        Ok(vec![
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(wrapped),
        ])
    }
    #[cfg(windows)]
    {
        let bash = find_posix_shell().ok_or(ExecError::NoPosixShell)?;
        Ok(vec![bash, OsString::from("-lc"), OsString::from(wrapped)])
    }
}

/// §11 A8 preflight: Windows must find a POSIX shell BEFORE any thread or
/// process starts (no `cmd.exe` fallback).
pub(crate) fn check_posix_shell() -> Result<(), ExecError> {
    #[cfg(windows)]
    {
        find_posix_shell().ok_or(ExecError::NoPosixShell)?;
    }
    Ok(())
}

/// `execution.py:find_posix_shell` (Windows only).
#[cfg(windows)]
fn find_posix_shell() -> Option<OsString> {
    if let Ok(path_var) = std::env::var("PATH") {
        for directory in std::env::split_paths(&path_var) {
            for name in ["bash.exe", "bash"] {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Some(candidate.into_os_string());
                }
            }
        }
    }
    for candidate in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ] {
        let path = std::path::Path::new(candidate);
        if path.exists() {
            return Some(path.as_os_str().to_os_string());
        }
    }
    None
}
