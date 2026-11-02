//! Python-compatible value formatting for validation messages.
//!
//! `validation.py` interpolates floats with `f"{x}"` (Python `repr` for
//! floats, `str` otherwise) and stringifies non-string values with `str()`.
//! The rules below reproduce those texts exactly for every value shape a
//! pipeline can carry.

use crate::value::Value;

/// Python `repr()` for `float`: shortest round-trip digits, fixed notation
/// when the decimal exponent is in `-4..=15`, scientific (`1e+16`, `1e-05`)
/// otherwise, always with a `.0` on integral fixed values.
pub(crate) fn py_float(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".to_owned() } else { "-inf".to_owned() };
    }
    // Rust's `{:e}` yields shortest-roundtrip digits as `<sign><d[.ddd]>e<exp>`.
    let sci = format!("{:e}", v);
    let (sign, rest) = match sci.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", sci.as_str()),
    };
    let (mantissa, exp) = rest.split_once('e').expect("{:e} always contains 'e'");
    let exp: i32 = exp.parse().expect("exponent parses");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let n = digits.len() as i32;

    let body = if exp < -4 || exp >= 16 {
        // Scientific: first digit, then the rest as a fraction, `e` with an
        // explicit sign and at least two exponent digits (always true in this
        // branch, since |exp| >= 5 or exp >= 16).
        let mut s = String::new();
        s.push_str(&digits[..1]);
        if n > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        s.push(if exp < 0 { '-' } else { '+' });
        s.push_str(&format!("{:02}", exp.unsigned_abs()));
        s
    } else {
        // Fixed: decimal point at position exp + 1 within `digits`.
        let point = exp + 1;
        let mut s = String::new();
        if point <= 0 {
            s.push_str("0.");
            for _ in 0..-point {
                s.push('0');
            }
            s.push_str(&digits);
        } else if point >= n {
            s.push_str(&digits);
            for _ in 0..(point - n) {
                s.push('0');
            }
            s.push_str(".0");
        } else {
            let split = point as usize;
            s.push_str(&digits[..split]);
            s.push('.');
            s.push_str(&digits[split..]);
        }
        s
    };
    format!("{}{}", sign, body)
}

/// Python `str()` for a pipeline value (also the `f"{value}"` shape used in
/// validation messages and `str(value)` used before path resolution).
pub(crate) fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => py_float(*f),
        Value::Str(s) => s.clone(),
        Value::Seq(items) => {
            let inner: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Map(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", py_repr(&Value::Str(k.clone())), py_repr(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// Python `repr()`: like `py_str` but strings are single-quoted and escaped
/// (list/dict interpolation uses element reprs).
fn py_repr(value: &Value) -> String {
    match value {
        Value::Str(s) => format!("'{}'", escape_py_string(s)),
        _ => py_str(value),
    }
}

fn escape_py_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;
    use std::collections::BTreeMap;

    /// Expected strings captured from Python 3.12 `repr`/`str`
    /// (ground truth probed against the reference implementation).
    #[test]
    fn py_float_matches_python_repr() {
        let cases: [(f64, &str); 24] = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (10.0, "10.0"),
            (1.0, "1.0"),
            (5.0, "5.0"),
            (0.25, "0.25"),
            (2.0, "2.0"),
            (0.1, "0.1"),
            (-3.5, "-3.5"),
            (123456789.5, "123456789.5"),
            (0.0001, "0.0001"),
            (1e-5, "1e-05"),
            (1e-7, "1e-07"),
            (1e15, "1000000000000000.0"),
            (999_999_999_999_9998.0, "9999999999999998.0"),
            (1e16, "1e+16"),
            (1e17, "1e+17"),
            (100_000_000_000_000_02.0, "1.0000000000000002e+16"),
            (123_456_789_012_345_680.0, "1.2345678901234568e+17"),
            (1e300, "1e+300"),
            (f64::NAN, "nan"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (300.0, "300.0"),
        ];
        for (value, expected) in cases {
            assert_eq!(py_float(value), expected, "repr of {}", value);
        }
    }

    #[test]
    fn py_str_shapes() {
        assert_eq!(py_str(&Value::Null), "None");
        assert_eq!(py_str(&Value::Bool(true)), "True");
        assert_eq!(py_str(&Value::Bool(false)), "False");
        assert_eq!(py_str(&Value::Int(5)), "5");
        assert_eq!(py_str(&Value::Int(-3)), "-3");
        assert_eq!(py_str(&Value::Float(1.5)), "1.5");
        assert_eq!(py_str(&Value::Float(f64::NAN)), "nan");
        assert_eq!(py_str(&Value::Str("slow".into())), "slow");
        assert_eq!(py_str(&Value::Seq(vec![Value::Int(1), Value::Str("a".into())])), "[1, 'a']");
        let mut map = BTreeMap::new();
        map.insert("a".to_owned(), Value::Int(1));
        assert_eq!(py_str(&Value::Map(map)), "{'a': 1}");
    }
}
