// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Plan visual del modo Visual (ISSUE-030, §8.2–§8.4).
//!
//! Derivado de la MISMA gramática que la AST y los tokens de highlighting
//! (parser único, §4.2): rangos del fuente original que la UI traduce a tags
//! del `TextBuffer` único (§6.1). Los tramos con `style: None` son sintaxis
//! ocultable (§8.3); los que llevan estilo son contenido semántico (§8.2).
//! El plan no reescribe ni reformatea nada: Visual es una representación
//! directa del buffer y la fidelidad del fuente (§6.2) queda garantizada.
//!
//! Lo que este plan no cubre (math §24, citas §25, wiki links §19.3,
//! sustituciones §7.4.2, …) queda como bloque Markdown editable (§8.4): no
//! produce tramos y su sintaxis permanece visible.

use crate::highlight::{Highlight, HighlightKind};
use crate::ParseOptions;
use opm_document::{Ast, Block, BlockKind, Span};

/// Tono semántico de un callout (§22), con su color de la paleta §23.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalloutTone {
    Note,
    Info,
    Tip,
    Warning,
    Danger,
    Success,
    /// Tipos desconocidos o personalizados (§22 extensible): color neutro.
    Neutral,
}

/// Estilo semántico del contenido de un tramo visual (§8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualStyle {
    /// Contenido de heading, con nivel 1..=6 (§27.1).
    Heading(u8),
    /// Negrita `**…**` / `__…__`.
    Strong,
    /// Cursiva `*…*` / `_…_`.
    Emphasis,
    /// Texto de enlace `[texto](destino "título")` (§19.1).
    Link,
    /// Contenido de code span `` `…` `` (§7.4.1).
    Code,
    /// Contenido de bloque de código cercado (§7.4.1).
    CodeBlock,
    /// Contenido de línea de cita `> …` (§8.2).
    Quote,
    /// Contenido de ítem de lista, con nivel de anidamiento 1..=6 (§8.2).
    ListItem(u8),
    /// Marcador de tarea GFM `[ ]`/`[x]`, visible como marcador semántico
    /// (decisión ISSUE-032: ocultarlo perdería el estado).
    Task,
    /// Celda de cabecera de tabla (§35.1).
    TableHeader,
    /// Fila delimitadora de tabla, visible como línea separadora con estilo
    /// de regla (§35.1).
    TableRule,
    /// Chip/etiqueta semántica del `alt` de una imagen (§18).
    ImageAlt,
    /// Línea semántica `tipo "título"` de un callout (§22), con el color del
    /// tono del tipo.
    CalloutHeader(CalloutTone),
    /// Contenido de callout, con sangrado y fondo (§22).
    CalloutBody,
    /// Texto con color de primer plano `{nombre: …}` (§7.4.4): slot de la
    /// paleta §23 (`0..=16`) o de los colores personalizados (`17` + índice).
    ColorFg(u8),
    /// Texto con color de fondo `{+nombre: …}` (§7.4.4), mismo slot que
    /// [`VisualStyle::ColorFg`].
    ColorBg(u8),
}

/// Tramo de la representación visual sobre el fuente original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualSpan {
    pub span: Span,
    /// `Some(estilo)` = contenido semántico visible; `None` = sintaxis
    /// ocultable (§8.3: oculta siempre o revelada en el elemento editado).
    pub style: Option<VisualStyle>,
}

impl VisualSpan {
    /// `true` si el tramo es sintaxis (marcador ocultable).
    pub fn is_syntax(&self) -> bool {
        self.style.is_none()
    }
}

/// Plan visual completo del documento.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VisualPlan {
    /// Tramos en orden de aparición. Pueden solaparse (p. ej. `**` dentro de
    /// un heading): se aplican todos, y ocultar gana sobre mostrar.
    pub spans: Vec<VisualSpan>,
    /// Spans de los elementos (bloques) editables, recursivo: §8.3
    /// «reveal-current» revela el elemento más interno bajo el cursor.
    pub elements: Vec<Span>,
}

/// Plan visual del documento con las extensiones configurables (§7.3).
pub(crate) fn build(source: &str, options: &ParseOptions) -> VisualPlan {
    let (ast, highlights) = crate::blocks::parse_document(source, 0, options, true);
    from_parts(source, &ast, &highlights, options)
}

/// Construye el plan a partir de la AST y los tokens ya parseados.
fn from_parts(
    source: &str,
    ast: &Ast,
    highlights: &[Highlight],
    options: &ParseOptions,
) -> VisualPlan {
    let mut plan = VisualPlan::default();
    collect_elements(&ast.blocks, &mut plan.elements);
    for token in highlights {
        match token.kind {
            HighlightKind::Heading => heading_spans(source, token.span, &mut plan.spans),
            HighlightKind::Strong => {
                delimited_spans(source, token.span, VisualStyle::Strong, &mut plan.spans)
            }
            HighlightKind::Emphasis => {
                delimited_spans(source, token.span, VisualStyle::Emphasis, &mut plan.spans)
            }
            HighlightKind::Link => link_spans(source, token.span, &mut plan.spans),
            HighlightKind::Code => {
                delimited_spans(source, token.span, VisualStyle::Code, &mut plan.spans)
            }
            HighlightKind::CodeBlock => code_block_spans(source, token.span, &mut plan.spans),
            HighlightKind::Quote => quote_spans(source, token.span, &mut plan.spans),
            HighlightKind::List => list_spans(source, token.span, &mut plan.spans),
            HighlightKind::Table => table_spans(source, token.span, &mut plan.spans),
            HighlightKind::Image => image_spans(source, token.span, &mut plan.spans),
            HighlightKind::Callout => callout_spans(source, token.span, &mut plan.spans),
            HighlightKind::Color => color_spans(source, token.span, options, &mut plan.spans),
            // §8.4: el resto (math §24, citas §25, wiki links §19.3,
            // sustituciones §7.4.2, front matter §21, …) queda como bloque
            // Markdown editable: sin tramos, con su sintaxis visible.
            _ => {}
        }
    }
    // Orden de aparición; los solapes exactos (p. ej. el marcador de una
    // lista anidada, procesado por el token de la lista y por el de la lista
    // contenedora) se deduplican.
    plan.spans
        .sort_by_key(|span| (span.span.start, span.span.end));
    plan.spans.dedup();
    plan
}

/// Recoge los spans de todos los bloques, recursivo (quotes, ítems de lista y
/// callouts envuelven bloques propios).
fn collect_elements(blocks: &[Block], out: &mut Vec<Span>) {
    for block in blocks {
        out.push(block.span);
        match &block.kind {
            BlockKind::Quote { blocks } | BlockKind::Callout { blocks, .. } => {
                collect_elements(blocks, out);
            }
            BlockKind::List { items, .. } => {
                for item in items {
                    collect_elements(&item.blocks, out);
                }
            }
            _ => {}
        }
    }
}

