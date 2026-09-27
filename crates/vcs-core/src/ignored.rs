//! ADR-0064's "`.gitignore` stays visible" half: which paths get the
//! project tree's "ignored" colour.
//!
//! [`Repository::ignored_paths`] answers with `git status
//! --ignored=matching`'s own list, which already names every ignored file
//! individually — but the project tree is lazy (`ProjectTreeModel` loads
//! one directory at a time), so a row for a file two levels under an
//! ignored directory has to resolve on its own, without walking down from
//! that directory's own entry first. [`IgnoredSet`] answers that per-path
//! rather than the caller re-deriving "or one of its ancestors" at every
//! call site.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The repository-relative paths [`crate::Repository::ignored_paths`]
/// reported, queryable per path.
#[derive(Debug, Clone, Default)]
pub struct IgnoredSet {
    paths: HashSet<PathBuf>,
}

impl IgnoredSet {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        IgnoredSet {
            paths: paths.into_iter().collect(),
        }
    }

    /// Whether `path` is ignored: it is itself one of the reported paths,
    /// or one of its ancestors is — a file inside an ignored directory is
    /// ignored too, the same rule git itself applies when deciding whether
    /// to descend into a directory at all.
    pub fn is_ignored(&self, path: &Path) -> bool {
        if self.paths.is_empty() {
            return false;
        }
        path.ancestors()
            .filter(|ancestor| !ancestor.as_os_str().is_empty())
            .any(|ancestor| self.paths.contains(ancestor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_ignores_nothing() {
        let set = IgnoredSet::default();
        assert!(!set.is_ignored(Path::new("dist/app.js")));
    }

    #[test]
    fn an_exact_match_is_ignored() {
        let set = IgnoredSet::new(vec![PathBuf::from("dist/app.js")]);
        assert!(set.is_ignored(Path::new("dist/app.js")));
    }

    #[test]
    fn a_path_nested_under_an_ignored_directory_is_ignored() {
        let set = IgnoredSet::new(vec![PathBuf::from("dist")]);
        assert!(set.is_ignored(Path::new("dist/sub/app.js")));
        assert!(set.is_ignored(Path::new("dist")));
    }

    #[test]
    fn an_unrelated_path_is_not_ignored() {
        let set = IgnoredSet::new(vec![PathBuf::from("dist")]);
        assert!(!set.is_ignored(Path::new("src/main.rs")));
        // A shared prefix that is not a real ancestor must not match —
        // `dist` is not an ancestor of `dist2/app.js`.
        assert!(!set.is_ignored(Path::new("dist2/app.js")));
    }
}
