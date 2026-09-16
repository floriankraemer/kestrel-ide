//! Build-file editing assistance (jvm-build-tools plan, phase D):
//! caret-context classification ([`context`]), the local repository index
//! (D3), a Maven Central client (D4), completion (D5) and "newer version"
//! hints (D6) all live under this module.
//!
//! Deliberately Qt-free and `lsp-core`-free (per the plan's core
//! constraint): every type here is this crate's own — `ui-shell`'s bridge
//! is what maps a [`context::EditContext`]'s eventual completion items
//! into `lsp_core::CompletionItem`, not this crate.

pub mod central;
pub mod completion;
pub mod context;
pub mod repo_index;
pub mod version_order;
pub mod versions;
