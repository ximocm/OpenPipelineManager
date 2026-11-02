// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Conversión de eventos pulldown-cmark a la AST (base CommonMark + GFM, §7.1).
//!
//! Los runs de texto plano se escanean sobre el fuente crudo ([`scan_run`]):
//! los eventos de texto se fusionan por continuidad y se analizan con la
//! gramática §7.4. Los constructos de bloque extendidos (callouts `:::`,
//! math `$$`) y el front matter no llegan aquí: los absorbe la segmentación
//! de `blocks`.

use crate::highlight::{Highlight, HighlightKind};
use crate::inlines::scan_run;
use crate::ParseOptions;
use opm_document::{Align, Block, BlockKind, Inline, ListItem, Span};
use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, OffsetIter, Options, Parser, Tag, TagEnd,
};
use std::iter::Peekable;
use std::ops::Range;

type Item<'a> = (Event<'a>, Range<usize>);

pub(crate) fn parse_segment(
    source: &str,
    base: usize,
    options: &ParseOptions,
) -> (Vec<Block>, Vec<Highlight>) {
    let mut pulldown_options = Options::empty();
    pulldown_options.insert(Options::ENABLE_TABLES);
    pulldown_options.insert(Options::ENABLE_STRIKETHROUGH);
    pulldown_options.insert(Options::ENABLE_TASKLISTS);
    pulldown_options.insert(Options::ENABLE_FOOTNOTES);

    let iter = Parser::new_ext(source, pulldown_options).into_offset_iter();
    let mut state = State {
        source,
        iter: iter.peekable(),
        last_end: 0,
        base,
        options,
        tokens: Vec::new(),
    };
    let blocks = state.parse_blocks();
    (blocks, state.tokens)
}

struct State<'a> {
    source: &'a str,
    iter: Peekable<OffsetIter<'a>>,
    /// Fin del último evento consumido (para calcular spans).
    last_end: usize,
    /// Offset del segmento dentro del fuente completo.
    base: usize,
    options: &'a ParseOptions<'a>,
    /// Tokens de highlighting (ISSUE-015).
    tokens: Vec<Highlight>,
}

