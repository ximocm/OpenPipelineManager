// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Paridad AST ↔ escáner de anclas (§26): cada heading que renderiza la AST
//! está en la misma línea que su [`opm_document::HeadingNode`].
//!
//! El Preview asigna `id` a los headings por línea de inicio del bloque (mapa
//! de [`opm_document::find_heading_nodes`]); este invariante garantiza que
//! el mapa cae siempre sobre los headings renderizados, con los mismos
//! destinos que resuelve el índice.

use opm_document::{find_heading_nodes, Block, BlockKind};
use opm_parser::parse;

/// Líneas (1-based) de los headings de la AST, en orden, recursivamente.
fn ast_heading_lines(source: &str) -> Vec<u32> {
    fn walk(blocks: &[Block], source: &str, out: &mut Vec<u32>) {
        for block in blocks {
            match &block.kind {
                BlockKind::Heading { .. } => {
                    let offset = block.span.start as usize;
                    out.push(source[..offset].matches('\n').count() as u32 + 1);
                }
                BlockKind::Quote { blocks } | BlockKind::Callout { blocks, .. } => {
                    walk(blocks, source, out)
                }
                BlockKind::List { items, .. } => {
                    for item in items {
                        walk(&item.blocks, source, out);
                    }
                }
                _ => {}
            }
        }
    }
    let ast = parse(source);
    let mut out = Vec::new();
    walk(&ast.blocks, source, &mut out);
    out
}

#[test]
fn cada_heading_del_ast_tiene_su_nodo_escaneado_26() {
    let corpus = [
        "# ATX\n\nSetext\n===\n\npárrafo\ncontinuación\n---\n",
        "> # Citado\n\n- item\n\n  # En lista\n",
        "---\ntitle: x\n---\n# Con front matter\n",
        "```\n# en codigo\n```\n# Real\n",
        "## Título {#id} ###\n\n#hashtag no\n",
        "# Ñandú 見出し\r\n\r\n## Segunda\r\n",
        "# **Fuerte** y [enlace](x.md)\n",
        "1.1 Revisión\n---\n",
    ];
    for source in corpus {
        let ast_lines = ast_heading_lines(source);
        let scanned: Vec<u32> = find_heading_nodes(source)
            .into_iter()
            .map(|node| node.line)
            .collect();
        assert_eq!(ast_lines, scanned, "AST y escáner divergen en {source:?}");
    }
}
