//! The project tree's test entries (PHPUnit editor runs plan, E6): which
//! temporary `phpunit` configuration a right-clicked file, directory or the
//! project itself stands for, and the PHP package whose PHPUnit setup the
//! run uses.

use std::path::{Path, PathBuf};

use app_config::php::{PhpUnitRunSetting, PHPUNIT_CONFIG_CANDIDATES};
use app_config::RunConfigSetting;
use test_core::filter::relative_to_root;

use crate::phpunit_run::{marker_config, temporary_config};
use crate::tests::MarkerScope;

/// A test file larger than this is not parsed for a right-click.
pub const MAX_PARSED_BYTES: u64 = 1 << 20;

/// Directories holding other people's code: their tests are not the
/// project's to run.
const DEPENDENCY_DIRS: &[&str] = &["vendor", "node_modules"];

const COMPOSER_JSON: &str = "composer.json";

/// The PHP package a tree row belongs to: the project, or one package of a
/// monorepo with its own `composer.json` or PHPUnit configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestPackage {
    pub dir: PathBuf,
    /// The package's PHPUnit configuration file, when it has one.
    pub configuration: Option<PathBuf>,
    /// Whether the package uses PHPUnit or Pest: a PHPUnit configuration,
    /// or either package in its `composer.json`.
    pub has_test_framework: bool,
}

impl TestPackage {
    /// The package `dir` lies in: `dir` or its nearest ancestor inside
    /// `root` with a PHPUnit configuration or a `composer.json`; the project
    /// itself when none has one. A few stats per ancestor, then two reads.
    pub fn containing(root: &Path, dir: &Path) -> Self {
        let dir = dir
            .ancestors()
            .take_while(|ancestor| ancestor.starts_with(root))
            .find(|ancestor| is_package_dir(ancestor))
            .unwrap_or(root);
        let configuration = crate::phpunit_xml::discover(dir, "");
        let has_test_framework = configuration.is_some() || requires_test_framework(dir);
        Self {
            dir: dir.to_path_buf(),
            configuration,
            has_test_framework,
        }
    }
}

fn is_package_dir(dir: &Path) -> bool {
    std::iter::once(COMPOSER_JSON)
        .chain(PHPUNIT_CONFIG_CANDIDATES.iter().copied())
        .any(|name| dir.join(name).is_file())
}

/// Whether `dir`'s `composer.json` requires PHPUnit or Pest; a missing or
/// malformed one does not.
fn requires_test_framework(dir: &Path) -> bool {
    crate::composer::ComposerJson::read(dir)
        .ok()
        .flatten()
        .is_some_and(|composer| {
            composer
                .require
                .iter()
                .any(|r| matches!(r.name.as_str(), "phpunit/phpunit" | "pestphp/pest"))
        })
}

/// Whether a change to `path` can change a [`TestPackage`]: what a cache of
/// them is invalidated on.
pub fn is_package_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == COMPOSER_JSON || PHPUNIT_CONFIG_CANDIDATES.contains(&name))
}

/// A test file's source from disk, `None` when it is unreadable or larger
/// than [`MAX_PARSED_BYTES`] (checked before reading it).
pub fn read_test_source(path: &Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > MAX_PARSED_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// A right-clicked tree row.
#[derive(Debug, Clone, Copy)]
pub struct TreeRow<'a> {
    /// Absolute; the project root for the tree's empty area.
    pub path: &'a Path,
    pub is_dir: bool,
    /// Excluded from the project scope (ADR-0064), itself or an ancestor.
    pub excluded: bool,
}

/// What the tree's entries call a run, for the view to word and translate:
/// the stored configuration name stays data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRunLabel {
    /// The project's whole suite.
    AllTests,
    /// The tests in the directory named.
    Directory(String),
    /// A test class or Pest file, by its own name.
    Named(String),
}

/// The configuration a tree row runs, and what its entries call it.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRun {
    pub config: RunConfigSetting,
    pub label: TreeRunLabel,
}

/// The temporary configuration `row` stands for in the project at `root`,
/// or `None` when the row offers no test run: its package has no test
/// framework, it is excluded or a dependency, it lies outside the project,
/// or it is a file without tests. `read` supplies a PHP file's source (the
/// editor's unsaved text first); `ignore_case` is for Windows roots.
pub fn tree_run(
    root: &Path,
    row: TreeRow<'_>,
    package: &TestPackage,
    read: impl FnOnce(&Path) -> Option<String>,
    ignore_case: bool,
) -> Option<TreeRun> {
    if !package.has_test_framework || row.excluded {
        return None;
    }
    let root_text = root.to_string_lossy();
    let relative = relative_to_root(&row.path.to_string_lossy(), &root_text, ignore_case)?;
    if relative
        .split('/')
        .any(|part| DEPENDENCY_DIRS.contains(&part))
    {
        return None;
    }
    // A monorepo package runs with its own PHPUnit configuration; the
    // project's is the one PHPUnit finds by itself.
    let configuration = package
        .configuration
        .as_deref()
        .filter(|_| package.dir != root)
        .and_then(|file| relative_to_root(&file.to_string_lossy(), &root_text, ignore_case))
        .unwrap_or_default();
    let run = match (row.is_dir, relative.as_str()) {
        // The project root is its whole suite, never a directory scope of
        // `.`: one "All tests" entry, not two.
        (true, ".") => return Some(all_tests()),
        (true, _) => directory_run(&relative),
        (false, _) => file_run(row.path, &relative, read)?,
    };
    Some(TreeRun {
        config: with_configuration(run.config, configuration),
        label: run.label,
    })
}

