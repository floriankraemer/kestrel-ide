//! The local repository index (D3): every `group → artifact → versions`
//! this machine already has on disk, from Maven's local repository and
//! Gradle's module cache. Built once per sync (`gradle::sync`/
//! `maven::sync` run on the sync thread already, B1) and kept in the
//! service that owns the synced [`super::super::model::BuildModel`] — not
//! cheap enough to rebuild per keystroke, which is why D5's completion
//! takes a `&RepoIndex` rather than a path to walk.
//!
//! Two directory layouts, walked separately and merged:
//!
//! * Maven's `~/.m2/repository/<group, slash-per-segment>/<artifact>/
//!   <version>/<artifact>-<version>.{pom,jar}` — the group is the path
//!   itself, so the walk recurses until a directory's children are all
//!   version directories (each holding the artifact's own POM or JAR),
//!   and stops there rather than descending into a version directory's
//!   own files.
//! * Gradle's `~/.gradle/caches/modules-2/files-2.1/<group>/<artifact>/
//!   <version>/<hash>/<file>` — a fixed three-level shape (the group is
//!   one directory name, already dotted), so the walk is bounded by
//!   construction and never opens a hash directory.
//!
//! Every walk skips dot-prefixed directory names (`.cache`,
//! `.gradle-test-kit`, stray VCS directories some caches accumulate).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// `group → artifact → the versions found on disk for it`, deduplicated
/// and sorted lexicographically for deterministic iteration — *not* a
/// semantic version order, which is `editing::versions`' job (D6), not
/// this index's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoIndex {
    entries: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

impl RepoIndex {
    /// Every group id this index has at least one artifact for.
    pub fn groups(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Every artifact id known under `group`.
    pub fn artifacts(&self, group: &str) -> impl Iterator<Item = &str> {
        self.entries
            .get(group)
            .into_iter()
            .flat_map(|artifacts| artifacts.keys().map(String::as_str))
    }

    /// The versions found on disk for `group:artifact`, lexicographically
    /// sorted. Empty when this index has never seen the coordinate.
    pub fn versions(&self, group: &str, artifact: &str) -> Vec<&str> {
        self.entries
            .get(group)
            .and_then(|artifacts| artifacts.get(artifact))
            .map(|versions| versions.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn insert(&mut self, group: String, artifact: String, version: String) {
        self.entries
            .entry(group)
            .or_default()
            .entry(artifact)
            .or_default()
            .insert(version);
    }
}

/// A sanity bound against a pathological or symlink-cyclic directory tree —
/// no real Maven group nests this deep. Not a normal-case limit: even a
/// group like `org.springframework.boot.autoconfigure` is 4 segments.
const MAX_MAVEN_DEPTH: usize = 24;

/// Walk `maven_repository` (Maven's local repo root) and `gradle_modules`
/// (Gradle's `files-2.1` root) and merge both into one index. Either path
/// not existing (a tool never run locally) is not an error — that half of
/// the index is simply empty.
pub fn build(maven_repository: &Path, gradle_modules: &Path) -> RepoIndex {
    let mut index = RepoIndex::default();
    walk_maven_repository(maven_repository, &mut index);
    walk_gradle_modules(gradle_modules, &mut index);
    index
}

fn is_dot_named(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

/// Review fix #11: `DirEntry::file_type()`, not `Path::is_dir()` — the
/// latter is `fs::metadata` under the hood, which *follows* a symlink.
/// A symlink inside `~/.gradle`/`~/.m2` pointing back at an ancestor
/// directory (real repository caches accumulate a few, deliberately or
/// not) would otherwise have this walk follow it and recurse forever;
/// `file_type()` reports what the directory entry itself is, so a
/// symlink — even one that points at a real directory — is simply
/// skipped rather than walked into.
fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path())
        .filter(|path| !is_dot_named(path))
        .collect()
}

/// Does `version_dir` (a candidate version directory under an artifact
/// directory named `artifact`) actually hold that artifact's own POM or
/// JAR — the proof it is a real version directory and not, say, a
/// metadata-only directory some Maven plugin left behind?
fn holds_artifact_files(version_dir: &Path, artifact: &str, version: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(version_dir) else {
        return false;
    };
    let prefix = format!("{artifact}-{version}");
    entries.filter_map(|e| e.ok()).any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with(&prefix) && (name.ends_with(".pom") || name.ends_with(".jar"))
    })
}