/// Tramos de un heading (token = el bloque entero, marcadores incluidos):
/// marcadores `#…` y la línea de atributos ` {…}` (§7.4.3) son sintaxis; el
/// resto es contenido con el nivel del heading (§27.1).
fn heading_spans(source: &str, span: Span, out: &mut Vec<VisualSpan>) {
    let Some(text) = slice(source, span) else { return };
    let chars: Vec<char> = text.chars().collect();
    let base = span.start;

    // Marcadores ATX `### ` (§27.1); sin `#`, el heading es setext (línea de
    // `===`/`---` al final, que también es sintaxis).
    let hashes = chars.iter().take_while(|c| **c == '#').count();
    if (1..=6).contains(&hashes) {
        let mut end = hashes;
        while end < chars.len() && (chars[end] == ' ' || chars[end] == '\t') {
            end += 1;
        }
        push_syntax(out, base, base + offset_of(text, end));
        let content_start = end;
        let mut content_end = chars.len();
        // Salto de línea final fuera del contenido estilizado.
        while content_end > content_start && chars[content_end - 1] == '\n' {
            content_end -= 1;
        }
        // Atributos de heading ` {#id …}` (§7.4.3, §26.1): sintaxis al final
        // de la línea de contenido (con el `\n` que los sigue).
        if let Some(rel) = trailing_attrs_start(&chars[content_start..content_end]) {
            content_end = content_start + rel;
            push_syntax(
                out,
                base + offset_of(text, content_end),
                base + offset_of(text, chars.len()),
            );
        }
        if content_end > content_start {
            out.push(VisualSpan {
                span: byte_span(base, text, content_start, content_end),
                style: Some(VisualStyle::Heading(hashes as u8)),
            });
        }
        return;
    }

    // Setext: primera línea = contenido; línea de subrayado = sintaxis.
    let Some(line1_end) = chars.iter().position(|c| *c == '\n') else {
        return;
    };
    let underline: Vec<char> = chars[line1_end + 1..]
        .iter()
        .copied()
        .filter(|c| *c != '\n')
        .collect();
    let is_underline = !underline.is_empty()
        && (underline.iter().all(|c| *c == '=') || underline.iter().all(|c| *c == '-'));
    if !is_underline {
        return;
    }
    if line1_end > 0 {
        out.push(VisualSpan {
            span: byte_span(base, text, 0, line1_end),
            style: Some(VisualStyle::Heading(1)),
        });
    }
    push_syntax(
        out,
        base + offset_of(text, line1_end + 1),
        base + offset_of(text, chars.len()),
    );
}

/// Tramos de un constructo delimitado por marcadores repetidos (token =
/// `**negrita**`, `` `código` `` … completo): los marcadores de ambos extremos
/// son sintaxis; el interior es contenido.
fn delimited_spans(source: &str, span: Span, style: VisualStyle, out: &mut Vec<VisualSpan>) {
    let Some(text) = slice(source, span) else { return };
    let chars: Vec<char> = text.chars().collect();
    let Some(&marker) = chars.first() else { return };
    let base = span.start;
    let lead = chars.iter().take_while(|c| **c == marker).count();
    let trail = chars.iter().rev().take_while(|c| **c == marker).count();
    if lead + trail >= chars.len() {
        return;
    }
    push_syntax(out, base, base + offset_of(text, lead));
    push_syntax(
        out,
        base + offset_of(text, chars.len() - trail),
        base + offset_of(text, chars.len()),
    );
    out.push(VisualSpan {
        span: byte_span(base, text, lead, chars.len() - trail),
        style: Some(style),
    });
}

/// Tramos de un enlace (token = `[texto](destino "título")` completo, §19.1):
/// `[`, `](`, destino, título y `)` son sintaxis; el texto del enlace es
/// contenido. §8.4: un enlace sin cerrar no produce token y queda como
/// bloque Markdown editable.
fn link_spans(source: &str, span: Span, out: &mut Vec<VisualSpan>) {
    let Some(text) = slice(source, span) else { return };
    let chars: Vec<char> = text.chars().collect();
    if chars.first() != Some(&'[') {
        return;
    }
    // Primer `]` que cierra el texto del enlace (llaves anidadas balanceadas,
    // code spans §7.4.1 y escapes saltados); el parser garantiza que existe.
    let Some(close) = link_text_close(&chars) else { return };
    let base = span.start;
    push_syntax(out, base, base + offset_of(text, 1));
    if close > 1 {
        out.push(VisualSpan {
            span: byte_span(base, text, 1, close),
            style: Some(VisualStyle::Link),
        });
    }
    // `](destino "título")` entero = sintaxis.
    push_syntax(
        out,
        base + offset_of(text, close),
        base + offset_of(text, chars.len()),
    );
}

/// Índice del `]` que cierra el texto de un enlace que empieza en `chars[0] =
/// '['`; `None` si no hay cierre balanceado (§8.4: Markdown editable).
fn link_text_close(chars: &[char]) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = 1;
    while index < chars.len() {
        match chars[index] {
            // §7.4.6: el carácter escapado no inicia sintaxis.
            '\\' => index += 2,
            // §7.4.1: dentro de un code span nada se interpreta.
            '`' => {
                let run = chars[index..].iter().take_while(|c| **c == '`').count();
                // Run de cierre de exactamente la misma longitud.
                let mut close = index + run;
                let mut closed = None;
                while close + run <= chars.len() {
                    let exact = chars.get(close.wrapping_sub(1)) != Some(&'`')
                        && chars.get(close + run) != Some(&'`');
                    if exact && chars[close..close + run].iter().all(|c| *c == '`') {
                        closed = Some(close + run);
                        break;
                    }
                    close += 1;
                }
                index = closed?;
            }
            '[' => {
                depth += 1;
                index += 1;
            }
            ']' => {
                if depth == 0 {
                    return Some(index);
                }
                depth -= 1;
                index += 1;
            }
            _ => index += 1,
        }
    }
    None
}

/// Tramos de un bloque de código (token = el bloque entero, fences e
/// info-string incluidos, §7.4.1): las líneas de fence son sintaxis; el
/// contenido es monoespaciado y nunca se reinterpreta (§7.4.1). Los bloques
/// indentados no tienen marcadores que ocultar.
fn code_block_spans(source: &str, span: Span, out: &mut Vec<VisualSpan>) {
    let Some(text) = slice(source, span) else { return };
    let chars: Vec<char> = text.chars().collect();
    let base = span.start;
    let mut end = chars.len();
    while end > 0 && chars[end - 1] == '\n' {
        end -= 1;
    }
    if end == 0 {
        return;
    }
    let lines = line_ranges(&chars[..end]);
    let mut content_start = 0;
    let mut content_end = end;
    if fence_marker(&chars, lines[0]).is_some() {
        // Línea de apertura (fence + info-string) y su `\n`: sintaxis.
        let open_end = lines[0].1;
        push_syntax(out, base, base + offset_of(text, open_end + 1));
        content_start = open_end + 1;
        // Línea de cierre: solo marcadores, también sintaxis.
        if lines.len() > 1 && fence_marker(&chars, lines[lines.len() - 1]).is_some() {
            content_end = lines[lines.len() - 1].0;
            push_syntax(
                out,
                base + offset_of(text, content_end),
                base + offset_of(text, end),
            );
        }
    }
    if content_end > content_start {
        out.push(VisualSpan {
            span: byte_span(base, text, content_start, content_end),
            style: Some(VisualStyle::CodeBlock),
        });
    }
}

