//! The PHPUnit configuration file (PHPUnit editor runs plan, E5): which
//! test suites it declares, which groups exist, and where the tests live.
//!
//! [`parse`] is a pure function over the file's text (PHPUnit 9 to 12 share
//! the elements read here). [`discover`] finds the file the way PHPUnit
//! does, and [`model`] joins the two with a bounded scan of the suites' test
//! files for `#[Group]` attributes and `@group` docblock tags, which is what
//! the run configuration editor's pickers offer.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use analysis_core::PHPUNIT_CONFIG_CANDIDATES;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// PHPUnit's default test file suffix for a `<directory>` without `suffix`.
const DEFAULT_SUFFIX: &str = "Test.php";
/// The scan stops after this many test files, so a suite that points at
/// `vendor` cannot stall the editor.
const MAX_SCANNED_FILES: usize = 2000;
/// A test file larger than this is not read.
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// A `<directory>` of a test suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuiteDirectory {
    pub path: String,
    /// The file name suffix a test file must have.
    pub suffix: String,
}

/// One `<testsuite>`; paths are relative to the configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TestSuite {
    pub name: String,
    pub directories: Vec<SuiteDirectory>,
    pub files: Vec<String>,
    pub excludes: Vec<String>,
}

/// What a configuration file declares that the run configuration uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhpUnitConfig {
    pub bootstrap: Option<String>,
    pub suites: Vec<TestSuite>,
    pub include_groups: Vec<String>,
    pub exclude_groups: Vec<String>,
}

/// The configuration is not well-formed XML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed XML: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Default)]
struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Element>,
    text: String,
}

impl Element {
    fn attribute(&self, key: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |child| child.name == name)
    }

    fn texts_of(&self, name: &str) -> Vec<String> {
        self.children_named(name)
            .map(|child| child.text.trim().to_string())
            .filter(|text| !text.is_empty())
            .collect()
    }
}

fn element_of(tag: &BytesStart<'_>) -> Result<Element, ParseError> {
    let mut attributes = Vec::new();
    for attribute in tag.attributes() {
        let attribute = attribute.map_err(|error| ParseError(error.to_string()))?;
        // `normalized_value` rather than `unescape_value`: the latter does not
        // exist when another crate enables quick-xml's `encoding` feature
        // (see `analysis-core::checkstyle`).
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|error| ParseError(error.to_string()))?;
        attributes.push((
            String::from_utf8_lossy(attribute.key.as_ref()).into_owned(),
            value.into_owned(),
        ));
    }
    Ok(Element {
        name: String::from_utf8_lossy(tag.name().as_ref()).into_owned(),
        attributes,
        ..Element::default()
    })
}

fn read_tree(text: &str) -> Result<Element, ParseError> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Element> = Vec::new();
    let mut root = None;
    let attach =
        |stack: &mut Vec<Element>, root: &mut Option<Element>, element: Element| match stack
            .last_mut()
        {
            Some(parent) => parent.children.push(element),
            None => *root = Some(element),
        };
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => stack.push(element_of(&tag)?),
            Ok(Event::Empty(tag)) => attach(&mut stack, &mut root, element_of(&tag)?),
            Ok(Event::Text(raw)) => {
                if let Some(top) = stack.last_mut() {
                    let raw = raw.decode().map_err(|e| ParseError(e.to_string()))?;
                    match quick_xml::escape::unescape(&raw) {
                        Ok(text) => top.text.push_str(&text),
                        Err(_) => top.text.push_str(&raw),
                    }
                }
            }
            Ok(Event::End(_)) => {
                let element = stack
                    .pop()
                    .ok_or_else(|| ParseError("unbalanced closing tag".into()))?;
                attach(&mut stack, &mut root, element);
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(ParseError(error.to_string())),
        }
    }
    root.ok_or_else(|| ParseError("no root element".into()))
}

fn suite_of(element: &Element) -> Option<TestSuite> {
    let name = element.attribute("name")?.trim();
    if name.is_empty() {
        return None;
    }
    let directories = element
        .children_named("directory")
        .filter(|directory| !directory.text.trim().is_empty())
        .map(|directory| SuiteDirectory {
            path: directory.text.trim().to_string(),
            suffix: directory
                .attribute("suffix")
                .unwrap_or(DEFAULT_SUFFIX)
                .to_string(),
        })
        .collect();
    Some(TestSuite {
        name: name.to_string(),
        directories,
        files: element.texts_of("file"),
        excludes: element.texts_of("exclude"),
    })
}

