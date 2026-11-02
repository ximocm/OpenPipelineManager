// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Composición multiarchivo (§20): documento lógico virtual a partir de
//! varios `.md`, sin concatenerlos físicamente (§20).
//!
//! [`compose`] produce el fuente virtual y la AST combinada:
//!
//! - La parte 0 aporta su source completo (front matter, §21) para que
//!   `Ast.front_matter` siga siendo el suyo y `split_front_matter` siga
//!   funcionando sobre el documento combinado; las partes `i > 0` aportan
//!   solo su cuerpo. Las partes se unen con una línea en blanco y, con
//!   `page_break_between_files` (§20.3), con el token literal `{{pagebreak}}`
//!   entre medias.
//! - Los niveles de heading se conservan exactamente (§20.2): solo se
//!   desplazan los spans, uniformemente por parte, para coincidir con el
//!   fuente virtual.
//! - Las etiquetas de nota `[^x]` son locales al archivo (§30): al fusionar
//!   se renombran con el sufijo `--{índice}` para que dos archivos con `[^1]`
//!   no colisionen.
//! - Los destinos se reescriben para resolverse como enlaces internos del
//!   documento final (§20.4): imágenes rebasadas léxicamente a la raíz del
//!   proyecto; enlaces y wiki links a archivos incluidos, convertidos a
//!   `#ancla` según la política única de IDs de [`crate::anchors`] calculada
//!   sobre TODO el documento combinado; archivos no incluidos se rebasan con
//!   su fragmento intacto.
//! - Una parte sin headings es alcanzable como `mm-part-{i}`: al inicio de su
//!   cuerpo se inserta un párrafo vacío sobre la línea de atributos
//!   `{#mm-part-i}` (el Preview emite su `id`; el parser nunca produce
//!   párrafos vacíos, así que los documentos simples no cambian).
//!
//! Módulo puro (§4.1): solo `std`.

use crate::references::split_front_matter;
use crate::{
    document_anchors, find_heading_nodes, Anchor, Ast, Block, BlockKind, Inline, Span, Substitution,
};
use std::ops::Range;
use std::path::PathBuf;

/// Una parte de una composición: un `.md` del proyecto con su fuente completo
/// (front matter incluido) y su AST parseado de ese mismo source.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposePart {
    /// Ruta relativa a la raíz del proyecto, p. ej. `chapters/01.md` (§20.1).
    pub path: PathBuf,
    /// Fuente completo, front matter incluido.
    pub source: String,
    /// Parse de ese mismo source.
    pub ast: Ast,
}

/// Documento lógico resultante de componer varias partes (§20).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Composed {
    /// AST única del documento combinado (§20.4): bloques de las partes con
    /// sus spans desplazados al fuente virtual, separadores y reescrituras.
    pub ast: Ast,
    /// Fuente virtual: las partes concatenadas en orden; los spans de `ast`
    /// coinciden con él.
    pub source: String,
    /// `(ruta, inicio, fin)` de cada parte en el fuente virtual, en orden de
    /// composición (§20.1).
    pub part_offsets: Vec<(PathBuf, u32, u32)>,
    /// Avisos de composición (p. ej. «Fragmento inexistente», §20.4). Los IDs
    /// duplicados NO se listan aquí: los diagnostica el renderer sobre la
    /// AST combinada.
    pub diagnostics: Vec<String>,
}