/// Tramos de una cita (token = el bloque de cita entero, §8.2): el marcador
/// `>` (con su indentación y un espacio opcional) de cada línea es sintaxis;
/// el resto de la línea es contenido con sangrado y color de cita.
fn quote_spans(source: &str, span: Span, out: &mut Vec<VisualSpan>) {
    let Some(text) = slice(source, span) else { return };
    let chars: Vec<char> = text.chars().collect();
    let base = span.start;
    let mut end = chars.len();
    while end > 0 && chars[end - 1] == '\n' {
        end -= 1;
    }
    for (line_start, line_end) in line_ranges(&chars[..end]) {
        let mut marker_end = line_start;
        while marker_end < line_end && chars[marker_end] == ' ' {
            marker_end += 1;
        }
        if chars.get(marker_end) != Some(&'>') {
            // Sin marcador (línea de continuación): todo el contenido.
            marker_end = line_start;
        } else {
            // Marcadores de todos los niveles (`> > …`): cada nivel oculta el
            // suyo, pero ocultar gana sobre mostrar y así ningún `>` queda
            // visible se solapen como se solapen los tokens anidados.
            while chars.get(marker_end) == Some(&'>') {
                marker_end += 1;
                if chars.get(marker_end) == Some(&' ') {
                    marker_end += 1;
                }
            }
        }
        push_syntax(
            out,
            base + offset_of(text, line_start),
            base + offset_of(text, marker_end),
        );
        if marker_end < line_end {
            out.push(VisualSpan {
                span: byte_span(base, text, marker_end, line_end),
                style: Some(VisualStyle::Quote),
            });
        }
    }
}

/// Tramos de una lista (token = la lista entera, marcadores incluidos, §8.2):
/// los marcadores `-`/`*`/`+`/`N.` con su espacio son sintaxis; la sangría de
/// anidamiento se conserva visible (define la estructura) y el contenido del
/// ítem recibe `ListItem` con sangría visual coherente con el nivel. Las
/// tareas GFM `[ ]`/`[x]` permanecen visibles con estilo propio (decisión
/// ISSUE-032: ocultarlas perdería el estado). El interior de bloques de
/// código no se toca (§7.4.1). §8.4: un marcador suelto sin contenido no
/// produce lista y queda como bloque Markdown editable.
fn list_spans(source: &str, span: Span, out: &mut Vec<VisualSpan>) {
    let Some(text) = slice(source, span) else { return };
    let chars: Vec<char> = text.chars().collect();
    let base = span.start;
    let mut end = chars.len();
    while end > 0 && chars[end - 1] == '\n' {
        end -= 1;
    }
    // Nivel del último marcador (las líneas de continuación heredan el nivel
    // del ítem; el nivel del marcador es función pura de su indentación para
    // que los tokens de listas anidadas no se contradigan al solaparse).
    let mut level: u8 = 1;
    let mut fence: Option<char> = None;
    for (line_start, line_end) in line_ranges(&chars[..end]) {
        let mut index = line_start;
        while index < line_end && chars[index] == ' ' {
            index += 1;
        }
        // Fence de cualquier indentación (§7.4.1): dentro de código nada se
        // interpreta, ni siquiera marcadores.
        if let Some(marker) = fence_run(&chars, index, line_end) {
            fence = match fence {
                Some(open) if open == marker => None,
                Some(open) => Some(open),
                None => Some(marker),
            };
            continue;
        }
        if fence.is_some() || index == line_end {
            continue;
        }
        if let Some(content_start) = list_marker(&chars, index, line_end) {
            level = (((index - line_start) / 2) + 1).min(6) as u8;
            push_syntax(
                out,
                base + offset_of(text, index),
                base + offset_of(text, content_start),
            );
            let mut item_start = content_start;
            // Tarea GFM: `[ ]`/`[x]`/`[X]` visibles con estilo propio.
            if chars.get(item_start) == Some(&'[')
                && matches!(chars.get(item_start + 1), Some(' ') | Some('x') | Some('X'))
                && chars.get(item_start + 2) == Some(&']')
            {
                out.push(VisualSpan {
                    span: byte_span(base, text, item_start, item_start + 3),
                    style: Some(VisualStyle::Task),
                });
                item_start += 3;
                if chars.get(item_start) == Some(&' ') {
                    item_start += 1;
                }
            }
            if item_start < line_end {
                out.push(VisualSpan {
                    span: byte_span(base, text, item_start, line_end),
                    style: Some(VisualStyle::ListItem(level)),
                });
            }
        } else if index < line_end && !marker_like(&chars, index, line_end) {
            // Línea de continuación del ítem actual. Un marcador sin
            // contenido no es continuación: §8.4 queda como Markdown.
            out.push(VisualSpan {
                span: byte_span(base, text, index, line_end),
                style: Some(VisualStyle::ListItem(level)),
            });
        }
    }
}

/// Run de fence (` ``` ` o `~~~`, ≥3) en la línea desde `index`; `None` si no
/// es fence. El marcador devuelto permite cerrar solo con el mismo carácter.
fn fence_run(chars: &[char], index: usize, line_end: usize) -> Option<char> {
    let marker = *chars.get(index)?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = chars[index..line_end].iter().take_while(|c| **c == marker).count();
    (run >= 3).then_some(marker)
}

/// `true` si la línea empieza como marcador de lista (`-`/`*`/`+`/`N.`),
/// tenga o no contenido tras él (§8.4: sin contenido no es un ítem).
fn marker_like(chars: &[char], index: usize, line_end: usize) -> bool {
    let mut marker_end = index;
    match chars.get(marker_end) {
        Some('-' | '*' | '+') => marker_end += 1,
        Some('0'..='9') => {
            while matches!(chars.get(marker_end), Some('0'..='9')) {
                marker_end += 1;
            }
            match chars.get(marker_end) {
                Some('.' | ')') => marker_end += 1,
                _ => return false,
            }
        }
        _ => return false,
    }
    marker_end == line_end || chars.get(marker_end) == Some(&' ')
}

/// Índice donde empieza el contenido del ítem (tras el marcador y su
/// espacio); `None` si la línea no es un marcador de lista con contenido
/// (§8.4: marcador suelto = bloque Markdown editable).
fn list_marker(chars: &[char], index: usize, line_end: usize) -> Option<usize> {
    let mut marker_end = index;
    match chars.get(marker_end)? {
        '-' | '*' | '+' => marker_end += 1,
        '0'..='9' => {
            while matches!(chars.get(marker_end), Some('0'..='9')) {
                marker_end += 1;
            }
            match chars.get(marker_end)? {
                '.' | ')' => marker_end += 1,
                _ => return None,
            }
        }
        _ => return None,
    }
    if chars.get(marker_end) != Some(&' ') {
        return None;
    }
    marker_end += 1;
    // Marcador sin contenido no es ítem (§8.4).
    (marker_end < line_end).then_some(marker_end)
}

/// Tramos de una imagen (token = `![alt](ruta "título")` completo, §18):
/// `![`, `](`, ruta, título y `)` son sintaxis; el `alt` es un chip semántico.
/// La línea de atributos ` {…}` posterior (§7.4.3, §18.5) también se oculta;
/// sin atributos válidos se queda visible (§8.4).
fn image_spans(source: &str, span: Span, out: &mut Vec<VisualSpan>) {
    let Some(text) = slice(source, span) else { return };
    let chars: Vec<char> = text.chars().collect();
    if chars.first() != Some(&'!') || chars.get(1) != Some(&'[') {
        return;
    }
    let Some(close) = link_text_close(&chars[1..]).map(|index| index + 1) else {
        return;
    };
    let base = span.start;
    push_syntax(out, base, base + offset_of(text, 2));
    if close > 2 {
        out.push(VisualSpan {
            span: byte_span(base, text, 2, close),
            style: Some(VisualStyle::ImageAlt),
        });
    }
    push_syntax(
        out,
        base + offset_of(text, close),
        base + offset_of(text, chars.len()),
    );
    // Atributos ` {#id "caption" width=N%}` justo después del token.
    if let Some(end) = image_attrs_end(source, span.end as usize) {
        push_syntax(out, span.end, end as u32);
    }
}

