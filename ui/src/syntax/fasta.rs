//! Lexer FASTA: cabeceras `>`, corridas de secuencia y comentarios `;`.
//!
//! Formato simple y por líneas (FASTA/QUAL de bioinformática, los `.fa`,
//! `.fasta`, `.fna`, `.ffn` y `.faa` de OPM):
//!
//! - línea que empieza por `>` → [`SyntaxRole::Header`] (toda la línea);
//! - línea que empieza por `;` → [`SyntaxRole::Comment`] (comentario
//!   clásico de FASTA);
//! - cualquier otra línea no vacía → [`SyntaxRole::Sequence`] (sin el `\r`
//!   del CRLF, para que el tag no pinte el terminador).

use super::SyntaxRole;

pub(crate) fn highlight(text: &str) -> Vec<(usize, usize, SyntaxRole)> {
    let mut spans = Vec::new();
    for (offset, line) in super::lines_with_offsets(text) {
        if line.is_empty() {
            continue;
        }
        let role = match line.as_bytes()[0] {
            b'>' => SyntaxRole::Header,
            b';' => SyntaxRole::Comment,
            _ => SyntaxRole::Sequence,
        };
        spans.push((offset, offset + line.len(), role));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::highlight;
    use crate::syntax::SyntaxRole;

    /// Span de la línea `index`-ésima (0-based) sobre `text`, en bytes.
    fn line_span(text: &str, index: usize) -> (usize, usize) {
        let mut offset = 0usize;
        for (i, raw) in text.split_inclusive('\n').enumerate() {
            let content = raw
                .strip_suffix('\n')
                .map(|s| s.strip_suffix('\r').unwrap_or(s))
                .unwrap_or(raw);
            if i == index {
                return (offset, offset + content.len());
            }
            offset += raw.len();
        }
        panic!("no hay línea {index}");
    }

    fn roles(text: &str) -> Vec<SyntaxRole> {
        highlight(text).iter().map(|(_, _, r)| *r).collect()
    }

    #[test]
    fn cabecera_secuencia_y_comentario() {
        let text = ">chr1 plasmid\nATGCATGC\n; nota interna\nAT GC\n";
        assert_eq!(
            roles(text),
            vec![
                SyntaxRole::Header,
                SyntaxRole::Sequence,
                SyntaxRole::Comment,
                SyntaxRole::Sequence,
            ]
        );
        // La cabecera es la línea completa desde `>`.
        let (start, end) = line_span(text, 0);
        assert_eq!(&text[start..end], ">chr1 plasmid");
    }

    #[test]
    fn lineas_vacias_sin_span() {
        let text = ">a\n\nATGC\n\n";
        assert_eq!(roles(text), vec![SyntaxRole::Header, SyntaxRole::Sequence]);
    }

    #[test]
    fn crlf_no_pinta_el_retorno() {
        let text = ">a\r\nATGC\r\n";
        let spans = highlight(text);
        assert_eq!(spans.len(), 2);
        assert_eq!(&text[spans[0].0..spans[0].1], ">a");
        assert_eq!(&text[spans[1].0..spans[1].1], "ATGC");
        // Sin solapes y en orden.
        assert!(spans[0].1 <= spans[1].0);
    }

    #[test]
    fn unicode_en_secuencia_y_comentario() {
        // No hay por qué: FASTA es ASCII, pero el lexer no debe partir
        // bytes multibyte ni panic con comentarios UTF-8.
        let text = ">seq émoji 🧬\n; comentarios en español\nACGT\n";
        let spans = highlight(text);
        assert_eq!(spans.len(), 3);
        for (start, end, _) in &spans {
            assert!(text.is_char_boundary(*start) && text.is_char_boundary(*end));
        }
        assert_eq!(
            spans.iter().map(|(_, _, r)| *r).collect::<Vec<_>>(),
            vec![SyntaxRole::Header, SyntaxRole::Comment, SyntaxRole::Sequence]
        );
    }

    #[test]
    fn texto_vacio() {
        assert!(highlight("").is_empty());
        assert!(highlight("\n\n").is_empty());
    }
}