/// Construye el documento combinado de una composición (§20).
///
/// `page_break_between_files` (§20.3) inserta `{{pagebreak}}` entre partes.
/// Una lista vacía devuelve un [`Composed`] vacío, sin diagnósticos.
pub fn compose(parts: &[ComposePart], page_break_between_files: bool) -> Composed {
    if parts.is_empty() {
        return Composed::default();
    }

    // Cuerpo de cada parte: la 0 conserva su fuente completo (el front matter
    // encabeza el documento combinado); las demás empiezan tras su front matter.
    let cuerpos: Vec<usize> = parts
        .iter()
        .map(|p| cuerpo(&p.source, p.ast.front_matter.is_some()))
        .collect();

    // Anclas locales por parte: el archivo como documento simple, que es como
    // se escribieron los enlaces que ahora hay que reescribir.
    let locales: Vec<Vec<Anchor>> = parts
        .iter()
        .map(|p| document_anchors(&find_heading_nodes(&p.source)))
        .collect();

    // Pasada 1: fuente sin marcadores, para calcular las anclas combinadas y
    // decidir los `mm-part-{i}` libres de colisión (los marcadores no añaden
    // headings, así que las anclas no cambian en la pasada 2).
    let previa = construir(
        parts,
        &cuerpos,
        &vec![None; parts.len()],
        page_break_between_files,
    );
    let anclas_previas = document_anchors(&find_heading_nodes(&previa.source));
    let mut ocupadas: Vec<String> = anclas_previas.iter().map(|a| a.id.clone()).collect();
    let mut marcadores: Vec<Option<String>> = Vec::with_capacity(parts.len());
    for (i, propias) in locales.iter().enumerate() {
        if !propias.is_empty() {
            marcadores.push(None);
            continue;
        }
        // Parte sin headings: ancla `mm-part-{i}` con sufijo numérico si choca
        // con un ID explícito existente o con otro mm-part ya asignado.
        let base = format!("mm-part-{i}");
        let mut id = base.clone();
        let mut n = 1;
        while ocupadas.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        ocupadas.push(id.clone());
        marcadores.push(Some(id));
    }

    // Pasada 2: fuente y AST definitivas.
    let mut hecho = construir(parts, &cuerpos, &marcadores, page_break_between_files);
    let nodes = find_heading_nodes(&hecho.source);
    let anclas = document_anchors(&nodes);

    // Anclas combinadas por parte: los headings dentro del rango de bytes de
    // cada parte, en orden, corresponden a sus anclas locales una a una.
    let mut info: Vec<InfoParte> = parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let path = normaliza(&p.path.to_string_lossy().replace('\\', "/"));
            let dir = path
                .rsplit_once('/')
                .map(|(d, _)| d.to_owned())
                .unwrap_or_default();
            InfoParte {
                path,
                dir,
                locales: locales[i].clone(),
                compuestas: Vec::new(),
                mm: marcadores[i].clone(),
            }
        })
        .collect();
    for (n, node) in nodes.iter().enumerate() {
        if let Some(i) = hecho
            .rangos
            .iter()
            .position(|(s, e)| *s <= node.span.start as usize && (node.span.start as usize) < *e)
        {
            info[i].compuestas.push(anclas[n].clone());
        }
    }

    // Reescritura de destinos (§20.4) y renombrado de notas por parte (§30).
    let mut diagnostics = Vec::new();
    for (i, rango) in hecho.rangos_bloques.iter().enumerate() {
        let (s, e) = (rango.start, rango.end);
        reescribir(&mut hecho.blocks[s..e], i, &info, &mut diagnostics);
    }

    let part_offsets = parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (s, e) = hecho.rangos[i];
            (p.path.clone(), s as u32, e as u32)
        })
        .collect();

    Composed {
        ast: Ast {
            blocks: hecho.blocks,
            front_matter: parts[0].ast.front_matter.clone(),
        },
        source: hecho.source,
        part_offsets,
        diagnostics,
    }
}

// --- Fuente virtual ----------------------------------------------------------

/// Inicio del cuerpo de una parte cuyo AST ya extrae el front matter (§21):
/// justo tras el delimitador de cierre. Devuelve 0 si la parte no tiene front
/// matter (aporta su fuente completo).
///
/// `split_front_matter` localiza el bloque; su cierre puede exceder el que usó
/// el parser en entradas raras, así que se toma el mínimo con el cierre
/// espejo del parser: nunca se descarta texto que la AST sí parseó como
/// cuerpo (los spans de sus bloques siempre quedan dentro del rango servido).
fn cuerpo(source: &str, front_matter: bool) -> usize {
    if !front_matter {
        return 0;
    }
    let split = split_front_matter(source).map(|(_, b)| salta_del_cierre(source, b));
    match (cierre_front_matter(source), split) {
        (Some(parser), Some(divido)) => parser.min(divido),
        (Some(parser), None) => parser,
        (None, Some(divido)) => divido,
        (None, None) => 0,
    }
}