impl<'a> State<'a> {
    fn next(&mut self) -> Option<Item<'a>> {
        let item = self.iter.next();
        if let Some((_, range)) = &item {
            self.last_end = range.end;
        }
        item
    }

    fn peek(&mut self) -> Option<Item<'a>> {
        self.iter.peek().cloned()
    }

    fn span_from(&self, start: usize) -> Span {
        Span {
            start: (self.base + start) as u32,
            end: (self.base + self.last_end) as u32,
        }
    }

    /// Registra un token de highlighting del constructo que arranca en
    /// `start` (offset del segmento) y termina en el último evento consumido.
    fn push_token(&mut self, start: usize, kind: HighlightKind) {
        self.tokens.push(Highlight {
            span: self.span_from(start),
            kind,
        });
    }

    /// Recoge bloques hasta el `End` del contenedor actual (que consume) o EOF.
    ///
    /// El contenido inline suelto a nivel de bloque —ítems de lista ajustada
    /// (tight), que no envuelven su contenido en `Tag::Paragraph`— forma un
    /// párrafo implícito.
    fn parse_blocks(&mut self) -> Vec<Block> {
        let mut blocks = Vec::new();
        while let Some((event, range)) = self.peek() {
            match event {
                Event::End(_) => {
                    self.next();
                    break;
                }
                Event::Rule => {
                    self.next();
                    blocks.push(Block {
                        span: self.span_from(range.start),
                        kind: BlockKind::Rule,
                    });
                }
                Event::Start(Tag::Paragraph) => {
                    self.next();
                    let content = self.parse_inlines();
                    self.next(); // End(Paragraph)
                    blocks.push(Block {
                        span: self.span_from(range.start),
                        kind: BlockKind::Paragraph { content },
                    });
                }
                Event::Start(Tag::FootnoteDefinition(label)) => {
                    self.next();
                    let inner = self.parse_blocks();
                    blocks.push(Block {
                        span: self.span_from(range.start),
                        kind: BlockKind::FootnoteDefinition {
                            label: opm_document::normalize_label(&label),
                            blocks: inner,
                        },
                    });
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    self.next();
                    let content = self.parse_inlines();
                    self.next(); // End(Heading)
                    blocks.push(Block {
                        span: self.span_from(range.start),
                        kind: BlockKind::Heading {
                            level: heading_level(level),
                            content,
                            attrs: None,
                        },
                    });
                }
                Event::Start(Tag::BlockQuote(..)) => {
                    self.next();
                    let inner = self.parse_blocks();
                    blocks.push(Block {
                        span: self.span_from(range.start),
                        kind: BlockKind::Quote { blocks: inner },
                    });
                }
                Event::Start(Tag::List(start)) => {
                    self.next();
                    let block = self.parse_list(range.start, start);
                    blocks.push(block);
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    self.next();
                    let (language, code) = self.parse_code_block(kind);
                    blocks.push(Block {
                        span: self.span_from(range.start),
                        kind: BlockKind::CodeBlock { language, code },
                    });
                }
                Event::Start(Tag::Table(aligns)) => {
                    self.next();
                    let block = self.parse_table(range.start, &aligns);
                    blocks.push(block);
                }
                Event::Start(Tag::HtmlBlock) => {
                    self.next();
                    // §7.2: el HTML nunca se interpreta, se preserva como texto.
                    let raw = self.collect_raw_until_end();
                    blocks.push(Block {
                        span: self.span_from(range.start),
                        kind: BlockKind::Paragraph {
                            content: vec![Inline::Text(raw)],
                        },
                    });
                }
                // HTML suelto a nivel de bloque: mismo tratamiento (§7.2).
                Event::Html(raw) => {
                    self.next();
                    blocks.push(Block {
                        span: self.span_from(range.start),
                        kind: BlockKind::Paragraph {
                            content: vec![Inline::Text(raw.to_string())],
                        },
                    });
                }
                _ => {
                    let content = self.parse_inlines();
                    if !content.is_empty() {
                        blocks.push(Block {
                            span: self.span_from(range.start),
                            kind: BlockKind::Paragraph { content },
                        });
                    }
                }
            }
        }
        blocks
    }

    fn parse_list(&mut self, start: usize, first: Option<u64>) -> Block {
        let mut items = Vec::new();
        while let Some((event, range)) = self.peek() {
            match event {
                Event::Start(Tag::Item) => {
                    self.next();
                    let blocks = self.parse_blocks();
                    items.push(ListItem {
                        span: self.span_from(range.start),
                        blocks,
                    });
                }
                Event::End(_) => {
                    self.next();
                    break;
                }
                _ => {
                    self.next();
                }
            }
        }
        Block {
            span: self.span_from(start),
            kind: BlockKind::List {
                ordered: first.is_some(),
                start: first.unwrap_or(1),
                items,
            },
        }
    }

    fn parse_code_block(&mut self, kind: CodeBlockKind<'a>) -> (Option<String>, String) {
        // §7.4.1: dentro de code spans y fences no se interpreta ninguna
        // extensión; el contenido se copia literal.
        let language = match kind {
            CodeBlockKind::Fenced(lang) if !lang.trim().is_empty() => {
                Some(lang.trim().to_string())
            }
            _ => None,
        };
        let mut code = String::new();
        while let Some((event, _)) = self.next() {
            match event {
                Event::End(_) => break,
                Event::Text(text) => code.push_str(&text),
                Event::SoftBreak | Event::HardBreak => code.push('\n'),
                _ => {}
            }
        }
        (language, code)
    }

    fn parse_table(&mut self, start: usize, aligns: &[Alignment]) -> Block {
        let aligns = aligns.iter().map(align).collect();
        let mut header: Vec<Vec<Inline>> = Vec::new();
        let mut rows: Vec<Vec<Vec<Inline>>> = Vec::new();
        while let Some((event, _)) = self.peek() {
            match event {
                Event::Start(Tag::TableHead) => {
                    self.next();
                    header = self.parse_table_cells(true);
                }
                Event::Start(Tag::TableRow) => {
                    self.next();
                    let row = self.parse_table_cells(false);
                    rows.push(row);
                }
                Event::End(_) => {
                    self.next();
                    break;
                }
                _ => {
                    self.next();
                }
            }
        }
        Block {
            span: self.span_from(start),
            kind: BlockKind::Table {
                aligns,
                header,
                rows,
                attrs: None,
            },
        }
    }

    /// Recoge las celdas de la fila actual y consume el `End` que la cierra.
    /// `swallow_row_end` absorbe el `End(TableRow)` cuando la fila está
    /// envuelta en `TableHead`.
    fn parse_table_cells(&mut self, swallow_row_end: bool) -> Vec<Vec<Inline>> {
        let mut cells = Vec::new();
        while let Some((event, _)) = self.next() {
            match event {
                Event::Start(Tag::TableCell) => {
                    let content = self.parse_inlines();
                    self.next(); // End(TableCell)
                    cells.push(content);
                }
                Event::End(TagEnd::TableRow) if swallow_row_end => continue,
                Event::End(_) => break,
                _ => {}
            }
        }
        cells
    }

    /// Recoge inlines hasta el `End` del contenedor actual. **No** consume ese
    /// `End`: lo consume el dueño del contenedor.
    fn parse_inlines(&mut self) -> Vec<Inline> {
        let mut inlines = Vec::new();
        while let Some((event, range)) = self.peek() {
            if stops_inline_run(&event) {
                break;
            }
            match event {
                // Runs de texto plano: se fusionan por continuidad y se
                // analizan con la gramática §7.4 sobre el fuente crudo.
                Event::FootnoteReference(label) => {
                    self.next();
                    inlines.push(Inline::FootnoteReference(opm_document::normalize_label(&label)));
                }
                Event::Text(_) | Event::SoftBreak | Event::HardBreak => {
                    // pulldown-cmark excluye el backslash de los escapes en sus
                    // rangos de offset: el escape puede empezar justo antes del
                    // run y §7.4.6 exige verlo para no iniciar sintaxis.
                    let mut start = range.start;
                    while start > 0 && self.source.as_bytes()[start - 1] == b'\\' {
                        start -= 1;
                    }
                    let mut end = range.end;
                    loop {
                        self.next();
                        match self.peek() {
                            Some((Event::Text(_), r))
                            | Some((Event::SoftBreak, r))
                            | Some((Event::HardBreak, r)) => end = r.end,
                            _ => break,
                        }
                    }
                    let source = self.source;
                    let options = self.options;
                    inlines.extend(scan_run(
                        &source[start..end],
                        self.base + start,
                        options,
                        &mut self.tokens,
                    ));
                }
                Event::Start(Tag::Emphasis) => {
                    self.next();
                    let content = self.parse_inlines();
                    self.next(); // End(Emphasis)
                    self.push_token(range.start, HighlightKind::Emphasis);
                    inlines.push(Inline::Emphasis(content));
                }
                Event::Start(Tag::Strong) => {
                    self.next();
                    let content = self.parse_inlines();
                    self.next(); // End(Strong)
                    self.push_token(range.start, HighlightKind::Strong);
                    inlines.push(Inline::Strong(content));
                }
                Event::Start(Tag::Strikethrough) => {
                    self.next();
                    let content = self.parse_inlines();
                    self.next(); // End(Strikethrough)
                    self.push_token(range.start, HighlightKind::Strikethrough);
                    inlines.push(Inline::Strikethrough(content));
                }
                Event::Start(Tag::Link {
                    dest_url, title, ..
                }) => {
                    self.next();
                    let content = self.parse_inlines();
                    self.next(); // End(Link)
                    self.push_token(range.start, HighlightKind::Link);
                    inlines.push(Inline::Link {
                        destination: dest_url.to_string(),
                        title: title.to_string(),
                        content,
                    });
                }
                Event::Start(Tag::Image {
                    dest_url, title, ..
                }) => {
                    self.next();
                    let alt = self.parse_inlines();
                    self.next(); // End(Image)
                    self.push_token(range.start, HighlightKind::Image);
                    inlines.push(Inline::Image {
                        destination: dest_url.to_string(),
                        title: title.to_string(),
                        alt,
                        attrs: None,
                    });
                }
                Event::Code(code) => {
                    self.next();
                    self.push_token(range.start, HighlightKind::Code);
                    inlines.push(Inline::Code(code.to_string()));
                }
                Event::TaskListMarker(checked) => {
                    self.next();
                    inlines.push(Inline::TaskListMarker(checked));
                }
                // §7.2: HTML embebido = texto literal, nunca contenido activo.
                Event::Html(raw) | Event::InlineHtml(raw) => {
                    self.next();
                    inlines.push(Inline::Text(raw.to_string()));
                }
                _ => {
                    self.next();
                }
            }
        }
        inlines
    }

    /// Concatena el contenido textual del contenedor actual y consume su `End`.
    fn collect_raw_until_end(&mut self) -> String {
        let mut text = String::new();
        while let Some((event, _)) = self.next() {
            match event {
                Event::End(_) => break,
                Event::Text(raw) | Event::Html(raw) | Event::InlineHtml(raw) => text.push_str(&raw),
                Event::SoftBreak | Event::HardBreak => text.push('\n'),
                _ => {}
            }
        }
        text
    }
}

