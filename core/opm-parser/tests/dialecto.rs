// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Corpus de validación del dialecto §7.4 (ISSUE-014, criterio 5):
//! casos positivos y negativos de la gramática de extensiones.
//!
//! Cada test referencia la sección normativa que lo define.

use opm_document::{Attributes, BlockKind, CitationItem, Inline, Substitution};
use opm_parser::{parse, parse_with, ParseOptions};

fn only_paragraph(source: &str) -> Vec<Inline> {
    let ast = parse(source);
    assert_eq!(ast.blocks.len(), 1, "esperaba un único bloque en {source:?}");
    let BlockKind::Paragraph { content } = &ast.blocks[0].kind else {
        panic!("esperaba párrafo en {source:?}: {:?}", ast.blocks[0]);
    };
    content.clone()
}

// --- Positivos ---------------------------------------------------------------

#[test]
fn tablas_gfm_y_code_fences() {
    let ast = parse("| A | B |\n|:--|--:|\n| 1 | 2 |\n");
    assert!(matches!(
        &ast.blocks[0].kind,
        BlockKind::Table { aligns, rows, .. }
            if aligns.len() == 2 && rows.len() == 1
    ));

    let ast = parse("```python\nprint({{toc}})\n```\n");
    let BlockKind::CodeBlock { language, code, .. } = &ast.blocks[0].kind else {
        panic!("esperaba code fence");
    };
    assert_eq!(language.as_deref(), Some("python"));
    // §7.4.1: dentro de fences no se interpreta ninguna extensión.
    assert_eq!(code, "print({{toc}})\n");
}

#[test]
fn code_fences_protegen_bloques_extendidos() {
    // §7.4.1: `:::` (§22) y `$$` (§24) dentro de un fence son código,
    // no callouts ni ecuaciones de bloque.
    let source = "```markdown\n::: warning \"X\"\n$$\nE = mc^2\n$$\n:::\n```\n";
    let ast = parse(source);
    assert_eq!(ast.blocks.len(), 1, "{:?}", ast.blocks);
    let BlockKind::CodeBlock { language, code } = &ast.blocks[0].kind else {
        panic!("esperaba code fence: {:?}", ast.blocks[0]);
    };
    assert_eq!(language.as_deref(), Some("markdown"));
    assert_eq!(code, "::: warning \"X\"\n$$\nE = mc^2\n$$\n:::\n");

    // Un callout que contiene un fence con `:::` cierra con su propio `:::`.
    let ast = parse("::: note \"T\"\n```md\n:::\n```\ncontenido\n:::\n");
    assert_eq!(ast.blocks.len(), 1, "{:?}", ast.blocks);
    let BlockKind::Callout { blocks, .. } = &ast.blocks[0].kind else {
        panic!("esperaba callout: {:?}", ast.blocks[0]);
    };
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert!(
        matches!(&blocks[0].kind, BlockKind::CodeBlock { code, .. } if code == ":::\n"),
        "{:?}",
        blocks[0]
    );
    assert!(
        matches!(&blocks[1].kind, BlockKind::Paragraph { .. }),
        "{:?}",
        blocks[1]
    );

    // Fences de `~` también protegen.
    let ast = parse("~~~\n$$\nx = 1\n$$\n~~~\n");
    assert_eq!(ast.blocks.len(), 1, "{:?}", ast.blocks);
    assert!(
        matches!(&ast.blocks[0].kind, BlockKind::CodeBlock { code, .. } if code == "$$\nx = 1\n$$\n"),
        "{:?}",
        ast.blocks[0]
    );
}

#[test]
fn math_latex_inline_y_bloque() {
    // §24.
    assert!(only_paragraph("La energía es $E = mc^2$.\n").contains(&Inline::Math(
        "E = mc^2".into()
    )));

    let ast = parse("$$\n\\int_0^\\infty e^{-x} dx\n$$\n");
    assert!(matches!(
        &ast.blocks[0].kind,
        BlockKind::MathBlock { latex, .. } if latex == "\\int_0^\\infty e^{-x} dx"
    ));
}

#[test]
fn citas_estilo_pandoc() {
    // §25.2.
    assert!(only_paragraph("[@smith2025]\n").contains(&Inline::Citation {
        items: vec![CitationItem { key: "smith2025".into(), locator: None }]
    }));
    assert!(only_paragraph("[@smith2025; @jones2024]\n").contains(&Inline::Citation {
        items: vec![
            CitationItem { key: "smith2025".into(), locator: None },
            CitationItem { key: "jones2024".into(), locator: None },
        ]
    }));
    assert!(only_paragraph("[@smith2025, p. 42]\n").contains(&Inline::Citation {
        items: vec![CitationItem { key: "smith2025".into(), locator: Some("p. 42".into()) }]
    }));
}

