//! Detection: does this project have this analyzer, and how would it be
//! run?
//!
//! Three independent questions, kept as three functions rather than one
//! that does everything, because a settings page wants to show all three
//! answers separately (which program, which config file, why a declared
//! tool is still marked not installed):
//!
//! - [`find_program`] — probe the manifest's `program-candidates` in order.
//! - [`find_config_file`] — does a config file for this tool exist.
//! - [`composer_require_dev`] — what does `composer.json` declare.
//!
//! [`AnalyzerStatus::describe`] is the settings-page sentence built from
//! the three; its wording is a contract other code (and the manual test
//! matrix, E3) matches on, not prose to be paraphrased.

use std::fs;
use std::path::{Path, PathBuf};

/// Where an analyzer stands relative to one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalyzerStatus {
    /// A program candidate resolved to a real, runnable path.
    Detected { program: PathBuf },
    /// `composer.json` names the package in `require-dev`, but no program
    /// candidate resolved — most commonly because `vendor/` was never
    /// installed. Distinct from [`AnalyzerStatus::NotDetected`] so the
    /// settings page can say *why* nothing runs instead of reporting
    /// nothing at all.
    DeclaredNotInstalled { composer_package: String },
    /// No program candidate resolved and nothing declares this analyzer.
    NotDetected,
}

impl AnalyzerStatus {
    /// The sentence the Analysis settings page and the status bar show.
    ///
    /// The `DeclaredNotInstalled` wording contains the literal phrase
    /// "declared but not installed" — the manual test matrix (E3) and a
    /// later settings-page test both match on that exact substring, so it
    /// is not to be paraphrased even for style.
    pub fn describe(&self, name: &str) -> String {
        match self {
            Self::Detected { program } => format!("{name}: detected at {}", program.display()),
            Self::DeclaredNotInstalled { composer_package } => format!(
                "{name}: declared but not installed ({composer_package} is in \
                 composer.json's require-dev, but vendor/ has no matching binary — \
                 run `composer install`)"
            ),
            Self::NotDetected => format!("{name}: not detected"),
        }
    }
}

/// Probe `candidates` in order against `project_root`, returning the first
/// one that resolves to an existing, runnable file.
///
/// A candidate containing a path separator (`vendor/bin/phpstan`) is
/// resolved relative to `project_root` only — a Composer project's own
/// tools are never confused with a same-named global install. A bare name
/// (`phpstan`) is looked up on `PATH`, the same rule `Command::new` itself
/// uses, so this function's notion of "on PATH" never drifts from what
/// actually spawning the program would find.
pub fn find_program(candidates: &[String], project_root: &Path) -> Option<PathBuf> {
    candidates
        .iter()
        .find_map(|candidate| resolve_one(candidate, project_root))
}

fn resolve_one(candidate: &str, project_root: &Path) -> Option<PathBuf> {
    if candidate.contains('/') || candidate.contains('\\') {
        let path = project_root.join(candidate);
        // On Windows a Composer shim is `vendor/bin/phpstan.bat`, not the
        // bare name in the manifest (C2's job to add those candidates); B3
        // only has to recognise a file that is actually there.
        return existing_file(&path).or_else(|| existing_file(&with_extension(&path, "bat")));
    }
    find_on_path(candidate)
}

fn existing_file(path: &Path) -> Option<PathBuf> {
    path.is_file().then(|| path.to_path_buf())
}

fn with_extension(path: &Path, ext: &str) -> PathBuf {
    let mut with_ext = path.as_os_str().to_owned();
    with_ext.push(".");
    with_ext.push(ext);
    PathBuf::from(with_ext)
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var).find_map(|dir| {
        let candidate = dir.join(program);
        existing_file(&candidate)
    })
}

/// Does a config file for this tool exist? `candidates` is a list of
/// filenames to try, in preference order (e.g. `["phpstan.neon",
/// "phpstan.neon.dist"]`) — deliberately supplied by the caller rather than
/// carried on [`crate::AnalyzerDef`]: the plan reserves per-tool filenames
/// for C2's PHP-specific rows, and this function is the reusable primitive
/// they call.
pub fn find_config_file(project_root: &Path, candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(|name| project_root.join(name))
        .find(|path| path.is_file())
}