/// Fin (offset del fuente, exclusivo) de los atributos ` {…}` válidos y no
/// vacíos que siguen a una imagen; `None` si no hay atributos aprovechables.
fn image_attrs_end(source: &str, from: usize) -> Option<usize> {
    let rest = source.get(from..)?;
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let line = &rest[..line_end];
    let trimmed = line.trim_start();
    let spaces = line.len() - trimmed.len();
    trimmed.strip_prefix('{')?;
    let close = trimmed.find('}')?;
    let attrs = opm_document::parse_attributes(&trimmed[1..close])?;
    if attrs.is_empty() {
        return None;
    }
    Some(from + spaces + close + 1)
}

/// Tramos de un callout (token = el bloque entero, §22): el `:::` de apertura
/// (con su espacio) y de cierre y las comillas del título son sintaxis;
/// `tipo` + título son la línea semántica con el color del tono del tipo
/// (§23; tipos desconocidos o personalizados, §22 extensible, con color
/// neutro) y el contenido va con sangrado y fondo de callout.
fn callout_spans(source: &str, span: Span, out: &mut Vec<VisualSpan>) {
    let Some(text) = slice(source, span) else { return };
    let chars: Vec<char> = text.chars().collect();
    let base = span.start;
    let mut end = chars.len();
    while end > 0 && chars[end - 1] == '\n' {
        end -= 1;
    }
    let lines = line_ranges(&chars[..end]);
    let Some(&(first_start, first_end)) = lines.first() else { return };
    let mut index = first_start;
    while index < first_end && chars[index] == ' ' {
        index += 1;
    }
    if chars[index..first_end.min(index + 3)] != [':', ':', ':'] {
        return;
    }
    let mut marker_end = index + 3;
    if chars.get(marker_end) == Some(&' ') {
        marker_end += 1;
    }
    push_syntax(
        out,
        base + offset_of(text, index),
        base + offset_of(text, marker_end),
    );
    // Tono del tipo (`::: tipo "título"`), con el nombre como primer
    // segmento de la línea semántica.
    let name_end = chars[marker_end..first_end]
        .iter()
        .position(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
        .map(|offset| marker_end + offset)
        .unwrap_or(first_end);
    let tone = {
        let name: String = chars[marker_end..name_end].iter().collect();
        callout_tone(&name)
    };
    // Segmentos entre comillas: `tipo`, `título`; las comillas son sintaxis.
    let mut segment = marker_end;
    for cursor in marker_end..=first_end {
        if cursor == first_end || chars[cursor] == '"' {
            let mut from = segment;
            let mut to = cursor;
            while from < to && chars[from] == ' ' {
                from += 1;
            }
            while to > from && chars[to - 1] == ' ' {
                to -= 1;
            }
            if from < to {
                out.push(VisualSpan {
                    span: byte_span(base, text, from, to),
                    style: Some(VisualStyle::CalloutHeader(tone)),
                });
            }
            if cursor < first_end {
                push_syntax(
                    out,
                    base + offset_of(text, cursor),
                    base + offset_of(text, cursor + 1),
                );
                segment = cursor + 1;
            }
        }
    }
    // Línea de cierre `:::` (si el callout está cerrado) y contenido.
    let closed = lines.len() > 1 && {
        let (start, stop) = lines[lines.len() - 1];
        let mut cursor = start;
        while cursor < stop && chars[cursor] == ' ' {
            cursor += 1;
        }
        stop - cursor >= 3 && chars[cursor..cursor + 3] == [':', ':', ':']
    };
    let content_end = if closed { lines.len() - 1 } else { lines.len() };
    if closed {
        let (start, stop) = lines[lines.len() - 1];
        push_syntax(
            out,
            base + offset_of(text, start),
            base + offset_of(text, stop),
        );
    }
    for &(line_start, line_end) in &lines[1..content_end] {
        if line_start == line_end {
            continue;
        }
        out.push(VisualSpan {
            span: byte_span(base, text, line_start, line_end),
            style: Some(VisualStyle::CalloutBody),
        });
    }
}

/// Tono de un tipo de callout (§22), con el mismo color que el Preview.
fn callout_tone(name: &str) -> CalloutTone {
    match name {
        "note" => CalloutTone::Note,
        "info" => CalloutTone::Info,
        "tip" => CalloutTone::Tip,
        "warning" => CalloutTone::Warning,
        "danger" => CalloutTone::Danger,
        "success" => CalloutTone::Success,
        _ => CalloutTone::Neutral,
    }
}

/// Tramos de un color inline (§7.4.4): la sintaxis `{…: ` y `}` se oculta y el
/// texto recibe los colores según las reglas §23 (texto, fondo o combinación).
fn color_spans(
    source: &str,
    span: Span,
    options: &ParseOptions,
    out: &mut Vec<VisualSpan>,
) {
    let Some(text) = slice(source, span) else { return };
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'{') || bytes.last() != Some(&b'}') {
        return;
    }
    let mut index = 1;
    let mut names: [Option<&str>; 2] = [None, None];
    if bytes.get(index) == Some(&b'+') {
        index += 1;
        names[1] = parse_color_name(bytes, &mut index, text);
    } else {
        names[0] = parse_color_name(bytes, &mut index, text);
        if bytes.get(index) == Some(&b'+') {
            index += 1;
            names[1] = parse_color_name(bytes, &mut index, text);
        }
    }
    if bytes.get(index) != Some(&b':') || names.iter().all(Option::is_none) {
        return;
    }
    index += 1;
    if bytes.get(index) == Some(&b' ') {
        index += 1;
    }
    let content_end = bytes.len() - 1;
    let base = span.start;
    push_syntax(out, base, base + index as u32);
    push_syntax(
        out,
        base + content_end as u32,
        base + bytes.len() as u32,
    );
    if content_end <= index {
        return;
    }
    let content = Span {
        start: base + index as u32,
        end: base + content_end as u32,
    };
    if let Some(slot) = names[0].and_then(|name| color_slot(name, options)) {
        out.push(VisualSpan {
            span: content,
            style: Some(VisualStyle::ColorFg(slot)),
        });
    }
    if let Some(slot) = names[1].and_then(|name| color_slot(name, options)) {
        out.push(VisualSpan {
            span: content,
            style: Some(VisualStyle::ColorBg(slot)),
        });
    }
}

/// Nombre de color `[A-Za-z][A-Za-z0-9_-]*` (§7.4.4) desde `index`.
fn parse_color_name<'a>(
    bytes: &[u8],
    index: &mut usize,
    text: &'a str,
) -> Option<&'a str> {
    let start = *index;
    if !matches!(bytes.get(*index), Some(b'a'..=b'z' | b'A'..=b'Z')) {
        return None;
    }
    while matches!(bytes.get(*index), Some(c) if c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
    {
        *index += 1;
    }
    Some(&text[start..*index])
}