/// Espejo del cierre de front matter del parser (§21): offset tras la línea
/// `---` que cierra el bloque, o `None` si la fuente no tiene front matter.
fn cierre_front_matter(source: &str) -> Option<usize> {
    let mut lines = source.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim() != "---" {
        return None;
    }
    let mut offset = first.len();
    for line in lines {
        if line.trim() == "---" {
            return Some(offset + line.len());
        }
        offset += line.len();
    }
    None
}

/// `split_front_matter` devuelve el cuerpo justo antes del delimitador de
/// cierre (`\n---`); este lo salta para empezar tras él, como el parser.
fn salta_del_cierre(source: &str, inicio: usize) -> usize {
    let Some(resto) = source.get(inicio..) else {
        return inicio;
    };
    if !resto.starts_with("\n---") {
        return inicio;
    }
    let bytes = source.as_bytes();
    let mut i = inicio + 4;
    if bytes.get(i) == Some(&b'\r') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'\n') {
        i += 1;
    }
    i
}

/// Fuente virtual + bloques con spans absolutos + rangos por parte.
struct Construccion {
    source: String,
    blocks: Vec<Block>,
    /// `(inicio, fin)` en bytes de cada parte dentro de `source`.
    rangos: Vec<(usize, usize)>,
    /// Índices de `blocks` que pertenecen a cada parte.
    rangos_bloques: Vec<Range<usize>>,
}

fn construir(
    partes: &[ComposePart],
    cuerpos: &[usize],
    marcadores: &[Option<String>],
    page_break: bool,
) -> Construccion {
    let mut out = Construccion {
        source: String::new(),
        blocks: Vec::new(),
        rangos: Vec::new(),
        rangos_bloques: Vec::new(),
    };
    for (i, parte) in partes.iter().enumerate() {
        if i > 0 {
            // Unión con separador de línea en blanco (§20).
            if !out.source.ends_with("\n\n") {
                if out.source.ends_with('\n') {
                    out.source.push('\n');
                } else {
                    out.source.push_str("\n\n");
                }
            }
            if page_break {
                // Token literal `{{pagebreak}}` (§31) seguido del separador:
                // el bloque separador es exactamente el del parser, con su
                // span apuntando al texto insertado.
                let tok = out.source.len() as u32;
                out.source.push_str("{{pagebreak}}\n\n");
                out.blocks.push(Block {
                    span: Span {
                        start: tok,
                        end: tok + "{{pagebreak}}\n".len() as u32,
                    },
                    kind: BlockKind::Paragraph {
                        content: vec![Inline::Substitution(Substitution::PageBreak)],
                    },
                });
            }
        }

        // La parte 0 conserva su fuente completo: el front matter (§21) sigue
        // encabezando el documento y el marcador se inserta tras él.
        if i == 0 {
            out.source.push_str(&parte.source[..cuerpos[0]]);
        }
        let bloques_start = out.blocks.len();

        // Párrafo ancla `mm-part-{i}` (§20.4) al inicio del cuerpo de la
        // parte: línea de atributos en el fuente + párrafo vacío en la AST.
        if let Some(id) = &marcadores[i] {
            let marca = out.source.len();
            let linea = format!("{{#{id}}}\n");
            out.source.push_str(&linea);
            out.source.push('\n');
            out.blocks.push(Block {
                span: Span {
                    start: marca as u32,
                    end: (marca + linea.len()) as u32,
                },
                kind: BlockKind::Paragraph { content: vec![] },
            });
        }

        // Origen virtual del cuerpo y desplazamiento uniforme de la parte
        // (negativo cuando el front matter extraído era más largo que el
        // prefijo virtual acumulado; los bloques nunca bajan de 0 porque
        // todos empiezan en o después del cuerpo).
        let origen = out.source.len();
        if i == 0 {
            out.source.push_str(&parte.source[cuerpos[0]..]);
        } else {
            out.source.push_str(&parte.source[cuerpos[i]..]);
        }
        let fin = out.source.len();
        let shift = origen as i64 - cuerpos[i] as i64;

        // Rangos: la parte 0 cubre su fuente completa (front matter incluido,
        // con su marcador si lo hay); las demás cubren solo su cuerpo.
        out.rangos
            .push(if i == 0 { (0, fin) } else { (origen, fin) });

        for bloque in &parte.ast.blocks {
            let mut bloque = bloque.clone();
            desplazar(&mut bloque, shift);
            out.blocks.push(bloque);
        }
        out.rangos_bloques.push(bloques_start..out.blocks.len());
    }
    out
}