/// Read the suites, groups and bootstrap out of a PHPUnit configuration.
pub fn parse(text: &str) -> Result<PhpUnitConfig, ParseError> {
    let root = read_tree(text)?;
    let groups = root.children_named("groups").next();
    let group_list = |kind: &str| {
        groups
            .and_then(|groups| groups.children_named(kind).next())
            .map(|list| list.texts_of("group"))
            .unwrap_or_default()
    };
    Ok(PhpUnitConfig {
        bootstrap: root
            .attribute("bootstrap")
            .map(str::trim)
            .filter(|bootstrap| !bootstrap.is_empty())
            .map(str::to_string),
        suites: root
            .children_named("testsuites")
            .flat_map(|suites| suites.children_named("testsuite"))
            .filter_map(suite_of)
            .collect(),
        include_groups: group_list("include"),
        exclude_groups: group_list("exclude"),
    })
}

/// The configuration file PHPUnit would use: `explicit` (a file, or a
/// directory searched like the project root) relative to `root`, else the
/// first of the default names in `root`.
pub fn discover(root: &Path, explicit: &str) -> Option<PathBuf> {
    let explicit = explicit.trim();
    let base = if explicit.is_empty() {
        root.to_path_buf()
    } else {
        root.join(explicit)
    };
    if base.is_file() {
        return Some(base);
    }
    if explicit.is_empty() || base.is_dir() {
        return PHPUNIT_CONFIG_CANDIDATES
            .iter()
            .map(|name| base.join(name))
            .find(|candidate| candidate.is_file());
    }
    None
}

/// Every group name a test file declares, through `#[Group('x')]`
/// attributes (any namespace qualification) and `@group x` docblock tags.
pub fn groups_in(source: &str) -> BTreeSet<String> {
    let mut groups = BTreeSet::new();
    let bytes = source.as_bytes();
    let is_name_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    for (at, _) in source.match_indices("Group(") {
        // `Group(`, not `DataGroup(`; a namespace separator before it is fine.
        if at > 0 && is_name_byte(bytes[at - 1]) {
            continue;
        }
        let rest = source[at + "Group(".len()..].trim_start();
        let Some(quote) = rest.chars().next().filter(|c| matches!(c, '\'' | '"')) else {
            continue;
        };
        if let Some(name) = rest[1..].split(quote).next().filter(|n| !n.is_empty()) {
            groups.insert(name.to_string());
        }
    }
    for (at, _) in source.match_indices("@group") {
        let rest = &source[at + "@group".len()..];
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some(name) = rest.split_whitespace().next().filter(|n| *n != "*/") {
            groups.insert(name.to_string());
        }
    }
    groups
}

fn glob_free(path: &str) -> bool {
    !path.contains(['*', '?', '['])
}

/// The test files of `suite` under `config_dir`, at most `limit`.
// ponytail: a glob in a suite path (`tests/*/Unit`) is skipped, not expanded;
// add a glob walk when a real configuration needs it.
fn suite_files(config_dir: &Path, suite: &TestSuite, limit: usize) -> Vec<PathBuf> {
    let excluded: Vec<PathBuf> = suite
        .excludes
        .iter()
        .map(|exclude| config_dir.join(exclude))
        .collect();
    let mut found: Vec<PathBuf> = suite
        .files
        .iter()
        .filter(|file| glob_free(file))
        .map(|file| config_dir.join(file))
        .filter(|file| file.is_file())
        .collect();
    for directory in suite.directories.iter().filter(|d| glob_free(&d.path)) {
        let mut pending = vec![config_dir.join(&directory.path)];
        while let Some(dir) = pending.pop() {
            if found.len() >= limit || excluded.iter().any(|ex| dir.starts_with(ex)) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.to_string_lossy().ends_with(&directory.suffix)
                    && !excluded.iter().any(|ex| path.starts_with(ex))
                {
                    found.push(path);
                }
            }
        }
    }
    found.truncate(limit);
    found
}

