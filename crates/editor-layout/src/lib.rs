//! Original UI-independent source geometry over low-level shaping primitives.
//!
//! COSMIC Text supplies font discovery, shaping, line breaking, and rasterization.
//! This crate supplies source positions, legal carets, hit testing, visual movement,
//! and selection geometry. No COSMIC editor/controller/selection implementation is
//! used. Block flow uses stable identities, indexed estimated/measured heights,
//! bounded viewport work, and explicit long-block policies. Legacy full-text
//! shaping remains available as a measured functional fallback.

mod table;
pub use table::*;
mod source;
mod virtual_document;

pub use source::*;
pub use virtual_document::*;