#[test]
fn cross_refs_y_sustituciones() {
    // §7.4.2, §26.5, §29, §31.
    for (source, expected) in [
        ("{{ref:fig:architecture}}", Substitution::CrossRef { id: "fig:architecture".into() }),
        ("{{ref:tbl:results}}", Substitution::CrossRef { id: "tbl:results".into() }),
        ("{{ref:eq:energy}}", Substitution::CrossRef { id: "eq:energy".into() }),
        ("{{toc}}", Substitution::Toc),
        ("{{pagebreak}}", Substitution::PageBreak),
    ] {
        assert!(
            only_paragraph(source).contains(&Inline::Substitution(expected.clone())),
            "falló {source}"
        );
    }
}

#[test]
fn callouts_integrados() {
    // §22.
    let ast = parse("::: warning\nNo elimines este archivo.\n:::\n");
    assert!(matches!(
        &ast.blocks[0].kind,
        BlockKind::Callout { callout_type, title, .. }
            if callout_type == "warning" && title.is_none()
    ));

    let ast = parse("::: warning \"Atención\"\nContenido.\n:::\n");
    assert!(matches!(
        &ast.blocks[0].kind,
        BlockKind::Callout { callout_type, title, .. }
            if callout_type == "warning" && title.as_deref() == Some("Atención")
    ));

    for tipo in ["note", "info", "tip", "danger", "success"] {
        let ast = parse(&format!("::: {tipo}\nx\n:::\n"));
        assert!(
            matches!(&ast.blocks[0].kind, BlockKind::Callout { callout_type, .. } if callout_type == tipo),
            "tipo {tipo}"
        );
    }
}

#[test]
fn colores_semanticos() {
    // §23, §7.4.4.
    assert_eq!(
        only_paragraph("{danger: Texto rojo}"),
        vec![Inline::Color {
            foreground: Some("danger".into()),
            background: None,
            text: "Texto rojo".into(),
        }]
    );
    assert_eq!(
        only_paragraph("{+highlight: Texto resaltado}"),
        vec![Inline::Color {
            foreground: None,
            background: Some("highlight".into()),
            text: "Texto resaltado".into(),
        }]
    );
    assert_eq!(
        only_paragraph("{accent+highlight: Texto con color y fondo}"),
        vec![Inline::Color {
            foreground: Some("accent".into()),
            background: Some("highlight".into()),
            text: "Texto con color y fondo".into(),
        }]
    );
}

#[test]
fn atributos_de_imagen() {
    // §18.5, §26.2, §7.4.3 posición 1.
    let content = only_paragraph("![x](y.png){width=50%}\n");
    let Inline::Image { attrs, .. } = &content[0] else {
        panic!("esperaba imagen: {content:?}");
    };
    assert_eq!(attrs.as_ref().unwrap().width_percent, Some(50));

    let content = only_paragraph("![Arquitectura](assets/architecture.png) {#fig:architecture}\n");
    let Inline::Image { attrs, .. } = &content[0] else {
        panic!("esperaba imagen: {content:?}");
    };
    assert_eq!(attrs.as_ref().unwrap().id.as_deref(), Some("fig:architecture"));
}

#[test]
fn escapes_7_4_6() {
    // Tabla normativa de §7.4.6.
    assert_eq!(only_paragraph("{{toc}}"), vec![Inline::Substitution(Substitution::Toc)]);
    assert_eq!(only_paragraph("{{foo}}"), vec![Inline::Text("{{foo}}".into())]);
    assert_eq!(only_paragraph(r"\{{toc}}"), vec![Inline::Text("{{toc}}".into())]);
    assert_eq!(
        only_paragraph(r"\{danger: x}"),
        vec![Inline::Text("{danger: x}".into())]
    );
}

// --- Negativos ---------------------------------------------------------------

#[test]
fn sustituciones_fuera_de_vocabulario_son_literales() {
    for source in ["{{foo}}", "{{ref:}}", "{{ref:con espacios}}", "{{}}"] {
        let content = only_paragraph(source);
        assert!(
            content.iter().all(|i| matches!(i, Inline::Text(_))),
            "{source} no debía ser sintaxis: {content:?}"
        );
    }
}