/// The repository root itself is never a group segment — only walk its
/// children as the first segment, or an empty-`.m2` temp dir's own name
/// (or the real `repository` directory's name) would leak into every
/// group id this walk produces.
fn walk_maven_repository(dir: &Path, index: &mut RepoIndex) {
    for child in read_subdirs(dir) {
        walk_maven_dir(&child, &mut Vec::new(), index, 0);
    }
}

fn walk_maven_dir(
    dir: &Path,
    group_segments: &mut Vec<String>,
    index: &mut RepoIndex,
    depth: usize,
) {
    if depth > MAX_MAVEN_DEPTH {
        return;
    }
    let Some(artifact) = dir.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let subdirs = read_subdirs(dir);
    if subdirs.is_empty() {
        return;
    }

    let version_dirs: Vec<&str> = subdirs
        .iter()
        .filter_map(|version_dir| version_dir.file_name().and_then(|n| n.to_str()))
        .filter(|version| holds_artifact_files(dir.join(version).as_path(), artifact, version))
        .collect();

    if !version_dirs.is_empty() && !group_segments.is_empty() {
        let group = group_segments.join(".");
        for version in version_dirs {
            index.insert(group.clone(), artifact.to_string(), version.to_string());
        }
        // A real artifact directory's children are version directories —
        // nothing worth descending into below them (jars, checksums, the
        // version's own further subdirectories for a classified build).
        return;
    }

    group_segments.push(artifact.to_string());
    for subdir in &subdirs {
        walk_maven_dir(subdir, group_segments, index, depth + 1);
    }
    group_segments.pop();
}

fn walk_gradle_modules(files21_root: &Path, index: &mut RepoIndex) {
    for group_dir in read_subdirs(files21_root) {
        let Some(group) = group_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        for artifact_dir in read_subdirs(&group_dir) {
            let Some(artifact) = artifact_dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            for version_dir in read_subdirs(&artifact_dir) {
                let Some(version) = version_dir.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                index.insert(group.to_string(), artifact.to_string(), version.to_string());
            }
        }
    }
}

/// The Maven local repository to walk: `[build_tools.maven].local_repository`
/// when the user set one, else Maven's own default `~/.m2/repository`.
pub fn default_maven_repository(local_repository: Option<&str>, home: &Path) -> PathBuf {
    match local_repository {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => home.join(".m2").join("repository"),
    }
}

