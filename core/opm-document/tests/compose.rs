// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Tests de composición multiarchivo (§20, §26.5, §27, §30): el contrato de
//! [`opm_document::compose`] con `opm_parser::parse`.
//!
//! Viven en `tests/` (integración) en lugar de `#[cfg(test)]`: el parser
//! depende de este crate, y un test unitario rompería la identidad de tipos a
//! través del ciclo de dev-dependencies.

use opm_document::{
    compose, document_anchors, find_heading_nodes, Block, BlockKind, ComposePart, Composed, Inline,
    Substitution,
};
use opm_parser::{parse, parse_with, ParseOptions};
use std::path::PathBuf;

fn parte(path: &str, source: &str) -> ComposePart {
    ComposePart {
        path: path.into(),
        source: source.into(),
        ast: parse(source),
    }
}

fn parte_wiki(path: &str, source: &str) -> ComposePart {
    let options = ParseOptions {
        wiki_links: true,
        ..Default::default()
    };
    ComposePart {
        path: path.into(),
        source: source.into(),
        ast: parse_with(source, &options),
    }
}

/// Visita todos los inlines de una AST en orden de documento.
fn caminar(blocks: &[Block], f: &mut impl FnMut(&Inline)) {
    for block in blocks {
        match &block.kind {
            BlockKind::Heading { content, .. } | BlockKind::Paragraph { content } => {
                for inline in content {
                    f(inline);
                }
            }
            BlockKind::Quote { blocks } | BlockKind::Callout { blocks, .. } => caminar(blocks, f),
            BlockKind::FootnoteDefinition { blocks, .. } => caminar(blocks, f),
            BlockKind::List { items, .. } => {
                for item in items {
                    caminar(&item.blocks, f);
                }
            }
            BlockKind::Table { header, rows, .. } => {
                for cell in header.iter().chain(rows.iter().flatten()) {
                    for inline in cell {
                        f(inline);
                    }
                }
            }
            BlockKind::Bibliography { entries, .. } => {
                for (label, content) in entries {
                    for inline in label.iter().chain(content.iter()) {
                        f(inline);
                    }
                }
            }
            BlockKind::CodeBlock { .. } | BlockKind::MathBlock { .. } | BlockKind::Rule => {}
        }
    }
}

fn enlaces(comp: &Composed) -> Vec<String> {
    let mut out = Vec::new();
    caminar(&comp.ast.blocks, &mut |inline| {
        if let Inline::Link { destination, .. } = inline {
            out.push(destination.clone());
        }
    });
    out
}

fn imagenes(comp: &Composed) -> Vec<String> {
    let mut out = Vec::new();
    caminar(&comp.ast.blocks, &mut |inline| {
        if let Inline::Image { destination, .. } = inline {
            out.push(destination.clone());
        }
    });
    out
}

fn wiki_links(comp: &Composed) -> Vec<String> {
    let mut out = Vec::new();
    caminar(&comp.ast.blocks, &mut |inline| {
        if let Inline::WikiLink { target } = inline {
            out.push(target.clone());
        }
    });
    out
}

fn spans_de_heading(blocks: &[Block], out: &mut Vec<u32>) {
    for block in blocks {
        match &block.kind {
            BlockKind::Heading { .. } => out.push(block.span.start),
            BlockKind::Quote { blocks } | BlockKind::Callout { blocks, .. } => {
                spans_de_heading(blocks, out)
            }
            BlockKind::FootnoteDefinition { blocks, .. } => spans_de_heading(blocks, out),
            BlockKind::List { items, .. } => {
                for item in items {
                    spans_de_heading(&item.blocks, out);
                }
            }
            _ => {}
        }
    }
}

