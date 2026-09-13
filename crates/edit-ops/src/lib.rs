//! Editing operations that need the text *and* the grammar.
//!
//! Commenting, expanding a selection, indenting a new line, closing a
//! bracket and finding a bracket's partner are all answers to "what does
//! this language look like here?", and answering needs two crates at once:
//! [`editor_core`] for carets, offsets and transactions, and
//! [`syntax_core`] for the language registry and the parse tree.
//! `editor-core` may not depend on `syntax-core` and must not start, and
//! joining them in `bridge.rs` is what the layering rules exist to forbid,
//! so the join lives here — the same situation `settings-model` is in, and
//! the same answer.
//!
//! # Every entry point takes `text: &str`
//!
//! Never a [`editor_core::Document`]: its rope is only refreshed on save,
//! so it is one save behind the live Qt buffer at all times. This is the
//! stateless shape [`editor_core::find_matches`] already has, for exactly
//! that reason.
//!
//! # Offsets are bytes
//!
//! tree-sitter is byte-addressed and so are carets; the FFI seam speaks
//! UTF-16 and converts in one place, [`editor_core::offsets`].
//!
//! # What each module owns
//!
//! - [`comment`] — toggling line and block comments.
//! - [`selection_expand`] — expand/shrink over the node tree, with the
//!   stack that makes shrink retrace exactly.
//! - [`indent`] — the indent of a new line, and indent/unindent of a
//!   selection.
//! - [`pairs`] — auto-close, type-over, smart backspace, surround.
//! - [`brackets`] — a bracket's partner, and where the caret jumps to.
//!
//! Qt-free, like every crate below the adapter.

pub mod brackets;
pub mod comment;
pub mod indent;
pub mod pairs;
pub mod selection_expand;
/// R2: the LSP snippet grammar (`${1:name}`, `$0`, choices, nesting,
/// escapes) parsed into inserted text plus tab-stop ranges.
pub mod snippet;
mod syntax;

pub use syntax::{Syntax, Tokens};