/// Un run inline termina antes de un `End` del contenedor (que no consume) o
/// de cualquier evento de nivel de bloque.
fn stops_inline_run(event: &Event) -> bool {
    matches!(
        event,
        Event::End(_)
            | Event::Rule
            | Event::Start(
                Tag::Paragraph
                    | Tag::Heading { .. }
                    | Tag::BlockQuote(..)
                    | Tag::CodeBlock(_)
                    | Tag::HtmlBlock
                    | Tag::List(_)
                    | Tag::Item
                    | Tag::Table(_)
                    | Tag::TableHead
                    | Tag::TableRow
                    | Tag::TableCell
                    | Tag::MetadataBlock(_)
            )
    )
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn align(alignment: &Alignment) -> Align {
    match alignment {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_segment(source: &str) -> Vec<Block> {
        let options = ParseOptions::default();
        super::parse_segment(source, 0, &options).0
    }

    #[test]
    fn heading_y_parrafo_con_spans() {
        let blocks = parse_segment("# H\n\ntext\n");
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0].kind,
            BlockKind::Heading {
                level: 1,
                content: vec![Inline::Text("H".to_string())],
                attrs: None,
            }
        );
        // El span cubre el bloque incluido su salto de línea final.
        assert_eq!(blocks[0].span, Span { start: 0, end: 4 });
        assert_eq!(
            blocks[1].kind,
            BlockKind::Paragraph {
                content: vec![Inline::Text("text".to_string())],
            }
        );
        assert_eq!(blocks[1].span, Span { start: 5, end: 10 });
    }

    #[test]
    fn listas_y_citas_anidadas() {
        let blocks = parse_segment("- uno\n- dos\n");
        let BlockKind::List {
            ordered,
            start,
            items,
        } = &blocks[0].kind
        else {
            panic!("esperaba lista: {:?}", blocks[0]);
        };
        assert!(!ordered);
        assert_eq!(*start, 1);
        assert_eq!(items.len(), 2);
        // Ítem ajustado (tight): párrafo implícito.
        assert_eq!(
            items[0].blocks[0].kind,
            BlockKind::Paragraph {
                content: vec![Inline::Text("uno".to_string())],
            }
        );

        let blocks = parse_segment("3. tres\n");
        let BlockKind::List {
            ordered,
            start,
            items,
        } = &blocks[0].kind
        else {
            panic!("esperaba lista: {:?}", blocks[0]);
        };
        assert!(*ordered);
        assert_eq!(*start, 3);
        assert_eq!(items.len(), 1);

        let blocks = parse_segment("> cita\n");
        let BlockKind::Quote { blocks } = &blocks[0].kind else {
            panic!("esperaba cita: {:?}", blocks[0]);
        };
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn codigo_no_interpreta_extensiones() {
        let blocks = parse_segment("```md\n{danger: x} {{toc}}\n```\n");
        assert_eq!(
            blocks[0].kind,
            BlockKind::CodeBlock {
                language: Some("md".to_string()),
                code: "{danger: x} {{toc}}\n".to_string(),
            }
        );
    }

    #[test]
    fn html_nunca_se_interpreta() {
        // §7.2: el HTML se preserva como texto, sin ejecutar nada.
        let blocks = parse_segment("<script>alert(1)</script>\n");
        let BlockKind::Paragraph { content } = &blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", blocks[0]);
        };
        assert_eq!(
            content,
            &[Inline::Text("<script>alert(1)</script>\n".to_string())]
        );

        let blocks = parse_segment("texto <b>negrita</b>\n");
        let BlockKind::Paragraph { content } = &blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", blocks[0]);
        };
        assert!(content
            .iter()
            .any(|i| matches!(i, Inline::Text(t) if t.contains("<b>"))));
    }

    #[test]
    fn tabla_gfm_con_alineaciones() {
        let blocks = parse_segment("| A | B |\n|---|:-:|\n| 1 | 2 |\n");
        let BlockKind::Table {
            aligns,
            header,
            rows,
            attrs: None,
        } = &blocks[0].kind
        else {
            panic!("esperaba tabla: {:?}", blocks[0]);
        };
        assert_eq!(*aligns, vec![Align::None, Align::Center]);
        assert_eq!(header.len(), 2);
        assert_eq!(header[0], vec![Inline::Text("A".to_string())]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], vec![Inline::Text("2".to_string())]);
    }

    #[test]
    fn tareas_y_strikethrough_gfm() {
        let blocks = parse_segment("- [x] hecho\n");
        let BlockKind::List { items, .. } = &blocks[0].kind else {
            panic!("esperaba lista: {:?}", blocks[0]);
        };
        let BlockKind::Paragraph { content } = &items[0].blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", items[0].blocks[0]);
        };
        assert_eq!(content[0], Inline::TaskListMarker(true));

        let blocks = parse_segment("~~viejo~~\n");
        let BlockKind::Paragraph { content } = &blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", blocks[0]);
        };
        assert_eq!(
            content[0],
            Inline::Strikethrough(vec![Inline::Text("viejo".to_string())])
        );
    }

    #[test]
    fn enlaces_e_imagenes() {
        let blocks = parse_segment("[t](u \"ti\") ![alt](img.png)\n");
        let BlockKind::Paragraph { content } = &blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", blocks[0]);
        };
        assert!(content.iter().any(|i| matches!(
            i,
            Inline::Link { destination, title, content }
                if destination == "u" && title == "ti"
                    && content == &[Inline::Text("t".to_string())]
        )));
        assert!(content.iter().any(|i| matches!(
            i,
            Inline::Image { destination, alt, attrs: None, .. }
                if destination == "img.png"
                    && alt == &[Inline::Text("alt".to_string())]
        )));
    }

    #[test]
    fn extensiones_fuera_de_sitio_son_texto_literal() {
        // Vocabulario cerrado (§7.4.1): sin posición válida, es texto literal.
        let blocks = parse_segment("texto {#a} más\n");
        let BlockKind::Paragraph { content } = &blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", blocks[0]);
        };
        assert!(content
            .iter()
            .any(|i| matches!(i, Inline::Text(t) if t.contains("{#a}"))));
    }

    #[test]
    fn color_inline_en_texto() {
        // §7.4.4: los colores son sintaxis de texto inline.
        let blocks = parse_segment("{danger: x}\n");
        let BlockKind::Paragraph { content } = &blocks[0].kind else {
            panic!("esperaba párrafo: {:?}", blocks[0]);
        };
        assert_eq!(
            content,
            &[Inline::Color {
                foreground: Some("danger".into()),
                background: None,
                text: "x".into(),
            }]
        );
    }
}