/// Slot de un color: `0..=16` para la paleta §23 (física + semántica, en el
/// orden de `minmark_renderer::Palette::colors`) y `17` + índice para los
/// colores personalizados configurados (§58.2).
fn color_slot(name: &str, options: &ParseOptions) -> Option<u8> {
    if let Some(index) = crate::inlines::PHYSICAL_COLORS.iter().position(|c| *c == name) {
        return Some(index as u8);
    }
    if let Some(index) = crate::inlines::SEMANTIC_COLORS.iter().position(|c| *c == name) {
        return Some(9 + index as u8);
    }
    options
        .custom_colors
        .iter()
        .position(|c| c == name)
        .and_then(|index| u8::try_from(17 + index).ok())
}

/// Tramos de una tabla (token = la tabla entera, §35.1): las barras `|` (y el
/// escape `\|`) son sintaxis; la fila delimitadora se muestra como línea
/// separadora con estilo de regla (`:` ocultos, `-` visibles) y las celdas de
/// cabecera van en negrita. La línea de atributos propia posterior (§26.3,
/// fuera del span del bloque) también es sintaxis; sin atributos válidos se
/// queda visible (§8.4). Lo que no sea tabla GFM parseable no produce token y
/// queda como bloque Markdown editable (§8.4).
fn table_spans(source: &str, span: Span, out: &mut Vec<VisualSpan>) {
    let Some(text) = slice(source, span) else { return };
    let chars: Vec<char> = text.chars().collect();
    let base = span.start;
    let mut end = chars.len();
    while end > 0 && chars[end - 1] == '\n' {
        end -= 1;
    }
    let lines = line_ranges(&chars[..end]);
    let delimiter = (0..lines.len()).position(|index| is_delimiter_row(&chars, lines[index]));
    for (index, &line) in lines.iter().enumerate() {
        let (cells, pipes) = row_cells(&chars, line.0, line.1);
        for (start, stop) in pipes {
            push_syntax(
                out,
                base + offset_of(text, start),
                base + offset_of(text, stop),
            );
        }
        if Some(index) == delimiter {
            // Separador: `:` ocultos, `-` visibles con estilo de regla.
            for (start, stop) in cells {
                let mut cursor = start;
                while cursor < stop {
                    match chars[cursor] {
                        ':' => {
                            push_syntax(
                                out,
                                base + offset_of(text, cursor),
                                base + offset_of(text, cursor + 1),
                            );
                            cursor += 1;
                        }
                        '-' => {
                            let run = chars[cursor..stop]
                                .iter()
                                .take_while(|c| **c == '-')
                                .count();
                            out.push(VisualSpan {
                                span: byte_span(base, text, cursor, cursor + run),
                                style: Some(VisualStyle::TableRule),
                            });
                            cursor += run;
                        }
                        _ => cursor += 1,
                    }
                }
            }
        } else if Some(index + 1) == delimiter {
            // Fila de cabecera: celdas en negrita (§35.1).
            for (start, stop) in cells {
                out.push(VisualSpan {
                    span: byte_span(base, text, start, stop),
                    style: Some(VisualStyle::TableHeader),
                });
            }
        }
    }
    // Línea de atributos posterior (§26.3): `attach_attributes` la consume
    // como párrafo propio, fuera del span del bloque; solo líneas vacías
    // separan la tabla de sus atributos.
    if let Some(keepout) = attr_line_after(source, span.end as usize) {
        push_syntax(out, span.end, keepout);
    }
}

/// `true` si la línea `(inicio, fin)` es la fila delimitadora GFM: todas sus
/// celdas son solo `-` y `:`, con al menos un `-` (`---`, `:--`, `:-:`, `--:`).
fn is_delimiter_row(chars: &[char], (start, end): (usize, usize)) -> bool {
    let (cells, _) = row_cells(chars, start, end);
    !cells.is_empty()
        && cells.iter().all(|&(from, to)| {
            let mut dash = false;
            (from..to).all(|index| match chars[index] {
                '-' => {
                    dash = true;
                    true
                }
                ':' => true,
                _ => false,
            }) && dash
        })
}

/// Fin (offset del fuente, exclusivo, con su `\n`) de la línea de atributos
/// válida y no vacía `{…}` (§26.3) que sigue a un bloque, tras líneas vacías;
/// `None` si la primera línea no vacía no es de atributos.
fn attr_line_after(source: &str, from: usize) -> Option<u32> {
    let rest = source.get(from..)?;
    let mut offset = 0;
    while offset <= rest.len() {
        let line_end = rest[offset..]
            .find('\n')
            .map(|index| offset + index)
            .unwrap_or(rest.len());
        let line = rest[offset..line_end].trim();
        if !line.is_empty() {
            let inner = line.strip_prefix('{')?.strip_suffix('}')?;
            let attrs = opm_document::parse_attributes(inner)?;
            if attrs.is_empty() {
                return None;
            }
            let end = if line_end < rest.len() {
                line_end + 1
            } else {
                line_end
            };
            return Some((from + end) as u32);
        }
        if line_end >= rest.len() {
            return None;
        }
        offset = line_end + 1;
    }
    None
}

/// Celdas recortadas `(inicio, fin)` de una fila y la sintaxis de barras
/// (`|` y `\|`) que las separa.
fn row_cells(chars: &[char], start: usize, end: usize) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
    let mut cells = Vec::new();
    let mut pipes = Vec::new();
    let mut cell_start = start;
    let mut index = start;
    while index < end {
        match chars[index] {
            '\\' if chars.get(index + 1) == Some(&'|') => {
                pipes.push((index, index + 2));
                index += 2;
            }
            '|' => {
                pipes.push((index, index + 1));
                cells.push((cell_start, index));
                cell_start = index + 1;
                index += 1;
            }
            _ => index += 1,
        }
    }
    cells.push((cell_start, end));
    let trimmed = cells
        .into_iter()
        .map(|(from, to)| {
            let mut a = from;
            let mut b = to;
            while a < b && chars[a] == ' ' {
                a += 1;
            }
            while b > a && chars[b - 1] == ' ' {
                b -= 1;
            }
            (a, b)
        })
        .filter(|(from, to)| from < to)
        .collect();
    (trimmed, pipes)
}

/// Rangos `[inicio, fin)` de cada línea de `chars` (sin los `\n`).
fn line_ranges(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (index, c) in chars.iter().enumerate() {
        if *c == '\n' {
            out.push((start, index));
            start = index + 1;
        }
    }
    if start < chars.len() {
        out.push((start, chars.len()));
    }
    out
}

/// Marcador de fence de apertura de la línea `(inicio, fin)`: ` ``` ` o `~~~`
/// tras hasta 3 espacios de indentación (§7.4.1); `None` si la línea no es
/// fence.
fn fence_marker(chars: &[char], (start, end): (usize, usize)) -> Option<char> {
    let mut index = start;
    let mut indent = 0;
    while index < end && chars[index] == ' ' && indent < 3 {
        index += 1;
        indent += 1;
    }
    let marker = *chars.get(index)?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run = chars[index..end].iter().take_while(|c| **c == marker).count();
    (run >= 3).then_some(marker)
}

/// Índice (en chars) donde empiezan los atributos `{…}` finales del heading
/// (` {#id …}`, §7.4.3), incluido el espacio que los precede; `None` si no
/// hay una línea de atributos válida al final.
fn trailing_attrs_start(content: &[char]) -> Option<usize> {
    let end = content.len();
    if end < 3 || content[end - 1] != '}' {
        return None;
    }
    // Busca la `{` de apertura más a la derecha que cierre justo en el `}`
    // final y cuyo interior sea una línea de atributos válida (§7.4.3).
    for (index, c) in content.iter().enumerate().rev() {
        if *c != '{' {
            continue;
        }
        let inner: String = content[index + 1..end - 1].iter().collect();
        if opm_document::parse_attributes(&inner).is_some() {
            let mut start = index;
            while start > 0 && content[start - 1] == ' ' {
                start -= 1;
            }
            return Some(start);
        }
    }
    None
}

