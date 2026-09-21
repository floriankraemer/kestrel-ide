//! `db-sql`: the Qt-free, tokio-free SQL assistance engine (database-tools
//! plan F3.2) — statement splitting, classification, parsing, formatting,
//! schema-aware completion, lint-style inspections, and "identifier under
//! the caret" navigation, all keyed off `db_core::dialect::Dialect` and
//! `db_core::schema::SchemaSnapshot` rather than re-deriving either.
//!
//! `crate::classify::SqlClassifier` is the real
//! `db_core::readonly::Classifier` this crate contributes, replacing
//! `db_core::readonly::NaiveClassifier` at the point a console wires a
//! source's read-only guard (see `db_core::readonly`'s doc comment).

pub mod classify;
pub mod clauses;
pub mod completion;
pub mod dialects;
pub mod format;
pub mod inspections;
pub mod mongo;
pub mod navigation;
pub mod parse;
mod refs;
pub mod resp;
mod scan;
pub mod split;

pub use classify::{classify, Classification, SqlClassifier};
pub use clauses::apply as apply_clauses;
pub use completion::{completion, CompletionItem, CompletionKind};
pub use format::format;
pub use inspections::inspections;
pub use navigation::navigate;
pub use parse::parse;
pub use split::{split, Statement};