#[test]
fn colores_invalidos_son_literales() {
    // §23: nombre inexistente en las paletas → toda la construcción literal.
    for source in ["{peligro: x}", "{+fondo: x}", "{danger: sin cerrar", "{danger}"] {
        let content = only_paragraph(source);
        assert!(
            content.iter().all(|i| matches!(i, Inline::Text(_))),
            "{source} no debía ser sintaxis: {content:?}"
        );
    }
    // Variables de plantilla solo en headers/footers (§7.4.5).
    assert!(only_paragraph("{title}")
        .iter()
        .all(|i| matches!(i, Inline::Text(_))));
}

#[test]
fn atributos_fuera_de_posicion_son_literales() {
    // §7.4.3: solo las tres posiciones declaradas.
    for source in [
        "texto {#id} en medio\n",
        "![x](y.png) intermedio {#id} después\n",
        "{#id}\n",
    ] {
        let content = only_paragraph(source);
        assert!(
            content.iter().any(|i| matches!(i, Inline::Text(t) if t.contains('{'))),
            "{source} no debía ser sintaxis: {content:?}"
        );
    }
    // §18.5: width fuera de 1–100 no es atributo.
    let content = only_paragraph("![x](y.png) {width=150%}\n");
    let Inline::Image { attrs, .. } = &content[0] else {
        panic!("esperaba imagen");
    };
    assert_eq!(attrs, &None);
}

#[test]
fn code_primero_sobre_todo() {
    // §7.4.1: code spans y fences ganan a cualquier extensión.
    let content = only_paragraph("`{danger: x} [@a] $x$ {{toc}}`\n");
    assert_eq!(
        content,
        vec![Inline::Code("{danger: x} [@a] $x$ {{toc}}".into())]
    );
}

#[test]
fn wiki_links_desactivados_por_defecto() {
    // §19.3.
    let content = only_paragraph("[[architecture#database]]\n");
    assert!(content.iter().all(|i| matches!(i, Inline::Text(_))));

    let options = ParseOptions {
        wiki_links: true,
        ..Default::default()
    };
    let ast = parse_with("[[architecture#database]]\n", &options);
    let BlockKind::Paragraph { content } = &ast.blocks[0].kind else {
        panic!("esperaba párrafo");
    };
    assert!(content.contains(&Inline::WikiLink {
        target: "architecture#database".into()
    }));
}

#[test]
fn llamadas_validas_con_opciones_de_proyecto() {
    // §22/§23: tipos y colores personalizados desde `.mdproject.toml` (§58.2).
    let custom = ["quiz".to_string(), "brand".to_string()];
    let options = ParseOptions {
        custom_callout_types: &custom[..1],
        custom_colors: &custom[1..],
        wiki_links: false,
    };

    let ast = parse_with("::: quiz\nx\n:::\n", &options);
    assert!(matches!(
        &ast.blocks[0].kind,
        BlockKind::Callout { callout_type, .. } if callout_type == "quiz"
    ));

    let ast = parse_with("{brand: texto}\n", &options);
    let BlockKind::Paragraph { content } = &ast.blocks[0].kind else {
        panic!("esperaba párrafo");
    };
    assert_eq!(
        *content,
        vec![Inline::Color {
            foreground: Some("brand".into()),
            background: None,
            text: "texto".into(),
        }]
    );
}

#[test]
fn el_parseo_no_modifica_el_fuente() {
    // §6.2: cambiar de modo (o parsear) nunca reformatea el Markdown.
    let source = "# T  \n\n{danger: x}  \n*énfasis*\n\n| a |\n|---|\n| 1 |\n";
    let _ = parse(source);
    assert_eq!(
        source,
        "# T  \n\n{danger: x}  \n*énfasis*\n\n| a |\n|---|\n| 1 |\n"
    );
}

#[test]
fn atributos_completos_con_caption() {
    // §26.3: línea propia tras tabla.
    let ast = parse("| N | V |\n|---|---|\n| A | 10 |\n\n{#tbl:results \"Resultados del experimento\"}\n");
    let BlockKind::Table { attrs, .. } = &ast.blocks[0].kind else {
        panic!("esperaba tabla");
    };
    assert_eq!(
        attrs,
        &Some(Attributes {
            id: Some("tbl:results".into()),
            caption: Some("Resultados del experimento".into()),
            width_percent: None,
            extra: vec![],
        })
    );
    assert_eq!(ast.blocks.len(), 1);
}
