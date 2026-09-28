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

/// `lib.rs`'s `MAX_INDEXED_BYTES` and `BINARY_SNIFF_BYTES` — the content
/// rules the Project Scope settings page (ADR-0064) states in its note —
/// exposed as values only, so `ui-shell` never duplicates the constants as
/// separate literals. Placed here rather than in `lib.rs` for the same
/// size-baseline reason as [`IndexOptions`] above.
pub fn content_rule_limits() -> (u64, usize) {
    (crate::MAX_INDEXED_BYTES, crate::BINARY_SNIFF_BYTES)
}

/// Name of the file [`ensure_index_gitignore`] writes.
pub(crate) const GITIGNORE_FILE: &str = ".gitignore";

/// Self-ignoring, the same trick `cargo` plays for `target/`: a bare `*`
/// makes every file the index ever writes invisible to `git status`
/// without touching a single line of the user's own `.gitignore`. Without
/// it the index lived inside a tracked project as ordinary untracked
/// content — `git add -A`/"Stage all" would stage its (binary) segment
/// files, and the Changes dock would then try to diff them (#345).
const GITIGNORE_CONTENTS: &str = "*\n";

/// Write (or rewrite) [`GITIGNORE_FILE`] under `index_dir`. Cheap enough
/// (one small write) not to bother checking whether it is already there and
/// unchanged first; `TextIndex::is_index_internal` already keeps this path
/// itself out of the index, the walk and every mutating entry point, the
/// same as every other file the index writes under `index_dir`.
fn ensure_index_gitignore(index_dir: &std::path::Path) {
    let _ = std::fs::write(index_dir.join(GITIGNORE_FILE), GITIGNORE_CONTENTS);
}

/// [`std::fs::create_dir_all`] plus [`ensure_index_gitignore`], for
/// `TextIndex::build_with_progress` — a fresh index directory (including a
/// rebuild, which `remove_dir_all`s the old one first) always gets a
/// gitignore from the moment it exists. Folded into this one call, rather
/// than a second statement at the call site, so the fix does not grow
/// `lib.rs` past its size baseline (`scripts/check-file-size.sh`) for what
/// is otherwise a one-line change.
pub(crate) fn create_index_dir(index_dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(index_dir)?;
    ensure_index_gitignore(index_dir);
    Ok(())
}

/// [`crate::index_dir_for`] plus [`ensure_index_gitignore`], for
/// `TextIndex::open_existing` — same "no new line in `lib.rs`" reasoning as
/// [`create_index_dir`], and it backfills a gitignore into an index
/// directory a pre-fix build left without one, not just a fresh build's.
pub(crate) fn opened_index_dir(project_root: &std::path::Path) -> std::path::PathBuf {
    let dir = crate::index_dir_for(project_root);
    ensure_index_gitignore(&dir);
    dir
}
