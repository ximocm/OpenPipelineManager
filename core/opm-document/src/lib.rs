// Cloned and adapted from minmark (https://github.com/ximocm/minmark), Apache-2.0. Modifications © Open Pipeline Manager contributors.
//! Modelo documental de minmark (§5.1, §6).
//!
//! - [`Document`]: buffer Markdown único, fuente de verdad (§6.1).
//! - [`Ast`]: modelo sintáctico compartido entre parser, renderer y PDF.
//! - [`Editor`]: ediciones con undo/redo único (§8.5) y posiciones
//!   semánticas (§8.6); [`Matcher`]: búsqueda/reemplazo del archivo actual
//!   (§40.1, ISSUE-027) con motor regex propio std-only.
//! - [`LineEnding`]: preservación LF/CRLF (§12.6).
//! - [`compose`]: composiciones multiarchivo (§20): documento lógico virtual
//!   con destinos reescritos como enlaces internos (§20.4).
//!
//! Crate sin dependencias de compilación: solo `std` (§4.1); el parser, el
//! renderer y la configuración se usan solo como dev-dependencies de tests.

mod anchors;
mod ast;
mod attrs;
mod compose;
mod document;
mod editor;
mod position;
mod references;
mod regex;
mod search;

pub use anchors::{
    document_anchors, find_anchor, find_heading_nodes, find_headings, Anchor, Heading, HeadingNode,
};
pub use ast::{
    Align, Ast, Attributes, Block, BlockKind, CitationItem, Inline, InlineStyle, ListItem, Span,
    Substitution,
};
pub use attrs::{parse_attributes, strip_trailing_attrs};
pub use compose::{compose, ComposePart, Composed};
pub use document::{Document, LineEnding};
pub use editor::{EditError, Editor};
pub use position::{Position, Range, Selection};
pub use references::{
    apply_replacements, find_link_definitions, find_markdown_references, find_reference_usages,
    find_toml_references, normalize_label, scan_markdown_links, DestForm, LinkDefinition, LinkScan,
    RefKind, Reference, ReferenceUsage,
};
pub use search::{Matcher, SearchError, SearchOptions, TextMatch};