/// Desplaza uniformemente todos los spans de un bloque (y de sus bloques
/// anidados y `ListItem`) al fuente virtual.
fn desplazar(bloque: &mut Block, d: i64) {
    mover(&mut bloque.span, d);
    match &mut bloque.kind {
        BlockKind::Quote { blocks } | BlockKind::Callout { blocks, .. } => {
            for b in blocks {
                desplazar(b, d);
            }
        }
        BlockKind::FootnoteDefinition { blocks, .. } => {
            for b in blocks {
                desplazar(b, d);
            }
        }
        BlockKind::List { items, .. } => {
            for item in items {
                mover(&mut item.span, d);
                for b in &mut item.blocks {
                    desplazar(b, d);
                }
            }
        }
        _ => {}
    }
}

fn mover(span: &mut Span, d: i64) {
    span.start = u32::try_from(span.start as i64 + d).expect("span negativo en la composición");
    span.end = u32::try_from(span.end as i64 + d).expect("span negativo en la composición");
}

// --- Reescritura de destinos (§20.4) ----------------------------------------

/// Estado de una parte para reescribir destinos dentro de ella.
struct InfoParte {
    /// Ruta de la parte normalizada a la raíz del proyecto.
    path: String,
    /// Directorio de la parte (`chapters` o vacío en la raíz).
    dir: String,
    /// Anclas del archivo como documento simple (política única, §26).
    locales: Vec<Anchor>,
    /// Anclas de la parte dentro del documento combinado, en el mismo orden.
    compuestas: Vec<Anchor>,
    /// `mm-part-{i}` si la parte no tiene headings.
    mm: Option<String>,
}

fn reescribir(blocks: &mut [Block], propio: usize, info: &[InfoParte], diag: &mut Vec<String>) {
    for block in blocks {
        reescribir_bloque(block, propio, info, diag);
    }
}

fn reescribir_bloque(block: &mut Block, propio: usize, info: &[InfoParte], diag: &mut Vec<String>) {
    match &mut block.kind {
        BlockKind::Heading { content, .. } | BlockKind::Paragraph { content } => {
            reescribir_inlines(content, propio, info, diag)
        }
        BlockKind::Quote { blocks } | BlockKind::Callout { blocks, .. } => {
            reescribir(blocks, propio, info, diag)
        }
        BlockKind::FootnoteDefinition { label, blocks } => {
            renombrar_nota(label, propio);
            reescribir(blocks, propio, info, diag);
        }
        BlockKind::List { items, .. } => {
            for item in items {
                reescribir(&mut item.blocks, propio, info, diag);
            }
        }
        BlockKind::Table { header, rows, .. } => {
            for cell in header.iter_mut().chain(rows.iter_mut().flatten()) {
                reescribir_inlines(cell, propio, info, diag);
            }
        }
        BlockKind::Bibliography { entries, .. } => {
            for (label, content) in entries.iter_mut() {
                reescribir_inlines(label, propio, info, diag);
                reescribir_inlines(content, propio, info, diag);
            }
        }
        BlockKind::CodeBlock { .. } | BlockKind::MathBlock { .. } | BlockKind::Rule => {}
    }
}