/// What the run configuration editor offers for a project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhpUnitModel {
    /// The configuration file used, relative to the project root (`/`).
    pub file: Option<String>,
    pub suites: Vec<String>,
    /// Every group the configuration or its test files declare, sorted.
    pub groups: Vec<String>,
    /// Why the configuration could not be read, as a sentence.
    pub problem: Option<String>,
}

/// Read the configuration `explicit` names (default: the project's own) and
/// scan its suites' test files for groups. Blocking filesystem work: call it
/// off the GUI thread.
pub fn model(root: &Path, explicit: &str) -> PhpUnitModel {
    let Some(file) = discover(root, explicit) else {
        return PhpUnitModel {
            problem: (!explicit.trim().is_empty())
                .then(|| format!("{} was not found", explicit.trim())),
            ..PhpUnitModel::default()
        };
    };
    let relative = file
        .strip_prefix(root)
        .unwrap_or(&file)
        .to_string_lossy()
        .replace('\\', "/");
    let mut model = PhpUnitModel {
        file: Some(relative),
        ..PhpUnitModel::default()
    };
    let config = match std::fs::read_to_string(&file)
        .map_err(|error| error.to_string())
        .and_then(|text| parse(&text).map_err(|error| error.to_string()))
    {
        Ok(config) => config,
        Err(error) => {
            model.problem = Some(error);
            return model;
        }
    };
    let config_dir = file.parent().unwrap_or(root);
    let mut groups: BTreeSet<String> = config
        .include_groups
        .iter()
        .chain(&config.exclude_groups)
        .cloned()
        .collect();
    let mut budget = MAX_SCANNED_FILES;
    for suite in &config.suites {
        for path in suite_files(config_dir, suite, budget) {
            budget = budget.saturating_sub(1);
            let small = std::fs::metadata(&path).is_ok_and(|meta| meta.len() <= MAX_FILE_BYTES);
            if let Some(source) = small.then(|| std::fs::read_to_string(&path).ok()).flatten() {
                groups.extend(groups_in(&source));
            }
        }
    }
    model.suites = config.suites.into_iter().map(|suite| suite.name).collect();
    model.groups = groups.into_iter().collect();
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of a real Symfony project's `phpunit.dist.xml`.
    const SYMFONY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- https://phpunit.readthedocs.io/en/latest/configuration.html -->
<phpunit xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="vendor/phpunit/phpunit/phpunit.xsd" backupGlobals="false" colors="true" bootstrap="tests/bootstrap.php" cacheDirectory=".phpunit.cache">
  <php>
    <ini name="display_errors" value="1"/>
    <server name="APP_ENV" value="test" force="true"/>
  </php>
  <testsuites>
    <testsuite name="core-unit">
      <directory>lib/aop/tests</directory>
    </testsuite>
    <testsuite name="bundle-integration">
      <directory>lib/aop-bundle/tests</directory>
    </testsuite>
    <testsuite name="app-e2e">
      <directory>tests</directory>
    </testsuite>
  </testsuites>
  <source restrictNotices="true">
    <include>
      <directory>lib/aop/src</directory>
      <directory>src</directory>
    </include>
  </source>
</phpunit>"#;

    #[test]
    fn symfony_config_yields_suites_and_bootstrap() {
        let config = parse(SYMFONY).unwrap();
        let names: Vec<_> = config.suites.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["core-unit", "bundle-integration", "app-e2e"]);
        assert_eq!(config.bootstrap.as_deref(), Some("tests/bootstrap.php"));
        assert_eq!(config.suites[0].directories[0].path, "lib/aop/tests");
        assert_eq!(config.suites[0].directories[0].suffix, "Test.php");
        // `<source><include>` is coverage, not groups.
        assert!(config.include_groups.is_empty());
    }

    #[test]
    fn suite_directories_files_excludes_and_suffix() {
        let config = parse(
            r#"<phpunit><testsuites>
              <testsuite name="unit">
                <directory suffix="Spec.php">tests/Unit</directory>
                <file>tests/Extra.php</file>
                <exclude>tests/Unit/Slow</exclude>
              </testsuite>
              <testsuite name="">
                <directory>ignored</directory>
              </testsuite>
            </testsuites></phpunit>"#,
        )
        .unwrap();
        assert_eq!(config.suites.len(), 1);
        let suite = &config.suites[0];
        assert_eq!(suite.directories[0].suffix, "Spec.php");
        assert_eq!(suite.files, ["tests/Extra.php"]);
        assert_eq!(suite.excludes, ["tests/Unit/Slow"]);
    }

    #[test]
    fn groups_include_and_exclude_are_read() {
        let config = parse(
            r#"<phpunit><groups>
                 <include><group>fast</group></include>
                 <exclude><group>slow</group><group>flaky</group></exclude>
               </groups></phpunit>"#,
        )
        .unwrap();
        assert_eq!(config.include_groups, ["fast"]);
        assert_eq!(config.exclude_groups, ["slow", "flaky"]);
    }

    #[test]
    fn malformed_xml_is_an_error() {
        assert!(parse("<phpunit><testsuites></phpunit>").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn groups_come_from_attributes_and_docblocks() {
        let source = r#"<?php
use PHPUnit\Framework\Attributes\Group;
#[Group('api')]
#[\PHPUnit\Framework\Attributes\Group("db"), Group( 'multi' )]
final class FooTest extends TestCase {
    /**
     * @group legacy
     * @groupie nope
     */
    public function testX(): void {}
    #[DataGroup('not-a-group')]
    public function testY(): void {}
}"#;
        let groups: Vec<_> = groups_in(source).into_iter().collect();
        assert_eq!(groups, ["api", "db", "legacy", "multi"]);
    }

    fn write(root: &Path, name: &str, text: &str) {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn discovery_follows_phpunits_order_and_an_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(discover(root, ""), None);
        write(root, "phpunit.xml.dist", "<phpunit/>");
        assert_eq!(discover(root, ""), Some(root.join("phpunit.xml.dist")));
        write(root, "phpunit.dist.xml", "<phpunit/>");
        assert_eq!(discover(root, ""), Some(root.join("phpunit.dist.xml")));
        write(root, "phpunit.xml", "<phpunit/>");
        assert_eq!(discover(root, ""), Some(root.join("phpunit.xml")));
        write(root, "config/other.xml", "<phpunit/>");
        assert_eq!(
            discover(root, "config/other.xml"),
            Some(root.join("config/other.xml"))
        );
        assert_eq!(discover(root, "config"), None);
        assert_eq!(discover(root, "missing.xml"), None);
    }

    #[test]
    fn model_scans_suite_files_for_groups() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "phpunit.xml",
            r#"<phpunit><testsuites>
                 <testsuite name="unit">
                   <directory>tests/Unit</directory>
                   <exclude>tests/Unit/Skipped</exclude>
                 </testsuite>
               </testsuites>
               <groups><exclude><group>slow</group></exclude></groups></phpunit>"#,
        );
        write(
            root,
            "tests/Unit/AFooTest.php",
            "<?php #[Group('api')] class A {}",
        );
        write(
            root,
            "tests/Unit/deep/BTest.php",
            "<?php /** @group db */ class B {}",
        );
        write(
            root,
            "tests/Unit/Skipped/CTest.php",
            "<?php #[Group('hidden')] class C {}",
        );
        write(
            root,
            "tests/Unit/Helper.php",
            "<?php #[Group('helper')] class H {}",
        );
        write(
            root,
            "tests/Other/DTest.php",
            "<?php #[Group('other')] class D {}",
        );
        let model = model(root, "");
        assert_eq!(model.file.as_deref(), Some("phpunit.xml"));
        assert_eq!(model.suites, ["unit"]);
        assert_eq!(model.groups, ["api", "db", "slow"]);
        assert_eq!(model.problem, None);
    }

    #[test]
    fn model_reports_a_missing_explicit_file_and_a_broken_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(model(root, ""), PhpUnitModel::default());
        assert_eq!(
            model(root, "nope.xml").problem.as_deref(),
            Some("nope.xml was not found")
        );
        write(root, "phpunit.xml", "<phpunit><testsuites></phpunit>");
        let broken = model(root, "");
        assert_eq!(broken.file.as_deref(), Some("phpunit.xml"));
        assert!(broken.problem.unwrap().starts_with("malformed XML"));
    }
}
