//! What an index build skips, beyond its own `.ide-index` directory
//! (ADR-0064: `project_model::ProjectScope`, not `.gitignore`, decides).
//!
//! Separate from `lib.rs` because that file is at its size baseline and may
//! only shrink (`scripts/check-file-size.sh`).

/// What an index build is allowed to skip, on top of its own storage
/// directory.
///
/// A struct rather than positional parameters because the next thing to
/// configure — a size ceiling, a symlink policy — would otherwise be a third
/// bare argument, and a call site reading `(root, list1, list2, cb)` says
/// nothing about which list is which.
///
/// Both lists are resolved by `settings_model::scope` from the global and
/// project settings layers before they reach here (ADR-0022); this crate
/// only folds them into a [`project_model::ProjectScope`] and walks. Which
/// layer a pattern came from is not this crate's question.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexOptions {
    /// Per-project *Excluded* folders, gitignore syntax, root-anchored.
    pub excluded: Vec<String>,
    /// Global *Ignored names*, gitignore syntax, matched at any depth.
    pub ignored_names: Vec<String>,
}