fn reescribir_inlines(
    inlines: &mut [Inline],
    propio: usize,
    info: &[InfoParte],
    diag: &mut Vec<String>,
) {
    for inline in inlines {
        match inline {
            Inline::FootnoteReference(label) => renombrar_nota(label, propio),
            Inline::Link {
                destination,
                content,
                ..
            } => {
                if let Some(nuevo) = reescribe_enlace(destination, propio, info, diag) {
                    *destination = nuevo;
                }
                reescribir_inlines(content, propio, info, diag);
            }
            Inline::Image {
                destination, alt, ..
            } => {
                if let Some(nuevo) = reescribe_imagen(destination, propio, info) {
                    *destination = nuevo;
                }
                reescribir_inlines(alt, propio, info, diag);
            }
            Inline::WikiLink { target } => {
                if let Some(nuevo) = reescribe_wiki(target, info, diag) {
                    *target = nuevo;
                }
            }
            Inline::Styled { content, .. }
            | Inline::Emphasis(content)
            | Inline::Strong(content)
            | Inline::Strikethrough(content) => reescribir_inlines(content, propio, info, diag),
            Inline::Text(_)
            | Inline::Code(_)
            | Inline::Math(_)
            | Inline::Color { .. }
            | Inline::Citation { .. }
            | Inline::Substitution(_)
            | Inline::TaskListMarker(_)
            | Inline::SoftBreak
            | Inline::HardBreak => {}
        }
    }
}

/// Las notas son locales al archivo (§30): al fusionar, cada parte usa su
/// propio sufijo `--{índice}` para que `[^1]` de dos archivos no colisione.
fn renombrar_nota(label: &mut String, propio: usize) {
    label.push_str("--");
    label.push_str(&propio.to_string());
}

/// Enlace Markdown: si el destino resuelve a una parte incluida se convierte
/// en `#ancla` del documento combinado; si no, se rebasa a la raíz del
/// proyecto conservando el fragmento. Los fragmentos puros (`#…`) del mismo
/// archivo quedan intactos (mismo comportamiento que un documento simple).
fn reescribe_enlace(
    destino: &str,
    propio: usize,
    info: &[InfoParte],
    diag: &mut Vec<String>,
) -> Option<String> {
    if destino_intacto(destino) {
        return None;
    }
    let (ruta, frag) = parte_fragmento(destino);
    if ruta.is_empty() {
        return None;
    }
    let resuelto = une_dir(&info[propio].dir, ruta);
    Some(match info.iter().position(|i| i.path == resuelto) {
        Some(j) => format!("#{}", ancla_objetivo(j, frag, info, destino, diag)),
        None => format!("{resuelto}{}", sufijo_fragmento(frag)),
    })
}

/// Imagen: rebase léxico a la ruta relativa a la raíz (el documento combinado
/// no tiene directorio propio). Destinos con esquema, `//` o absolutos quedan
/// intactos (el loader de recursos los rechaza ya hoy).
fn reescribe_imagen(destino: &str, propio: usize, info: &[InfoParte]) -> Option<String> {
    if destino.is_empty() || destino_intacto(destino) {
        return None;
    }
    let (ruta, frag) = parte_fragmento(destino);
    if ruta.is_empty() {
        return None;
    }
    Some(format!(
        "{}{}",
        une_dir(&info[propio].dir, ruta),
        sufijo_fragmento(frag)
    ))
}

