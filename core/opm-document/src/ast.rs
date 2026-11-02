// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! AST compartida del dialecto Markdown de minmark (§7).
//!
//! La AST es la interfaz entre `opm-parser` (produce), `minmark-renderer`
//! y `minmark-pdf` (consumen). Los nodos cubren la base CommonMark + GFM (§7.1)
//! y las extensiones propias §7.3/§7.4 (ISSUE-014): sustituciones `{{...}}`,
//! atributos `{...}`, colores inline, math LaTeX, citas Pandoc, callouts `:::`
//! y wiki links.
//!
//! La AST es **derivada** del buffer (§6.1): parsear nunca modifica el fuente
//! y el Markdown original se preserva tal cual (§6.2).

/// Rango de bytes en el fuente Markdown original.
///
/// Base para la fidelidad del fuente (§6.2) y la sincronización de scroll por
/// posición semántica (§9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

/// Documento completo parseado.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ast {
    /// Bloques de nivel superior, en orden de aparición.
    pub blocks: Vec<Block>,
    /// YAML front matter sin interpretar (§21, §58.1 nivel documento).
    pub front_matter: Option<String>,
}

/// Atributos `{#id "caption" clave=valor ...}` (§7.4.3).
///
/// Solo son sintaxis en posición de atributo (§7.4.3): tras una imagen inline,
/// en línea propia tras tabla o ecuación de bloque, y al final de un heading.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attributes {
    /// `#id` (§26): `[A-Za-z0-9:._-]+` tras `#`.
    pub id: Option<String>,
    /// Caption entrecomillado (§7.4.3, §28).
    pub caption: Option<String>,
    /// `width=N%`, 1–100 (§18.5).
    pub width_percent: Option<u8>,
    /// Pares `clave=valor` adicionales; en v1 solo `width` tiene significado.
    pub extra: Vec<(String, String)>,
}

impl Attributes {
    pub fn is_empty(&self) -> bool {
        self.id.is_none()
            && self.caption.is_none()
            && self.width_percent.is_none()
            && self.extra.is_empty()
    }
}

/// Bloque con su posición en el fuente.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub span: Span,
    pub kind: BlockKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    /// Bibliografía derivada por CSL, con sus reglas de layout.
    Bibliography {
        entries: Vec<(Vec<Inline>, Vec<Inline>)>,
        hanging_indent: bool,
        second_field_align: bool,
        line_spacing: f32,
        entry_spacing: f32,
    },
    /// Encabezado H1–H6; `level` está en 1..=6 (§27.1). `attrs` cubre
    /// `## Título {#id}` (§26.1).
    Heading {
        level: u8,
        content: Vec<Inline>,
        attrs: Option<Attributes>,
    },
    Paragraph {
        content: Vec<Inline>,
    },
    Quote {
        blocks: Vec<Block>,
    },
    /// `ordered = false` → lista con guiones; `ordered = true` → numerada
    /// desde `start`.
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    /// Tabla GFM (§35): `aligns` por columna, `header` celdas, `rows` filas
    /// de celdas. `attrs` cubre la línea de atributos propia posterior (§26.3).
    Table {
        aligns: Vec<Align>,
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        attrs: Option<Attributes>,
    },
    /// Ecuación de bloque LaTeX (§24): `$$ ... $$`. `attrs` cubre la línea de
    /// atributos propia posterior (§26.4).
    MathBlock {
        latex: String,
        attrs: Option<Attributes>,
    },
    /// Definición de nota estándar (§30), conservada como nodo derivado.
    FootnoteDefinition {
        label: String,
        blocks: Vec<Block>,
    },
    /// Bloque destacado `::: tipo "título"` (§22).
    Callout {
        callout_type: String,
        title: Option<String>,
        blocks: Vec<Block>,
    },
    /// Separador temático (`---`, `***`, `___`).
    Rule,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub span: Span,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    /// Formato derivado del motor CSL; nunca sintaxis ni HTML del usuario.
    Styled { content: Vec<Inline>, style: InlineStyle },
    /// Referencia a una nota estándar (§30).
    FootnoteReference(String),
    Text(String),
    /// Code span (§7.4.1: dentro de código no se interpreta ninguna extensión).
    Code(String),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Link {
        destination: String,
        title: String,
        content: Vec<Inline>,
    },
    /// Imagen (§18). `attrs` cubre `![alt](ruta) {width=50%}` (§18.5, §26.2).
    Image {
        destination: String,
        title: String,
        alt: Vec<Inline>,
        attrs: Option<Attributes>,
    },
    /// Math inline LaTeX (§24): `$E = mc^2$`.
    Math(String),
    /// Color inline `{fg: t}`, `{+bg: t}`, `{fg+bg: t}` (§23, §7.4.4).
    /// El nombre debe existir en la paleta física, semántica o personalizada;
    /// si no, la construcción es texto literal.
    Color {
        foreground: Option<String>,
        background: Option<String>,
        text: String,
    },
    /// Cita estilo Pandoc (§25.2): `[@smith2025]`, `[@a; @b]`, `[@a, p. 42]`.
    Citation {
        items: Vec<CitationItem>,
    },
    /// Wiki link (§19.3): `[[architecture]]`, `[[architecture#database]]`.
    /// Solo sintaxis cuando `wiki_links` está activado.
    WikiLink {
        target: String,
    },
    /// Sustitución de vocabulario cerrado (§7.4.2).
    Substitution(Substitution),
    /// Marca de tarea GFM (`- [x]`), `bool` = completada.
    TaskListMarker(bool),
    SoftBreak,
    HardBreak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InlineStyle {
    pub italic: bool,
    pub bold: bool,
    pub light: bool,
    pub small_caps: bool,
    pub underline: bool,
    /// -1 = subíndice, 0 = baseline, 1 = superíndice.
    pub vertical: i8,
}

/// Item de cita (§25.2): clave `@key` con localizador opcional (`, p. 42`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationItem {
    pub key: String,
    pub locator: Option<String>,
}

/// Sustituciones `{{...}}` (§7.4.2): vocabulario cerrado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Substitution {
    /// `{{toc}}` (§29).
    Toc,
    /// `{{pagebreak}}` (§31).
    PageBreak,
    /// `{{ref:ID}}` (§26.5), `ID` = `[A-Za-z0-9:._-]+`.
    CrossRef { id: String },
}

/// Alineación de columna de tabla GFM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}