fn all_tests() -> TreeRun {
    TreeRun {
        config: temporary_config(
            PhpUnitRunSetting {
                scope: "config".into(),
                ..PhpUnitRunSetting::default()
            },
            Some("All tests".into()),
        ),
        label: TreeRunLabel::AllTests,
    }
}

fn directory_run(relative: &str) -> TreeRun {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    TreeRun {
        config: temporary_config(
            PhpUnitRunSetting {
                scope: "directory".into(),
                path: relative.into(),
                ..PhpUnitRunSetting::default()
            },
            None,
        ),
        label: TreeRunLabel::Directory(name.to_string()),
    }
}

/// PhpStorm runs a PHPUnit file as its class, a Pest file (no class to
/// name) as the file.
fn file_run(
    path: &Path,
    relative: &str,
    read: impl FnOnce(&Path) -> Option<String>,
) -> Option<TreeRun> {
    if syntax_core::language_for_path(path).id() != "php" {
        return None;
    }
    let all = crate::tests::markers(&read(path)?);
    if all.is_empty() {
        return None;
    }
    let config = match all
        .iter()
        .find(|m| m.scope == MarkerScope::Group && !m.class.is_empty())
    {
        Some(class) => marker_config(class, Some(relative)).ok()?,
        None => temporary_config(
            PhpUnitRunSetting {
                scope: "file".into(),
                path: relative.into(),
                ..PhpUnitRunSetting::default()
            },
            None,
        ),
    };
    Some(TreeRun {
        label: TreeRunLabel::Named(config.name.clone()),
        config,
    })
}

