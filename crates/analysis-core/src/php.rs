//! PHP-specific detection data (the PHP tooling plan's C2): what `detect.rs`
//! needs a caller to supply for `phpstan`/`phpcs` — `find_config_file`'s own
//! doc comment already says the candidate list is the caller's job, and
//! `status`'s `composer_packages` parameter says the same about the
//! id-to-package table (B8 shipped it as `&[]`, a stated stub for this
//! module to fill in). Nothing here is new detection *machinery*; it is
//! PHPStan's and PHPCS's own answers, kept in one place so neither is
//! guessed twice.

use std::path::{Path, PathBuf};

/// PHPStan's own automatic config-file discovery order (no `-c` given):
/// `phpstan.neon`, then `phpstan.neon.dist`, then `phpstan.dist.neon` —
/// confirmed against `https://phpstan.org/config-reference`. The usual
/// project convention is `phpstan.neon.dist` (or the newer `.dist.neon`
/// spelling) under version control, with an untracked `phpstan.neon`
/// overriding it locally.
pub const PHPSTAN_CONFIG_CANDIDATES: &[&str] =
    &["phpstan.neon", "phpstan.neon.dist", "phpstan.dist.neon"];

/// PHP_CodeSniffer's own automatic config-file discovery order: the dotfile
/// spelling before the bare one, and the non-`.dist` spelling before the
/// `.dist` one — `.phpcs.xml`, `phpcs.xml`, `.phpcs.xml.dist`,
/// `phpcs.xml.dist` (`squizlabs/PHP_CodeSniffer`'s `Config` source and its
/// wiki's Advanced Usage page). Deliberately *not* the plan's own
/// stated-as-uncertain order — this is the tool's real one.
pub const PHPCS_CONFIG_CANDIDATES: &[&str] = &[
    ".phpcs.xml",
    "phpcs.xml",
    ".phpcs.xml.dist",
    "phpcs.xml.dist",
];

/// PHPUnit's own automatic config-file discovery order (no `-c` given,
/// PHPUnit's `Configuration::locateConfigurationFile`): the bare filename
/// before its `.dist` counterpart, since a tracked default should not
/// shadow a developer's own untracked override.
pub const PHPUNIT_CONFIG_CANDIDATES: &[&str] = &["phpunit.xml", "phpunit.xml.dist"];

/// The Composer package an analyzer id corresponds to, for
/// [`crate::status`]'s `composer_packages` argument — the table B8 left as
/// a stub (`analyzer_rows()` passed `&[]`, so a declared-but-uninstalled
/// tool reported `NotDetected` rather than saying why).
pub fn composer_package(analyzer_id: &str) -> Option<&'static str> {
    match analyzer_id {
        "phpstan" => Some("phpstan/phpstan"),
        "phpcs" => Some("squizlabs/php_codesniffer"),
        // Not just an analyzer id: `test-core`'s D7 caller looks this up
        // for `phpunit` too, since it is the same "declared but not
        // installed" question `AnalyzerStatus` already answers for a
        // linter, applied to a test framework's own program candidates.
        "phpunit" => Some("phpunit/phpunit"),
        _ => None,
    }
}

/// Does `program` need to be run through a PHP binary rather than executed
/// directly?
///
/// True for a `.phar` archive (never itself executable without a registered
/// file association) and, on Unix, for any resolved candidate that lacks
/// the executable bit — a `vendor/bin` shim Composer installed without `+x`,
/// which happens on some filesystems and some CI checkouts. Windows has no
/// executable bit to check; a resolved `.bat` shim already runs directly
/// through `cmd`, so only the `.phar` case applies there.
pub fn needs_php_prefix(program: &Path) -> bool {
    is_phar(program) || !is_executable(program)
}

fn is_phar(program: &Path) -> bool {
    program.extension().is_some_and(|ext| ext == "phar")
}

#[cfg(unix)]
fn is_executable(program: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(program)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_program: &Path) -> bool {
    true
}

/// Resolve how to actually invoke `program`, given an optional configured
/// PHP binary override.
///
/// Some setups cannot execute the resolved program directly — Windows
/// without a registered `.phar` file association, or a filesystem where the
/// `vendor/bin` shim was installed without its executable bit — and need
/// `php vendor/bin/phpstan` rather than `vendor/bin/phpstan`. Returns the
/// program to spawn and the argv prefix that must precede an analyzer's own
/// arguments; the pair is `(program, [])` unchanged whenever no prefix is
/// configured or none is needed.
pub fn invocation(program: &Path, php_binary: Option<&str>) -> (PathBuf, Vec<String>) {
    match php_binary {
        Some(php) if needs_php_prefix(program) => (
            PathBuf::from(php),
            vec![program.to_string_lossy().into_owned()],
        ),
        _ => (program.to_path_buf(), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phpstan_maps_to_its_composer_package() {
        assert_eq!(composer_package("phpstan"), Some("phpstan/phpstan"));
    }

    #[test]
    fn phpcs_maps_to_its_composer_package() {
        assert_eq!(composer_package("phpcs"), Some("squizlabs/php_codesniffer"));
    }

    #[test]
    fn phpunit_maps_to_its_composer_package() {
        assert_eq!(composer_package("phpunit"), Some("phpunit/phpunit"));
    }

    #[test]
    fn an_unknown_analyzer_id_has_no_composer_package() {
        assert_eq!(composer_package("eslint"), None);
    }

    #[test]
    fn a_phar_always_needs_the_php_prefix() {
        assert!(needs_php_prefix(Path::new("vendor/phpstan.phar")));
    }

    #[cfg(unix)]
    #[test]
    fn an_executable_shim_needs_no_prefix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phpstan");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!needs_php_prefix(&path));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_executable_shim_needs_the_php_prefix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phpstan");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(needs_php_prefix(&path));
    }

    #[test]
    fn no_php_binary_configured_invokes_the_program_directly() {
        let (program, args) = invocation(Path::new("vendor/phpstan.phar"), None);
        assert_eq!(program, PathBuf::from("vendor/phpstan.phar"));
        assert!(args.is_empty());
    }

    #[test]
    fn a_configured_php_binary_prefixes_a_phar() {
        let (program, args) = invocation(Path::new("vendor/phpstan.phar"), Some("php"));
        assert_eq!(program, PathBuf::from("php"));
        assert_eq!(args, vec!["vendor/phpstan.phar".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn a_configured_php_binary_leaves_an_executable_shim_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phpstan");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (program, args) = invocation(&path, Some("php"));
        assert_eq!(program, path);
        assert!(args.is_empty());
    }

    #[test]
    fn phpstan_config_candidates_match_its_own_discovery_order() {
        assert_eq!(
            PHPSTAN_CONFIG_CANDIDATES,
            &["phpstan.neon", "phpstan.neon.dist", "phpstan.dist.neon"]
        );
    }

    #[test]
    fn phpcs_config_candidates_match_its_own_discovery_order() {
        assert_eq!(
            PHPCS_CONFIG_CANDIDATES,
            &[
                ".phpcs.xml",
                "phpcs.xml",
                ".phpcs.xml.dist",
                "phpcs.xml.dist"
            ]
        );
    }

    #[test]
    fn phpunit_config_candidates_match_its_own_discovery_order() {
        assert_eq!(
            PHPUNIT_CONFIG_CANDIDATES,
            &["phpunit.xml", "phpunit.xml.dist"]
        );
    }
}