fn niveles_de_heading(blocks: &[Block], out: &mut Vec<u8>) {
    for block in blocks {
        match &block.kind {
            BlockKind::Heading { level, .. } => out.push(*level),
            BlockKind::Quote { blocks } | BlockKind::Callout { blocks, .. } => {
                niveles_de_heading(blocks, out)
            }
            BlockKind::FootnoteDefinition { blocks, .. } => niveles_de_heading(blocks, out),
            BlockKind::List { items, .. } => {
                for item in items {
                    niveles_de_heading(&item.blocks, out);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn lista_vacia_de_partes_devuelve_documento_vacio() {
    let comp = compose(&[], false);
    assert_eq!(comp, Composed::default());
    assert!(comp.ast.blocks.is_empty());
    assert!(comp.source.is_empty());
    assert!(comp.part_offsets.is_empty());
    assert!(comp.diagnostics.is_empty());
}

/// §26.1 + política única: dos `# Introduccion` en archivos distintos se
/// deduplican y el enlace `b.md#introduccion` se reescribe al ancla final.
#[test]
fn anclas_deduplicadas_entre_archivos_y_enlace_reescrito() {
    let partes = [
        parte("a.md", "# Introduccion\n\n[ir](b.md#introduccion)\n"),
        parte("b.md", "# Introduccion\n"),
    ];
    let comp = compose(&partes, false);
    let ids: Vec<String> = document_anchors(&find_heading_nodes(&comp.source))
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(ids, ["introduccion", "introduccion-1"]);
    assert_eq!(enlaces(&comp), ["#introduccion-1"]);
    assert!(comp.diagnostics.is_empty(), "{:?}", comp.diagnostics);
}

/// §20.3/§31: con `page_break_between_files` el token literal y su bloque
/// separador (idéntico al del parser) aparecen entre partes; sin él, no.
#[test]
fn page_break_entre_partes_inserta_el_token_y_su_bloque_exacto() {
    let partes = [parte("a.md", "# uno\n"), parte("b.md", "# dos\n")];

    let comp = compose(&partes, true);
    assert_eq!(comp.source, "# uno\n\n{{pagebreak}}\n\n# dos\n");
    let tok = comp.source.find("{{pagebreak}}").unwrap() as u32;
    let parseado = parse(&comp.source);
    assert_eq!(comp.ast.blocks.len(), 3);
    assert_eq!(comp.ast.blocks[1].span.start, tok);
    // Exactamente el bloque que produce el parser para el token.
    assert_eq!(comp.ast.blocks[1], parseado.blocks[1]);
    assert!(matches!(&comp.ast.blocks[1].kind,
        BlockKind::Paragraph { content }
            if content.as_slice() == [Inline::Substitution(Substitution::PageBreak)]));

    let comp = compose(&partes, false);
    assert!(!comp.source.contains("{{pagebreak}}"));
    assert_eq!(comp.ast.blocks.len(), 2);
    assert!(comp.ast.blocks.iter().all(|b| !matches!(&b.kind,
        BlockKind::Paragraph { content }
            if content.as_slice() == [Inline::Substitution(Substitution::PageBreak)])));
}

/// §30: las etiquetas `[^x]` son locales al archivo; al fusionar cada parte
/// usa su sufijo y cada referencia resuelve a SU definición.
#[test]
fn notas_homonimas_de_dos_archivos_no_colisionan() {
    let partes = [
        parte("a.md", "ref a[^1]\n\n[^1]: Nota de A.\n"),
        parte("b.md", "ref b[^1]\n\n[^1]: Nota de B.\n"),
    ];
    let comp = compose(&partes, false);

    let mut defs = Vec::new();
    for block in &comp.ast.blocks {
        if let BlockKind::FootnoteDefinition { label, .. } = &block.kind {
            defs.push(label.clone());
        }
    }
    assert_eq!(defs, ["1--0", "1--1"]);
}

/// §20.4/§18: los destinos se rebasan léxicamente a la raíz del proyecto (el
/// documento combinado no tiene directorio propio); con esquema o absolutos
/// quedan intactos.
#[test]
fn rebase_de_imagenes_y_enlaces_a_la_raiz_del_proyecto() {
    let partes = [parte(
        "chapters/01.md",
        "![x](img.png) [s](../shared.md#sec) [e](https://ex.com/p) \
         [i](/abs.png) [o](otro.md)\n",
    )];
    let comp = compose(&partes, false);
    assert_eq!(imagenes(&comp), ["chapters/img.png"]);
    assert_eq!(
        enlaces(&comp),
        [
            "shared.md#sec",
            "https://ex.com/p",
            "/abs.png",
            "chapters/otro.md"
        ]
    );
    assert!(comp.diagnostics.is_empty(), "{:?}", comp.diagnostics);
}

/// §20.2/§21: los niveles se conservan exactamente y el front matter de las
/// partes i>0 no aparece ni en el fuente virtual ni en la AST.
#[test]
fn niveles_de_heading_intactos_y_front_matter_solo_de_la_parte_0() {
    let partes = [
        parte("a.md", "---\ntitle: Cero\n---\n# Cero\n"),
        parte("b.md", "---\ntitle: Uno\n---\n# Uno\n"),
    ];
    let comp = compose(&partes, false);

    assert_eq!(comp.ast.front_matter.as_deref(), Some("title: Cero\n"));
    assert!(comp.source.contains("title: Cero"));
    assert!(!comp.source.contains("title: Uno"));
    let mut spans = Vec::new();
    spans_de_heading(&comp.ast.blocks, &mut spans);
    assert_eq!(
        spans.len(),
        2,
        "solo los dos H1, sin bloques de front matter"
    );
    let mut niveles = Vec::new();
    niveles_de_heading(&comp.ast.blocks, &mut niveles);
    assert_eq!(niveles, [1, 1]);
    // La parte 0 no se desplaza: su heading empieza en la misma posición que
    // en su fuente original.
    assert_eq!(
        comp.ast.blocks[0].span.start as usize,
        comp.source.find("# Cero").unwrap()
    );
    // Las dos partes ocupan rangos contiguos y ordenados.
    assert_eq!(comp.part_offsets.len(), 2);
    let (p0, s0, e0) = &comp.part_offsets[0];
    let (p1, s1, e1) = &comp.part_offsets[1];
    assert_eq!(p0, &PathBuf::from("a.md"));
    assert_eq!(p1, &PathBuf::from("b.md"));
    assert_eq!(*s0, 0);
    assert!(*s0 < *e0 && *e0 <= *s1 && *s1 < *e1);
    assert!(comp.source[*s1 as usize..*e1 as usize].contains("# Uno"));
}

/// Invariante del fuente virtual: para cada heading de cada parte,
/// `find_heading_nodes` lo localiza en la misma línea que empieza su
/// `Block.span`, sin nodos de más ni de menos.
#[test]
fn invariante_de_spans_cada_heading_en_su_linea_del_fuente_virtual() {
    let partes = [
        parte(
            "a.md",
            "---\ntitle: A\n---\n# Introduccion\n\n> ## Citada\n\nSetext\n\
             =========\n\n- item\n\n  # En lista\n",
        ),
        parte("b.md", "# Introduccion\n\n## Detalle {#det}\n"),
        parte("c.md", "solo texto\n"),
    ];
    let comp = compose(&partes, false);

    let nodes = find_heading_nodes(&comp.source);
    let mut spans = Vec::new();
    spans_de_heading(&comp.ast.blocks, &mut spans);
    assert_eq!(nodes.len(), spans.len(), "nodos={spans:?} spans={nodes:?}");
    for start in spans {
        let linea = comp.source[..start as usize].matches('\n').count() + 1;
        assert!(
            nodes.iter().any(|n| n.line as usize == linea),
            "heading en la línea {linea} no localizado; nodos: {nodes:?}"
        );
    }
    // Y sin colisiones entre las anclas derivadas de ese fuente.
    let anclas = document_anchors(&nodes);
    let ids: Vec<&str> = anclas.iter().map(|a| a.id.as_str()).collect();
    let mut unicas = ids.clone();
    unicas.sort_unstable();
    unicas.dedup();
    assert_eq!(unicas.len(), ids.len(), "{ids:?}");
}

/// §20.4: una parte sin headings es alcanzable como `mm-part-{i}` y un
/// fragmento inexistente en la parte destino se diagnostica (reescribiendo
/// igualmente al ancla de inicio de la parte).
#[test]
fn parte_sin_headings_es_alcanzable_y_fragmento_roto_se_diagnostica() {
    let partes = [
        parte("a.md", "# Título\n\n[ver](b.md) y [x](b.md#nope)\n"),
        parte("b.md", "solo texto\n"),
    ];
    let comp = compose(&partes, false);
    assert_eq!(enlaces(&comp), ["#mm-part-1", "#mm-part-1"]);
    assert!(comp.source.contains("{#mm-part-1}"), "{}", comp.source);
    assert_eq!(comp.diagnostics, ["Fragmento inexistente: b.md#nope"]);
}

#[test]
fn mm_part_con_id_explicito_en_conflicto_gana_sufijo_numerico() {
    let partes = [
        parte("a.md", "# Título {#mm-part-1}\n\n[ver](b.md)\n"),
        parte("b.md", "texto\n"),
    ];
    let comp = compose(&partes, false);
    assert!(comp.source.contains("{#mm-part-1-1}"), "{}", comp.source);
    assert_eq!(enlaces(&comp), ["#mm-part-1-1"]);
}

/// §19.3: los wiki links se reescriben solo con coincidencia unívoca de
/// nombre (stem o ruta) con una parte incluida; la ambigüedad o el acierto a
/// nada dejan el literal.
#[test]
fn wiki_links_se_reescriben_solo_con_coincidencia_univoca() {
    let partes = [
        parte_wiki("chapters/01.md", "## sec\n\ntexto\n"),
        parte_wiki("chapters/02.md", "otro\n"),
        parte_wiki(
            "intro.md",
            "[[01]] [[intro]] [[otro]] [[chapters/01#sec]]\n",
        ),
    ];
    let comp = compose(&partes, false);
    // `01` → primera ancla de chapters/01.md; `intro` → mm-part-2; `otro` no
    // es ninguna parte → literal; con ruta y fragmento → ancla del heading.
    assert_eq!(wiki_links(&comp), ["#sec", "#mm-part-2", "otro", "#sec"]);

    // Ambigüedad: dos partes con stem `01` → se conserva el literal.
    let partes = [
        parte_wiki("chapters/01.md", "## sec\n"),
        parte_wiki("other/01.md", "x\n"),
        parte_wiki("intro.md", "[[01]]\n"),
    ];
    let comp = compose(&partes, false);
    assert_eq!(wiki_links(&comp), ["01"]);
}