fn push_syntax(out: &mut Vec<VisualSpan>, start: u32, end: u32) {
    if start < end {
        out.push(VisualSpan {
            span: Span { start, end },
            style: None,
        });
    }
}

fn slice(source: &str, span: Span) -> Option<&str> {
    source.get(span.start as usize..span.end as usize)
}

/// Offset en bytes dentro de `text` del índice de chars dado (relativo al
/// inicio de `text`; `chars.len()` da el final).
fn offset_of(text: &str, char_index: usize) -> u32 {
    text.char_indices()
        .nth(char_index)
        .map(|(i, _)| i)
        .unwrap_or(text.len()) as u32
}

/// Span en bytes absoluto de `[start_char, end_char)` dentro de `text`, que
/// empieza en `base` del fuente.
fn byte_span(base: u32, text: &str, start_char: usize, end_char: usize) -> Span {
    Span {
        start: base + offset_of(text, start_char),
        end: base + offset_of(text, end_char),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(source: &str) -> VisualPlan {
        plan_with(source, &ParseOptions::default())
    }

    fn plan_with(source: &str, options: &ParseOptions) -> VisualPlan {
        let highlights = crate::highlight_with(source, options);
        let ast = crate::parse(source);
        from_parts(source, &ast, &highlights, options)
    }

    /// Texto de un tramo, sin el `\n` final (la inclusión del salto en el
    /// span del token según el parser no debe cambiar el plan).
    fn text_of<'a>(source: &'a str, span: &Span) -> &'a str {
        source[span.start as usize..span.end as usize]
            .trim_end_matches('\n')
    }

    fn syntax_texts<'a>(source: &'a str, plan: &VisualPlan) -> Vec<&'a str> {
        plan.spans
            .iter()
            .filter(|s| s.is_syntax())
            .map(|s| text_of(source, &s.span))
            .collect()
    }

    fn content_texts<'a>(source: &'a str, plan: &VisualPlan) -> Vec<(&'a str, VisualStyle)> {
        plan.spans
            .iter()
            .filter_map(|s| s.style.map(|style| (text_of(source, &s.span), style)))
            .collect()
    }

    #[test]
    fn heading_atx_oculta_marcadores_y_estila_contenido() {
        let source = "## Instalación\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["## "]);
        assert_eq!(
            content_texts(source, &plan),
            vec![("Instalación", VisualStyle::Heading(2))]
        );
    }

    #[test]
    fn heading_niveles_1_a_6() {
        for (fuente, nivel) in [("# T\n", 1u8), ("### T\n", 3), ("###### T\n", 6)] {
            let plan = plan(fuente);
            assert_eq!(
                plan.spans.iter().find_map(|s| s.style),
                Some(VisualStyle::Heading(nivel)),
                "{fuente}"
            );
        }
    }

    #[test]
    fn heading_atx_con_atributos_7_4_3() {
        let source = "## Título {#instalacion}\n";
        let plan = plan(source);
        assert_eq!(
            syntax_texts(source, &plan),
            vec!["## ", " {#instalacion}"]
        );
        assert_eq!(
            content_texts(source, &plan),
            vec![("Título", VisualStyle::Heading(2))]
        );
    }

    #[test]
    fn heading_llaves_no_atributo_no_se_ocultan() {
        // §7.4.3: `{…}` solo es sintaxis en posición de atributo y con forma
        // válida; `con {llaves}` es texto literal.
        let source = "## Con {llaves}\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["## "]);
        assert_eq!(
            content_texts(source, &plan),
            vec![("Con {llaves}", VisualStyle::Heading(2))]
        );
    }

    #[test]
    fn heading_setext_subrayado_es_sintaxis() {
        let source = "Título\n======\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["======"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![("Título", VisualStyle::Heading(1))]
        );
    }

    #[test]
    fn negrita_y_cursiva_ocultan_solo_marcadores() {
        let source = "Texto en **negrita** y *cursiva* y __otra__.\n";
        let plan = plan(source);
        assert_eq!(
            syntax_texts(source, &plan),
            vec!["**", "**", "*", "*", "__", "__"]
        );
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("negrita", VisualStyle::Strong),
                ("cursiva", VisualStyle::Emphasis),
                ("otra", VisualStyle::Strong),
            ]
        );
    }

    #[test]
    fn enfasis_anidado_no_solapa_marcadores() {
        let source = "**a *b* c**\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["**", "*", "*", "**"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("a *b* c", VisualStyle::Strong),
                ("b", VisualStyle::Emphasis),
            ]
        );
    }

    #[test]
    fn marcadores_sin_parsear_no_producen_tramos() {
        // §8.4/§7.4.6: énfasis sin cerrar o escapado es texto literal.
        for source in ["a **b\n", "a \\*b\\* c\n", "snake_case_words\n"] {
            let plan = plan(source);
            assert!(plan.spans.is_empty(), "{source}: {:?}", plan.spans);
        }
    }

    #[test]
    fn elementos_cubren_bloques_anidados() {
        let source = "# T\n\n> cita\n>\n> - item\n";
        let plan = plan(source);
        assert!(plan.elements.len() >= 4, "{:?}", plan.elements);
        // El bloque externo de la cita contiene a los internos.
        let outer = plan.elements[1];
        let inner = plan.elements[2];
        assert!(inner.start >= outer.start && inner.end <= outer.end);
    }

    #[test]
    fn multi_byte_no_rompe_offsets() {
        let source = "## ñáé **íóú**\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["## ", "**", "**"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("ñáé **íóú**", VisualStyle::Heading(2)),
                ("íóú", VisualStyle::Strong),
            ]
        );
    }

    #[test]
    fn link_oculta_sintaxis_y_estila_texto() {
        let source = "[Arquitectura](architecture.md \"t\")\n";
        let plan = plan(source);
        assert_eq!(
            syntax_texts(source, &plan),
            vec!["[", "](architecture.md \"t\")"]
        );
        assert_eq!(
            content_texts(source, &plan),
            vec![("Arquitectura", VisualStyle::Link)]
        );
    }

    #[test]
    fn link_con_texto_anidado() {
        // `**` dentro del enlace: el texto entero es Link y el énfasis se
        // estiliza por encima (los planes se solapan).
        let source = "[**negrita**](x)\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["[", "**", "**", "](x)"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("**negrita**", VisualStyle::Link),
                ("negrita", VisualStyle::Strong),
            ]
        );
    }

    #[test]
    fn link_con_corchetes_y_code_span_en_el_texto() {
        // El primer `]` balanceado cierra el texto; `](` dentro de un code
        // span (§7.4.1) no cuenta.
        let source = "[a[b] `](` c](d)\n";
        let plan = plan(source);
        assert_eq!(
            syntax_texts(source, &plan),
            vec!["[", "`", "`", "](d)"]
        );
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("a[b] `](` c", VisualStyle::Link),
                ("](", VisualStyle::Code),
            ]
        );
    }

    #[test]
    fn code_span_oculta_backticks() {
        let source = "a `b` c\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["`", "`"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![("b", VisualStyle::Code)]
        );
    }

    #[test]
    fn code_span_no_interpreta_extensiones() {
        // §7.4.1: dentro de code spans no se interpreta ninguna extensión.
        let source = "`{danger: x} {{toc}} [@a]`\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["`", "`"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![("{danger: x} {{toc}} [@a]", VisualStyle::Code)]
        );
    }

    #[test]
    fn code_block_oculta_fences_e_info_string() {
        let source = "```rs\nx\n```\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["```rs", "```"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![("x", VisualStyle::CodeBlock)]
        );
    }

    #[test]
    fn code_block_indentado_solo_contenido() {
        // Sin fences que ocultar: el bloque entero es contenido (el parser
        // excluye la indentación del span del bloque).
        let source = "    x = 1\n\n";
        let plan = plan(source);
        assert!(plan.spans.iter().all(|s| !s.is_syntax()));
        assert_eq!(
            content_texts(source, &plan),
            vec![("x = 1", VisualStyle::CodeBlock)]
        );
    }

    #[test]
    fn code_block_no_interpreta_contenido() {
        let source = "```\n# no es heading **ni nada**\n```\n";
        let plan = plan(source);
        assert_eq!(
            content_texts(source, &plan),
            vec![("# no es heading **ni nada**", VisualStyle::CodeBlock)]
        );
    }

    #[test]
    fn quote_oculta_marcadores() {
        let source = "> hola\n> mundo\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["> ", "> "]);
        assert_eq!(
            content_texts(source, &plan),
            vec![("hola", VisualStyle::Quote), ("mundo", VisualStyle::Quote)]
        );
    }

    #[test]
    fn quote_con_linea_de_continuacion_y_contenido_inline() {
        let source = "> **fuerte**\ncontinuación\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["> ", "**", "**"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("**fuerte**", VisualStyle::Quote),
                ("fuerte", VisualStyle::Strong),
                ("continuación", VisualStyle::Quote),
            ]
        );
    }

    #[test]
    fn quote_anidada_oculta_ambos_marcadores() {
        let source = "> a\n> > b\n";
        let plan = plan(source);
        // Todo `>` del fuente queda cubierto por sintaxis ocultable.
        for (index, c) in source.char_indices() {
            if c != '>' {
                continue;
            }
            let index = index as u32;
            assert!(
                plan.spans.iter().any(|s| {
                    s.is_syntax() && s.span.start <= index && index < s.span.end
                }),
                "`>` sin ocultar en {index}"
            );
        }
        assert!(content_texts(source, &plan).contains(&("b", VisualStyle::Quote)));
    }

    #[test]
    fn sintaxis_rota_es_bloque_markdown_editable() {
        // §8.4: constructos sin cerrar no se representan; su sintaxis
        // permanece visible y el bloque sigue siendo editable.
        for source in [
            "[Arquitectura](architecture.md\n",
            "[texto] suelto\n",
            "cita sin marcar\n",
        ] {
            let plan = plan(source);
            assert!(plan.spans.is_empty(), "{source}: {:?}", plan.spans);
        }
    }

    #[test]
    fn lista_simple_oculta_marcadores() {
        let source = "- uno\n- dos\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["- ", "- "]);
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("uno", VisualStyle::ListItem(1)),
                ("dos", VisualStyle::ListItem(1)),
            ]
        );
    }

    #[test]
    fn lista_numerada_desde_n() {
        let source = "3. uno\n4. dos\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["3. ", "4. "]);
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("uno", VisualStyle::ListItem(1)),
                ("dos", VisualStyle::ListItem(1)),
            ]
        );
    }

    #[test]
    fn lista_anidada_conserva_sangria_y_nivel() {
        // La sangría "  " no es sintaxis (define la estructura, §6.2); el
        // nivel del contenido refleja la anidación.
        let source = "- a\n  - b\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["- ", "- "]);
        let content = content_texts(source, &plan);
        assert!(content.contains(&("a", VisualStyle::ListItem(1))), "{content:?}");
        assert!(content.contains(&("b", VisualStyle::ListItem(2))), "{content:?}");
    }

    #[test]
    fn tareas_visibles_con_estilo_propio() {
        // Decisión ISSUE-032: `[ ]`/`[x]` no se ocultan (perderían el estado).
        let source = "- [x] hecho\n- [ ] pendiente\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["- ", "- "]);
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("[x]", VisualStyle::Task),
                ("hecho", VisualStyle::ListItem(1)),
                ("[ ]", VisualStyle::Task),
                ("pendiente", VisualStyle::ListItem(1)),
            ]
        );
    }

    #[test]
    fn lista_con_codigo_interno_no_toca_el_codigo() {
        // §7.4.1: dentro del código nada se interpreta, ni marcadores de
        // lista falsos.
        let source = "- a\n\n  ```\n  - trampa\n  ```\n";
        let plan = plan(source);
        let markers: Vec<_> = syntax_texts(source, &plan)
            .into_iter()
            .filter(|t| *t == "- ")
            .collect();
        assert_eq!(markers, vec!["- "], "{:?}", plan.spans);
        assert!(
            content_texts(source, &plan)
                .iter()
                .any(|(t, s)| t.contains("trampa") && *s == VisualStyle::CodeBlock),
            "{:?}",
            plan.spans
        );
    }

    #[test]
    fn marcador_suelto_es_bloque_markdown_editable() {
        // §8.4: marcador sin contenido no se representa; su sintaxis
        // permanece visible.
        for source in ["texto\n\n-\n", "- \n", "suelto\n1.\n"] {
            let plan = plan(source);
            assert!(plan.spans.is_empty(), "{source}: {:?}", plan.spans);
        }
    }

    #[test]
    fn tabla_oculta_barras_y_estila_cabecera() {
        let source = "| A | B |\n|---|---|\n| 1 | 2 |\n";
        let plan = plan(source);
        let syntax = syntax_texts(source, &plan);
        assert_eq!(syntax.iter().filter(|t| **t == "|").count(), 9, "{syntax:?}");
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("A", VisualStyle::TableHeader),
                ("B", VisualStyle::TableHeader),
                ("---", VisualStyle::TableRule),
                ("---", VisualStyle::TableRule),
            ]
        );
    }

    #[test]
    fn tabla_desalineada_oculta_igual_las_barras() {
        // Fuente desalineado (columnas de anchos distintos): el plan oculta
        // igual las barras y estila la cabecera; la alineación visual la da
        // el buffer (§6.2).
        let source = "| A | B |\n| --- | --- |\n| contenido largo | 2 |\n";
        let plan = plan(source);
        let syntax = syntax_texts(source, &plan);
        assert_eq!(syntax.iter().filter(|t| **t == "|").count(), 9, "{syntax:?}");
        let header: Vec<_> = content_texts(source, &plan)
            .into_iter()
            .filter(|(_, s)| *s == VisualStyle::TableHeader)
            .collect();
        assert_eq!(
            header,
            vec![("A", VisualStyle::TableHeader), ("B", VisualStyle::TableHeader)]
        );
    }

    #[test]
    fn tabla_con_alineaciones_gfm() {
        // §35.1: `:--`, `--:` preservados en el buffer; los `:` son sintaxis
        // y los `-` quedan como separador con estilo de regla.
        let source = "| A | B |\n|:--|--:|\n| 1 | 2 |\n";
        let plan = plan(source);
        let syntax = syntax_texts(source, &plan);
        assert_eq!(syntax.iter().filter(|t| **t == "|").count(), 9, "{syntax:?}");
        assert_eq!(syntax.iter().filter(|t| **t == ":").count(), 2, "{syntax:?}");
        let rules: Vec<_> = content_texts(source, &plan)
            .into_iter()
            .filter(|(_, s)| *s == VisualStyle::TableRule)
            .collect();
        assert_eq!(rules, vec![("--", VisualStyle::TableRule), ("--", VisualStyle::TableRule)]);
    }

    #[test]
    fn atributos_de_tabla_ocultos() {
        // §26.3: línea de atributos propia posterior, sintaxis ocultable.
        let source = "| A |\n|---|\n| 1 |\n\n{#tbl:r \"Res\"}\n";
        let plan = plan(source);
        assert!(
            syntax_texts(source, &plan)
                .iter()
                .any(|t| t.contains("{#tbl:r")),
            "{:?}",
            plan.spans
        );
    }

    #[test]
    fn imagen_oculta_sintaxis_y_estila_alt() {
        let source = "![alt](img.png)\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["![", "](img.png)"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![("alt", VisualStyle::ImageAlt)]
        );
    }

    #[test]
    fn imagen_con_atributos_ocultos() {
        // §18.5/§7.4.3: atributos ` {…}` ocultos; sin atributos válidos no
        // se oculta nada.
        let source = "![alt](img.png) {#i \"c\" width=50%}\n";
        let con_attrs = plan(source);
        assert_eq!(
            syntax_texts(source, &con_attrs),
            vec!["![", "](img.png)", " {#i \"c\" width=50%}"]
        );
        let source = "![alt](img.png) {no atributos}\n";
        let sin_attrs = plan(source);
        assert_eq!(syntax_texts(source, &sin_attrs), vec!["![", "](img.png)"]);
    }

    #[test]
    fn imagenes_dentro_de_links() {
        let source = "[![alt](img.png)](p.md)\n";
        let plan = plan(source);
        assert_eq!(
            syntax_texts(source, &plan),
            vec!["[", "![", "](img.png)", "](p.md)"]
        );
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("![alt](img.png)", VisualStyle::Link),
                ("alt", VisualStyle::ImageAlt),
            ]
        );
    }

    #[test]
    fn tabla_malformada_es_bloque_markdown_editable() {
        // §8.4: sin fila delimitadora GFM no hay tabla.
        for source in ["a | b\n1 | 2\n", "solo texto\n"] {
            let plan = plan(source);
            assert!(plan.spans.is_empty(), "{source}: {:?}", plan.spans);
        }
    }

    #[test]
    fn callout_con_titulo_oculta_marcadores_y_estila_linea() {
        let source = "::: warning \"Atención\"\ncontenido\n:::\n";
        let plan = plan(source);
        assert_eq!(
            syntax_texts(source, &plan),
            vec!["::: ", "\"", "\"", ":::"]
        );
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("warning", VisualStyle::CalloutHeader(CalloutTone::Warning)),
                ("Atención", VisualStyle::CalloutHeader(CalloutTone::Warning)),
                ("contenido", VisualStyle::CalloutBody),
            ]
        );
    }

    #[test]
    fn callout_sin_titulo() {
        let source = "::: note\nx\n:::\n";
        let plan = plan(source);
        assert_eq!(syntax_texts(source, &plan), vec!["::: ", ":::"]);
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("note", VisualStyle::CalloutHeader(CalloutTone::Note)),
                ("x", VisualStyle::CalloutBody),
            ]
        );
    }

    #[test]
    fn callout_tipos_integrados_y_desconocidos() {
        // §22: el color sale del tipo; los tipos desconocidos o
        // personalizados (extensibles) van con color neutro.
        for (name, tone) in [
            ("note", CalloutTone::Note),
            ("info", CalloutTone::Info),
            ("tip", CalloutTone::Tip),
            ("warning", CalloutTone::Warning),
            ("danger", CalloutTone::Danger),
            ("success", CalloutTone::Success),
        ] {
            let source = format!("::: {name}\nx\n:::\n");
            let plan = plan(&source);
            assert!(
                content_texts(&source, &plan).contains(&(name, VisualStyle::CalloutHeader(tone))),
                "{name}: {:?}",
                plan.spans
            );
        }
        let tipos = vec!["custom-x".to_string()];
        let options = ParseOptions {
            custom_callout_types: &tipos,
            ..Default::default()
        };
        let source = "::: custom-x\nx\n:::\n";
        let plan = plan_with(source, &options);
        assert!(content_texts(source, &plan)
            .contains(&("custom-x", VisualStyle::CalloutHeader(CalloutTone::Neutral))));
    }

    #[test]
    fn callout_no_cerrado_es_bloque_markdown_editable() {
        // §8.4: sin `:::` de cierre el parser no produce callout; la
        // sintaxis queda visible y el bloque es editable.
        let source = "::: info\nx\n";
        let plan = plan(source);
        assert!(plan.spans.is_empty(), "{:?}", plan.spans);
    }

    #[test]
    fn colores_tres_formas_7_4_4() {
        // `{nombre:}` texto, `{+nombre:}` fondo, `{a+b:}` combinación.
        let source = "{danger: x} {+highlight: y} {accent+highlight: z}\n";
        let plan = plan(source);
        assert_eq!(
            syntax_texts(source, &plan),
            vec![
                "{danger: ",
                "}",
                "{+highlight: ",
                "}",
                "{accent+highlight: ",
                "}",
            ]
        );
        assert_eq!(
            content_texts(source, &plan),
            vec![
                ("x", VisualStyle::ColorFg(9)),  // danger (§23 semántico 0)
                ("y", VisualStyle::ColorBg(15)), // highlight (§23 semántico 6)
                ("z", VisualStyle::ColorFg(16)), // accent (§23 semántico 7)
                ("z", VisualStyle::ColorBg(15)),
            ]
        );
    }

    #[test]
    fn color_nombre_desconocido_es_texto_literal() {
        // §7.4.4: nombre inexistente en paletas y config → texto literal.
        for source in ["{noexiste: x}\n", "{+tampoco: y}\n"] {
            let plan = plan(source);
            assert!(plan.spans.is_empty(), "{source}: {:?}", plan.spans);
        }
    }

    #[test]
    fn color_personalizado_usa_slot_propio() {
        let names = vec!["brand".to_string()];
        let options = ParseOptions {
            custom_colors: &names,
            ..Default::default()
        };
        let source = "{brand: x}\n";
        let plan = plan_with(source, &options);
        assert_eq!(
            content_texts(source, &plan),
            vec![("x", VisualStyle::ColorFg(17))]
        );
    }

    #[test]
    fn bloques_sin_representacion_quedan_como_markdown() {
        // §8.4 completo: math §24, citas §25, sustituciones §7.4.2 y front
        // matter §21 sin ocultar; wiki links §19.3 igual aunque estén
        // activados.
        for source in [
            "$E = mc^2$\n",
            "$$\nE = mc^2\n$$\n",
            "[@s, p. 1]\n",
            "{{ref:fig:a}}\n",
            "---\ntitle: x\n---\n",
        ] {
            let plan = plan(source);
            assert!(plan.spans.is_empty(), "{source}: {:?}", plan.spans);
        }
        let options = ParseOptions {
            wiki_links: true,
            ..Default::default()
        };
        let plan = plan_with("[[architecture#database]]\n", &options);
        assert!(plan.spans.is_empty());
    }
}