/// Package names `composer.json`'s `require-dev` lists, or `None` when
/// there is no `composer.json` (not a Composer project) or it fails to
/// parse (malformed rather than silently "declares nothing").
pub fn composer_require_dev(project_root: &Path) -> Option<Vec<String>> {
    let text = fs::read_to_string(project_root.join("composer.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let table = value.get("require-dev")?.as_object()?;
    Some(table.keys().cloned().collect())
}

/// Combine detection and declaration into one [`AnalyzerStatus`].
///
/// `composer_packages` is the id(s) that, if present in `require-dev`,
/// explain a tool that resolved to nothing — e.g. PHPStan is
/// `phpstan/phpstan`. Left to the caller (C1/C2's job) rather than guessed
/// from the analyzer id, because a manifest id and a Composer package name
/// are not the same string (`phpstan` vs `phpstan/phpstan`).
pub fn status(
    program_candidates: &[String],
    project_root: &Path,
    composer_packages: &[&str],
) -> AnalyzerStatus {
    if let Some(program) = find_program(program_candidates, project_root) {
        return AnalyzerStatus::Detected { program };
    }
    if let Some(declared) = composer_require_dev(project_root) {
        if let Some(matched) = composer_packages
            .iter()
            .find(|pkg| declared.iter().any(|d| d == *pkg))
        {
            return AnalyzerStatus::DeclaredNotInstalled {
                composer_package: matched.to_string(),
            };
        }
    }
    AnalyzerStatus::NotDetected
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn touch_executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn a_project_relative_candidate_resolves_under_the_project_root() {
        let root = project();
        touch_executable(&root.path().join("vendor/bin/phpstan"));
        let found = find_program(&["vendor/bin/phpstan".to_string()], root.path());
        assert_eq!(found, Some(root.path().join("vendor/bin/phpstan")));
    }

    #[test]
    fn a_project_relative_candidate_falls_back_to_its_bat_shim() {
        let root = project();
        touch_executable(&root.path().join("vendor/bin/phpstan.bat"));
        let found = find_program(&["vendor/bin/phpstan".to_string()], root.path());
        assert_eq!(found, Some(root.path().join("vendor/bin/phpstan.bat")));
    }

    #[test]
    fn a_missing_candidate_is_none() {
        let root = project();
        assert_eq!(
            find_program(&["vendor/bin/phpstan".to_string()], root.path()),
            None
        );
    }

    #[test]
    fn config_file_discovery_tries_candidates_in_order() {
        let root = project();
        fs::write(root.path().join("phpstan.neon.dist"), "").unwrap();
        let found = find_config_file(root.path(), &["phpstan.neon", "phpstan.neon.dist"]);
        assert_eq!(found, Some(root.path().join("phpstan.neon.dist")));
    }

    #[test]
    fn no_config_file_is_none() {
        let root = project();
        assert_eq!(find_config_file(root.path(), &["phpstan.neon"]), None);
    }

    #[test]
    fn composer_require_dev_lists_the_declared_packages() {
        let root = project();
        fs::write(
            root.path().join("composer.json"),
            r#"{"require-dev": {"phpstan/phpstan": "^1.0", "squizlabs/php_codesniffer": "^3.0"}}"#,
        )
        .unwrap();
        let mut declared = composer_require_dev(root.path()).unwrap();
        declared.sort();
        assert_eq!(
            declared,
            vec!["phpstan/phpstan", "squizlabs/php_codesniffer"]
        );
    }

    #[test]
    fn no_composer_json_is_none() {
        let root = project();
        assert_eq!(composer_require_dev(root.path()), None);
    }

    #[test]
    fn status_is_detected_when_a_candidate_resolves() {
        let root = project();
        touch_executable(&root.path().join("vendor/bin/phpstan"));
        let s = status(
            &["vendor/bin/phpstan".to_string()],
            root.path(),
            &["phpstan/phpstan"],
        );
        assert_eq!(
            s,
            AnalyzerStatus::Detected {
                program: root.path().join("vendor/bin/phpstan")
            }
        );
    }

    #[test]
    fn status_is_declared_not_installed_when_composer_names_it_but_vendor_is_missing() {
        let root = project();
        fs::write(
            root.path().join("composer.json"),
            r#"{"require-dev": {"phpstan/phpstan": "^1.0"}}"#,
        )
        .unwrap();
        let s = status(
            &["vendor/bin/phpstan".to_string()],
            root.path(),
            &["phpstan/phpstan"],
        );
        assert_eq!(
            s,
            AnalyzerStatus::DeclaredNotInstalled {
                composer_package: "phpstan/phpstan".to_string()
            }
        );
        assert!(s.describe("PHPStan").contains("declared but not installed"));
    }

    #[test]
    fn status_is_not_detected_when_neither_a_candidate_nor_composer_says_anything() {
        let root = project();
        let s = status(
            &["vendor/bin/phpstan".to_string()],
            root.path(),
            &["phpstan/phpstan"],
        );
        assert_eq!(s, AnalyzerStatus::NotDetected);
    }
}
