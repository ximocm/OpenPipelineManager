// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Gramática de atributos (§7.4.3).
//!
//! Forma: `{#id "caption" clave=valor ...}` — `#id` (`[A-Za-z0-9:._-]+`),
//! caption entrecomillado y pares `clave=valor`. Único valor con significado
//! en v1: `width=N%` con N en 1–100 (§18.5). Cualquier otra cosa no encaja
//! con la gramática y la construcción completa es texto literal (§7.4.1).
//!
//! El parser (`opm-parser`) y el escáner de anclas ([`crate::anchors`])
//! comparten esta única implementación para que los IDs explícitos del AST y
//! los del índice sean siempre los mismos (ISSUE: resolución de destinos).

use crate::ast::Attributes;

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-')
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

fn is_value_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '%')
}

/// Parsea el interior de `{...}` (sin las llaves) como atributos.
/// `None` = no encaja con la gramática (la construcción será texto literal).
pub fn parse_attributes(inner: &str) -> Option<Attributes> {
    let mut attrs = Attributes::default();
    let mut cur = inner.chars().peekable();
    let mut seen_id = false;
    let mut seen_caption = false;

    loop {
        while matches!(cur.peek(), Some(c) if c.is_whitespace()) {
            cur.next();
        }
        match cur.peek() {
            None => break,
            Some('#') => {
                if seen_id {
                    return None;
                }
                cur.next();
                let id: String = take_while(&mut cur, is_id_char);
                if id.is_empty() {
                    return None;
                }
                attrs.id = Some(id);
                seen_id = true;
            }
            Some('"') => {
                if seen_caption {
                    return None;
                }
                cur.next();
                let mut caption = String::new();
                loop {
                    match cur.next() {
                        Some('\\') => match cur.next() {
                            Some(c) => caption.push(c),
                            None => return None,
                        },
                        Some('"') => break,
                        Some(c) => caption.push(c),
                        None => return None,
                    }
                }
                attrs.caption = Some(caption);
                seen_caption = true;
            }
            Some(_) => {
                let key: String = take_while(&mut cur, is_key_char);
                if key.is_empty() || cur.peek() != Some(&'=') {
                    return None;
                }
                cur.next();
                let value: String = take_while(&mut cur, is_value_char);
                if value.is_empty() {
                    return None;
                }
                if key == "width" {
                    // §18.5: solo porcentaje, 1%–100%.
                    let pct = value.strip_suffix('%')?;
                    let n: u8 = pct.parse().ok()?;
                    if !(1..=100).contains(&n) || attrs.width_percent.is_some() {
                        return None;
                    }
                    attrs.width_percent = Some(n);
                } else {
                    attrs.extra.push((key, value));
                }
            }
        }
    }
    Some(attrs)
}

fn take_while(cur: &mut std::iter::Peekable<std::str::Chars<'_>>, f: fn(char) -> bool) -> String {
    let mut out = String::new();
    while matches!(cur.peek(), Some(c) if f(*c)) {
        out.push(cur.next().unwrap());
    }
    out
}

/// Atributos `{...}` al final de un texto (posición 3 §7.4.3: fin de un
/// heading, §26.1). Devuelve el texto sin los atributos y los atributos;
/// `None` si el final del texto no es una construcción de atributos válida.
///
/// Es la misma regla que aplica el parser sobre el último texto literal del
/// heading (§26.1): `rfind('{')` más cierre `}` y gramática no vacía.
pub fn strip_trailing_attrs(text: &str) -> Option<(&str, Attributes)> {
    let open = text.rfind('{')?;
    if !text.ends_with('}') || open + 1 >= text.len() - 1 {
        return None;
    }
    let attrs = parse_attributes(&text[open + 1..text.len() - 1])?;
    if attrs.is_empty() {
        return None;
    }
    Some((text[..open].trim_end(), attrs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formas_validas() {
        assert_eq!(
            parse_attributes("#architecture"),
            Some(Attributes { id: Some("architecture".into()), ..Default::default() })
        );

        let attrs = parse_attributes("#tbl:results \"Resultados del experimento\"").unwrap();
        assert_eq!(attrs.id.as_deref(), Some("tbl:results"));
        assert_eq!(attrs.caption.as_deref(), Some("Resultados del experimento"));

        let attrs = parse_attributes("width=50%").unwrap();
        assert_eq!(attrs.width_percent, Some(50));

        let attrs = parse_attributes("#fig:arq \"Diagrama\" width=30% other=x").unwrap();
        assert_eq!(attrs.id.as_deref(), Some("fig:arq"));
        assert_eq!(attrs.caption.as_deref(), Some("Diagrama"));
        assert_eq!(attrs.width_percent, Some(30));
        assert_eq!(attrs.extra, vec![("other".to_owned(), "x".to_owned())]);
    }

    #[test]
    fn formas_invalidas() {
        assert_eq!(parse_attributes("width=150%"), None);
        assert_eq!(parse_attributes("width=0%"), None);
        assert_eq!(parse_attributes("width=50"), None);
        // No es `clave=valor` ni `#id` ni caption: texto literal (§7.4.1).
        assert_eq!(parse_attributes("danger: texto"), None);
        assert_eq!(parse_attributes("\"sin cerrar"), None);
        assert_eq!(parse_attributes("#a #b"), None);
        assert_eq!(parse_attributes("solo-clave"), None);
    }

    #[test]
    fn caption_con_escapes() {
        let attrs = parse_attributes("#x \"dijo: \\\"hola\\\"\"").unwrap();
        assert_eq!(attrs.caption.as_deref(), Some("dijo: \"hola\""));
    }

    #[test]
    fn atributos_al_final_del_texto_26_1() {
        assert_eq!(
            strip_trailing_attrs("Título {#id}"),
            Some(("Título", Attributes { id: Some("id".into()), ..Default::default() }))
        );
        // Sin gramática válida el texto se conserva tal cual (§7.4.1).
        assert_eq!(strip_trailing_attrs("Título {sin atributos}"), None);
        assert_eq!(strip_trailing_attrs("Título"), None);
        assert_eq!(strip_trailing_attrs("{#solo}"), Some(("", Attributes { id: Some("solo".into()), ..Default::default() })));
    }
}