/// Wiki link (§19.3): solo se reescribe si el nombre coincide de forma
/// unívoca con el stem o la ruta de UNA parte incluida; con ambigüedad o sin
/// coincidencia se conserva el literal.
fn reescribe_wiki(target: &str, info: &[InfoParte], diag: &mut Vec<String>) -> Option<String> {
    let (nombre, frag) = parte_fragmento(target);
    let mut hits = info
        .iter()
        .enumerate()
        .filter(|(_, i)| coincide_wiki(nombre, i))
        .map(|(j, _)| j);
    let j = hits.next()?;
    if hits.next().is_some() {
        return None;
    }
    Some(format!("#{}", ancla_objetivo(j, frag, info, target, diag)))
}

/// Ancla del documento combinado para una parte: con fragmento, la ancla final
/// del heading que lleva ese ID local (explícito o slug, §26.1); sin él, la
/// primera ancla de la parte o `mm-part-{i}` si no tiene headings. Un
/// fragmento inexistente en la parte destino se diagnostica y cae al ancla de
/// inicio de la parte (§20.4).
fn ancla_objetivo(
    j: usize,
    frag: Option<&str>,
    info: &[InfoParte],
    destino: &str,
    diag: &mut Vec<String>,
) -> String {
    if let Some(f) = frag {
        if let Some(k) = info[j].locales.iter().position(|a| a.id == f) {
            if let Some(ancla) = info[j].compuestas.get(k) {
                return ancla.id.clone();
            }
        }
        let aviso = format!("Fragmento inexistente: {destino}");
        if !diag.contains(&aviso) {
            diag.push(aviso);
        }
        return inicio_de_parte(j, info);
    }
    inicio_de_parte(j, info)
}

fn inicio_de_parte(j: usize, info: &[InfoParte]) -> String {
    info[j]
        .compuestas
        .first()
        .map(|a| a.id.clone())
        .or_else(|| info[j].mm.clone())
        .unwrap_or_else(|| format!("mm-part-{j}"))
}

fn coincide_wiki(nombre: &str, i: &InfoParte) -> bool {
    let n = normaliza(nombre);
    let base = i.path.rsplit('/').next().unwrap_or(&i.path);
    let sin_extension = i.path.strip_suffix(".md").unwrap_or(&i.path);
    let base_sin_extension = base.strip_suffix(".md").unwrap_or(base);
    n == i.path || n == sin_extension || n == base || n == base_sin_extension
}

/// ¿El destino no debe rebasarse (esquema, `//` o ruta absoluta)?
fn destino_intacto(destino: &str) -> bool {
    destino.starts_with('/') || tiene_esquema(destino)
}

/// Esquema RFC 3986 (`https:`, `mailto:`, `data:`, también `C:` de rutas
/// Windows) antes de cualquier `/`, `#` o `?`.
fn tiene_esquema(destino: &str) -> bool {
    let Some(corte) = destino.find(':') else {
        return false;
    };
    let head = &destino[..corte];
    let mut chars = head.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Divide `ruta#fragmento` (el fragmento vacío cuenta como ausente).
fn parte_fragmento(destino: &str) -> (&str, Option<&str>) {
    match destino.split_once('#') {
        Some((ruta, frag)) => (ruta, Some(frag).filter(|f| !f.is_empty())),
        None => (destino, None),
    }
}

fn sufijo_fragmento(frag: Option<&str>) -> String {
    frag.map(|f| format!("#{f}")).unwrap_or_default()
}

/// Une `ruta` con el directorio de la parte y normaliza `.`/`..` léxicalemente.
fn une_dir(dir: &str, ruta: &str) -> String {
    if dir.is_empty() {
        normaliza(ruta)
    } else {
        normaliza(&format!("{dir}/{ruta}"))
    }
}

/// Normalización léxica de una ruta relativa: se eliminan `.` y componentes
/// vacíos y se resuelven `..` contra los componentes previos. Un `..` que
/// sobrepasa la raíz se conserva: el destino queda fuera del proyecto y el
/// loader de recursos lo rechaza.
fn normaliza(ruta: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for comp in ruta.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|c| *c != "..") {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out.join("/")
}