/// The Gradle module cache to walk: `$GRADLE_USER_HOME/caches/modules-2/
/// files-2.1` when that environment variable is set, else Gradle's own
/// default `~/.gradle/caches/modules-2/files-2.1`.
pub fn default_gradle_modules(gradle_user_home_env: Option<&str>, home: &Path) -> PathBuf {
    let base = match gradle_user_home_env {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => home.join(".gradle"),
    };
    base.join("caches").join("modules-2").join("files-2.1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn maven_repository_yields_group_artifact_versions() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        touch(&repo.join("com/google/guava/guava/32.1.3-jre/guava-32.1.3-jre.pom"));
        touch(&repo.join("com/google/guava/guava/32.1.3-jre/guava-32.1.3-jre.jar"));
        touch(&repo.join("com/google/guava/guava/33.0.0-jre/guava-33.0.0-jre.jar"));

        let index = build(repo, Path::new("/nonexistent/gradle-modules"));
        let mut versions = index.versions("com.google.guava", "guava");
        versions.sort();
        assert_eq!(versions, vec!["32.1.3-jre", "33.0.0-jre"]);
    }

    #[test]
    fn maven_repository_skips_dot_directories() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        touch(&repo.join(".cache/whatever/1.0/whatever-1.0.jar"));
        touch(&repo.join("com/example/lib/1.0/lib-1.0.jar"));

        let index = build(repo, Path::new("/nonexistent/gradle-modules"));
        assert!(index.groups().all(|g| g != ".cache"));
        assert_eq!(index.versions("com.example", "lib"), vec!["1.0"]);
    }

    /// Review fix #11: a symlinked directory — even a loop pointing back
    /// at an ancestor, the pathological case this rule exists for — is
    /// never followed. `Path::is_dir()` would recurse into it forever;
    /// `DirEntry::file_type()` reports the symlink for what it is.
    #[test]
    fn read_subdirs_does_not_follow_a_symlinked_directory() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        touch(&repo.join("com/example/lib/1.0/lib-1.0.jar"));
        // A loop: com/example/loop -> com/example (an ancestor).
        std::os::unix::fs::symlink(repo.join("com/example"), repo.join("com/example/loop"))
            .unwrap();

        // If the symlink were followed, this would recurse without bound
        // and the test would hang rather than fail — the walk finishing
        // at all, quickly, is itself part of what this proves.
        let index = build(repo, Path::new("/nonexistent/gradle-modules"));
        assert_eq!(index.versions("com.example", "lib"), vec!["1.0"]);
    }

    #[test]
    fn maven_repository_does_not_descend_past_a_version_directory() {
        // A directory *inside* a version directory that itself looks like
        // a nested artifact/version pair (a classifier subfolder some
        // plugin left behind) must not be picked up as its own entry.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        touch(&repo.join("com/example/lib/1.0/lib-1.0.jar"));
        touch(&repo.join("com/example/lib/1.0/nested/2.0/nested-2.0.jar"));

        let index = build(repo, Path::new("/nonexistent/gradle-modules"));
        assert_eq!(index.versions("com.example", "lib"), vec!["1.0"]);
        assert!(index.versions("com.example.lib.1.0", "nested").is_empty());
    }

    #[test]
    fn gradle_modules_cache_yields_group_artifact_versions() {
        let dir = tempfile::tempdir().unwrap();
        let modules = dir.path();
        touch(&modules.join("com.google.guava/guava/32.1.3-jre/deadbeef/guava-32.1.3-jre.jar"));
        touch(&modules.join("com.google.guava/guava/33.0.0-jre/cafef00d/guava-33.0.0-jre.jar"));

        let index = build(Path::new("/nonexistent/m2"), modules);
        let mut versions = index.versions("com.google.guava", "guava");
        versions.sort();
        assert_eq!(versions, vec!["32.1.3-jre", "33.0.0-jre"]);
    }

    #[test]
    fn gradle_modules_cache_skips_dot_directories() {
        let dir = tempfile::tempdir().unwrap();
        let modules = dir.path();
        touch(&modules.join(".lock/x/1.0/h/x-1.0.jar"));
        touch(&modules.join("com.example/lib/1.0/h/lib-1.0.jar"));

        let index = build(Path::new("/nonexistent/m2"), modules);
        assert!(index.groups().all(|g| g != ".lock"));
        assert_eq!(index.versions("com.example", "lib"), vec!["1.0"]);
    }

    #[test]
    fn missing_repositories_yield_an_empty_index_not_an_error() {
        let index = build(
            Path::new("/nonexistent/m2"),
            Path::new("/nonexistent/gradle-modules"),
        );
        assert!(index.is_empty());
    }

    #[test]
    fn same_coordinate_in_both_caches_merges_into_one_entry() {
        let m2 = tempfile::tempdir().unwrap();
        touch(&m2.path().join("com/example/lib/1.0/lib-1.0.jar"));
        let gradle = tempfile::tempdir().unwrap();
        touch(&gradle.path().join("com.example/lib/2.0/h/lib-2.0.jar"));

        let index = build(m2.path(), gradle.path());
        let mut versions = index.versions("com.example", "lib");
        versions.sort();
        assert_eq!(versions, vec!["1.0", "2.0"]);
    }

    #[test]
    fn default_maven_repository_prefers_the_configured_path() {
        let home = Path::new("/home/dev");
        assert_eq!(
            default_maven_repository(Some("/custom/repo"), home),
            PathBuf::from("/custom/repo")
        );
        assert_eq!(
            default_maven_repository(None, home),
            PathBuf::from("/home/dev/.m2/repository")
        );
        assert_eq!(
            default_maven_repository(Some("  "), home),
            PathBuf::from("/home/dev/.m2/repository")
        );
    }

    #[test]
    fn default_gradle_modules_prefers_the_environment_variable() {
        let home = Path::new("/home/dev");
        assert_eq!(
            default_gradle_modules(Some("/custom/gradle-home"), home),
            PathBuf::from("/custom/gradle-home/caches/modules-2/files-2.1")
        );
        assert_eq!(
            default_gradle_modules(None, home),
            PathBuf::from("/home/dev/.gradle/caches/modules-2/files-2.1")
        );
    }
}