/// `config` run with the PHPUnit configuration file `configuration`
/// (project-relative); unchanged when that is blank.
fn with_configuration(config: RunConfigSetting, configuration: String) -> RunConfigSetting {
    if configuration.is_empty() {
        return config;
    }
    let setting = PhpUnitRunSetting {
        configuration,
        ..config.phpunit.unwrap_or_default()
    };
    temporary_config(setting, Some(config.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASS_SOURCE: &str = "<?php\nnamespace App;\nclass GreeterTest extends TestCase\n{\n    public function testGreets() {}\n}\n";

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("phpunit.xml"), "<phpunit/>").unwrap();
        dir
    }

    fn run(root: &Path, path: &Path, is_dir: bool, text: Option<&str>) -> Option<TreeRun> {
        let package = TestPackage::containing(root, if is_dir { path } else { path.parent()? });
        let row = TreeRow {
            path,
            is_dir,
            excluded: false,
        };
        tree_run(root, row, &package, |_| text.map(str::to_string), false)
    }

    #[test]
    fn tree_rows_become_directory_class_file_and_project_runs() {
        let dir = project();
        let root = dir.path();

        let unit = run(root, &root.join("tests/Unit"), true, None).unwrap();
        assert_eq!(unit.config.name, "Tests in 'Unit'");
        assert_eq!(unit.label, TreeRunLabel::Directory("Unit".into()));
        assert!(unit.config.temporary);
        let setting = unit.config.phpunit.clone().unwrap();
        assert_eq!(
            (setting.path.as_str(), setting.configuration.as_str()),
            ("tests/Unit", "")
        );

        let class = run(
            root,
            &root.join("tests/GreeterTest.php"),
            false,
            Some(CLASS_SOURCE),
        )
        .unwrap();
        assert_eq!(class.label, TreeRunLabel::Named("GreeterTest".into()));
        assert_eq!(class.config.phpunit.unwrap().scope, "class");

        let pest = "<?php\nit('greets', fn () => 1);\n";
        let pest = run(root, &root.join("tests/Greets.php"), false, Some(pest)).unwrap();
        assert_eq!(pest.label, TreeRunLabel::Named("Greets.php".into()));
        assert_eq!(pest.config.phpunit.unwrap().scope, "file");

        let subject = "<?php\nclass Greeter {}\n";
        assert!(run(root, &root.join("src/Greeter.php"), false, Some(subject)).is_none());
        assert!(run(root, &root.join("README.md"), false, Some(CLASS_SOURCE)).is_none());
        // Unreadable (or too large to read): nothing to offer.
        assert!(run(root, &root.join("tests/GreeterTest.php"), false, None).is_none());

        // Running a directory twice reuses one entry.
        let again = run(root, &root.join("tests/Unit"), true, None).unwrap();
        assert_eq!(again.config.id, unit.config.id);
    }

    #[test]
    fn the_project_root_is_its_whole_suite_not_a_directory_of_dot() {
        let dir = project();
        for path in [dir.path().to_path_buf(), dir.path().join("")] {
            let all = run(dir.path(), &path, true, None).unwrap();
            assert_eq!(all.label, TreeRunLabel::AllTests);
            assert_eq!(all.config.name, "All tests");
            assert_eq!(all.config.phpunit.unwrap().scope, "config");
        }
    }

    #[test]
    fn excluded_dependency_and_outside_rows_offer_nothing() {
        let dir = project();
        let root = dir.path();
        assert!(run(root, &root.join("vendor/acme/lib/tests"), true, None).is_none());
        assert!(run(root, &root.join("web/node_modules/x"), true, None).is_none());
        assert!(run(root, Path::new("/elsewhere/tests"), true, None).is_none());
        let package = TestPackage::containing(root, root);
        let excluded = TreeRow {
            path: &root.join("build"),
            is_dir: true,
            excluded: true,
        };
        assert!(tree_run(root, excluded, &package, |_| None, false).is_none());
    }

    #[test]
    fn a_project_without_a_test_framework_offers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(run(root, &root.join("tests"), true, None).is_none());
        std::fs::write(root.join("composer.json"), "{ not json").unwrap();
        assert!(run(root, &root.join("tests"), true, None).is_none());
    }

    #[test]
    fn a_test_framework_is_a_phpunit_file_or_a_composer_requirement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let has = || TestPackage::containing(root, root).has_test_framework;
        assert!(!has());
        std::fs::write(
            root.join("composer.json"),
            r#"{"require": {"phpunit/phpunit": "^11"}}"#,
        )
        .unwrap();
        assert!(has());
        std::fs::write(
            root.join("composer.json"),
            r#"{"require-dev": {"pestphp/pest": "^3"}}"#,
        )
        .unwrap();
        assert!(has());
        std::fs::remove_file(root.join("composer.json")).unwrap();
        std::fs::write(root.join("phpunit.dist.xml"), "<phpunit/>").unwrap();
        assert!(has());
    }

    #[test]
    fn a_monorepo_package_runs_with_its_own_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let package = root.join("packages/mailer");
        std::fs::create_dir_all(package.join("tests")).unwrap();
        // The project itself has no test framework; the package does.
        std::fs::write(root.join("composer.json"), "{}").unwrap();
        std::fs::write(package.join("phpunit.xml.dist"), "<phpunit/>").unwrap();

        let found = TestPackage::containing(root, &package.join("tests"));
        assert_eq!(found.dir, package);
        assert!(found.has_test_framework);

        let tests = run(root, &package.join("tests"), true, None).unwrap();
        let setting = tests.config.phpunit.unwrap();
        assert_eq!(setting.path, "packages/mailer/tests");
        assert_eq!(setting.configuration, "packages/mailer/phpunit.xml.dist");
        assert_eq!(tests.config.name, "Tests in 'tests'");

        let class = run(
            root,
            &package.join("tests/GreeterTest.php"),
            false,
            Some(CLASS_SOURCE),
        )
        .unwrap();
        assert_eq!(
            class.config.phpunit.unwrap().configuration,
            "packages/mailer/phpunit.xml.dist"
        );
        // The project's own directories still offer nothing.
        assert!(run(root, &root.join("tests"), true, None).is_none());
    }

    #[test]
    fn package_files_are_composer_json_and_phpunit_configurations() {
        for name in [
            "composer.json",
            "phpunit.xml",
            "phpunit.xml.dist",
            "phpunit.dist.xml",
        ] {
            assert!(
                is_package_file(&Path::new("/p/packages/a").join(name)),
                "{name}"
            );
        }
        assert!(!is_package_file(Path::new("/p/composer.lock")));
        assert!(!is_package_file(Path::new("/p/tests/FooTest.php")));
    }

    #[test]
    fn a_test_source_over_the_size_cap_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("SmallTest.php");
        std::fs::write(&small, CLASS_SOURCE).unwrap();
        assert_eq!(read_test_source(&small).as_deref(), Some(CLASS_SOURCE));
        let large = dir.path().join("LargeTest.php");
        std::fs::write(&large, vec![b' '; MAX_PARSED_BYTES as usize + 1]).unwrap();
        assert_eq!(read_test_source(&large), None);
        assert_eq!(read_test_source(&dir.path().join("Missing.php")), None);
    }
}
