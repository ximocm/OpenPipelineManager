//! Comparación de rutas SEGMENTO A SEGMENTO con separadores normalizados
//! (P1, paridad con el fix del backend Python): una ruta serializada en
//! Windows (`folder\file.txt`) seguía siendo un descendiente de `folder`
//! aunque el check solo mirara `/`, y el prefijo crudo confundía `folderx`
//! con `folder`. Toda comparación de rutas de la UI pasa por aquí
//! ([`is_descendant`] / [`retargeted`]); para entradas de forma homogénea el
//! resultado es el del antiguo patrón `p == base || p.starts_with(base)`.

use std::path::{Path, PathBuf};

/// Segmentos de `path` tratando `\` y `/` como el mismo separador (los
/// vacíos —barras dobles o final— y `.` se descartan; `..` se conserva como
/// segmento literal, igual que hacía `Path::starts_with`).
fn segments(path: &Path) -> Vec<String> {
    path.to_string_lossy()
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect()
}

/// Segmentos de `path` que quedan POR DEBAJO de `ancestor`: `Some(vec![])`
/// si es el mismo destino, `Some(sufijo)` si está dentro, `None` si no lo
/// está (ni es él mismo).
pub(crate) fn suffix_segments(path: &Path, ancestor: &Path) -> Option<Vec<String>> {
    let path_segments = segments(path);
    let ancestor_segments = segments(ancestor);
    if path_segments.len() < ancestor_segments.len() {
        return None;
    }
    let (prefix, suffix) = path_segments.split_at(ancestor_segments.len());
    (prefix == ancestor_segments.as_slice()).then(|| suffix.to_vec())
}

/// ¿`path` es el propio `ancestor` o está por debajo de él? Por segmentos y
/// con separadores normalizados: `folder\file.txt` y `folder/file.txt` son
/// el mismo descendiente de `folder`; `folderx` no lo es. Incluye la propia
/// ruta (equivalente al patrón `p == base || p.starts_with(base)` que
/// sustituye en la UI).
pub(crate) fn is_descendant(path: &Path, ancestor: &Path) -> bool {
    suffix_segments(path, ancestor).is_some()
}

/// Re-objetivación de un rename/move: `to` con el sufijo relativo de `old`
/// bajo `from` re-pegado — SEGMENTO a segmento, sin depender del separador
/// serializado de `old` y sin `join("")` (un `PathBuf::join` con cadena
/// vacía añadiría un separador final que rompería el siguiente guardado con
/// ENOTDIR). `None` si `old` no está bajo `from` (no afectado).
pub(crate) fn retargeted(old: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let suffix = suffix_segments(old, from)?;
    let mut new_path = to.to_path_buf();
    for segment in &suffix {
        new_path.push(segment);
    }
    Some(new_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1 (regresión): formas mezcladas — `folder\file.txt` vs `folder`,
    /// `folder/sub` vs `folder`, y `folderx` NO descendiente de `folder`.
    #[test]
    fn is_descendant_normaliza_separadores_por_segmentos() {
        // Serialización Windows persistida: sigue bajo `folder`.
        assert!(is_descendant(
            Path::new(r"folder\file.txt"),
            Path::new("folder")
        ));
        // Forma nativa POSIX: igual que antes.
        assert!(is_descendant(Path::new("folder/sub"), Path::new("folder")));
        // Prefijo crudo ≠ segmento: `folderx` NO es descendiente.
        assert!(!is_descendant(
            Path::new("folderx/file.txt"),
            Path::new("folder")
        ));
        assert!(!is_descendant(Path::new("folderx"), Path::new("folder")));
        // La propia ruta cuenta (paridad con `== || starts_with`).
        assert!(is_descendant(Path::new("folder"), Path::new("folder")));
        // Dirección: un ancestro no es descendiente de su hijo.
        assert!(!is_descendant(Path::new("folder"), Path::new("folder/sub")));
        // Del lado del ancestro también puede llegar serializado al revés.
        assert!(is_descendant(
            Path::new("folder/file.txt"),
            Path::new(r"folder\")
        ));
        // Barra final y `.` sobrantes no cambian la comparación.
        assert!(is_descendant(Path::new("./folder/sub/"), Path::new("folder")));
        // Descendientes de varios niveles.
        assert!(is_descendant(
            Path::new("folder/a/b.txt"),
            Path::new("folder")
        ));
        assert!(!is_descendant(
            Path::new("folder/a/b.txt"),
            Path::new("folder/a/x")
        ));
    }

    /// P1 (regresión): el sufijo de la re-objetivación se re-pega por
    /// segmentos aunque `old` venga serializado con el otro separador, y el
    /// destino del mismo path no gana barra final (ENOTDIR).
    #[test]
    fn retargeted_repega_sufijos_con_formas_mixtas() {
        assert_eq!(
            retargeted(
                Path::new(r"folder\file.txt"),
                Path::new("folder"),
                Path::new("capitulos")
            ),
            Some(Path::new("capitulos").join("file.txt"))
        );
        // Igual que antes con formas homogéneas (multi-segmento).
        assert_eq!(
            retargeted(
                Path::new("cap/a/doc.md"),
                Path::new("cap"),
                Path::new("capitulos")
            ),
            Some(Path::new("capitulos").join("a").join("doc.md"))
        );
        // El propio `from` mapea a `to` SIN barra final.
        assert_eq!(
            retargeted(Path::new("cap"), Path::new("cap"), Path::new("nuevo")),
            Some(PathBuf::from("nuevo"))
        );
        // No afectado → `None`.
        assert_eq!(
            retargeted(Path::new("otro/a.md"), Path::new("cap"), Path::new("nuevo")),
            None
        );
        assert_eq!(
            retargeted(Path::new("capx/a.md"), Path::new("cap"), Path::new("nuevo")),
            None
        );
    }

    /// `suffix_segments` en detalle: igual → vacío; dentro → sufijo; fuera →
    /// `None`.
    #[test]
    fn suffix_segments_distingue_igual_dentro_y_fuera() {
        assert_eq!(
            suffix_segments(Path::new(r"folder\sub\doc.md"), Path::new("folder")),
            Some(vec![
                "sub".to_owned(),
                "doc.md".to_owned()
            ])
        );
        assert_eq!(
            suffix_segments(Path::new("folder"), Path::new("folder")),
            Some(Vec::new())
        );
        assert_eq!(suffix_segments(Path::new("folder"), Path::new("otro")), None);
        assert_eq!(
            suffix_segments(Path::new("fol"), Path::new("folder")),
            None
        );
    }
}
