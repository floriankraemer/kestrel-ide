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

use app_config::php::PHPUNIT_CONFIG_CANDIDATES;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// PHPUnit's default test file suffix for a `<directory>` without `suffix`.
const DEFAULT_SUFFIX: &str = "Test.php";
/// The scan stops after this many test files, so a suite that points at
/// `vendor` cannot stall the editor.
pub(crate) const MAX_SCANNED_FILES: usize = 2000;
/// A test file larger than this is not read.
pub(crate) const MAX_FILE_BYTES: u64 = 512 * 1024;

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

/// The text an entity or character reference stands for. An entity a
/// configuration cannot declare (no DTD) stays as written.
fn resolve_reference(reference: &quick_xml::events::BytesRef<'_>) -> Result<String, ParseError> {
    if let Some(c) = reference
        .resolve_char_ref()
        .map_err(|e| ParseError(e.to_string()))?
    {
        return Ok(c.to_string());
    }
    let name = reference.decode().map_err(|e| ParseError(e.to_string()))?;
    Ok(quick_xml::escape::resolve_predefined_entity(&name)
        .map_or_else(|| format!("&{name};"), str::to_string))
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
            // quick-xml reports `&amp;` and `&#38;` between text events, and
            // CDATA as its own event: both are part of the element's text.
            Ok(Event::GeneralRef(reference)) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&resolve_reference(&reference)?);
                }
            }
            Ok(Event::CData(data)) => {
                if let Some(top) = stack.last_mut() {
                    let data = data.decode().map_err(|e| ParseError(e.to_string()))?;
                    top.text.push_str(&data);
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

/// The quoted string at the start of `text`, on one line; `None` when the
/// quote is not closed there.
fn quoted(text: &str) -> Option<&str> {
    let quote = text.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let body = &text[1..];
    let end = body.find([quote, '\n'])?;
    (body[end..].starts_with(quote) && end > 0).then(|| &body[..end])
}

/// The quoted first argument of an attribute call, `after_paren` starting
/// just after its `(`, with or without the `name:` label.
fn attribute_argument(after_paren: &str) -> Option<&str> {
    let rest = after_paren.trim_start();
    let rest = rest
        .strip_prefix("name")
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix(':'))
        .map_or(rest, str::trim_start);
    quoted(rest)
}

/// The groups PHPUnit derives from attributes: `#[Group('x')]` and
/// `#[Ticket('x')]` name one, `#[Small]`/`#[Medium]`/`#[Large]` are the
/// size groups (any namespace qualification).
fn attribute_groups(source: &str, groups: &mut BTreeSet<String>) {
    let bytes = source.as_bytes();
    let is_name_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let starts_word = |at: usize| at == 0 || !is_name_byte(bytes[at - 1]);
    for name in ["Group(", "Ticket("] {
        for (at, _) in source.match_indices(name) {
            // `Group(`, not `DataGroup(`; a namespace separator before it is fine.
            if !starts_word(at) {
                continue;
            }
            if let Some(group) = attribute_argument(&source[at + name.len()..]) {
                groups.insert(group.to_string());
            }
        }
    }
    for size in ["Small", "Medium", "Large"] {
        let found = source.match_indices(size).any(|(at, _)| {
            let after = bytes.get(at + size.len()).copied();
            starts_word(at)
                && matches!(after, Some(b']' | b',' | b'(' | b' '))
                && source[..at]
                    .trim_end_matches(|c: char| c == '\\' || c == '_' || c.is_ascii_alphanumeric())
                    .trim_end()
                    .ends_with(['[', ','])
        });
        if found {
            groups.insert(size.to_lowercase());
        }
    }
}

/// The groups docblock annotations name: `@group x`, `@ticket x`,
/// `@author x` and the `@small`/`@medium`/`@large` sizes.
fn annotation_groups(source: &str, groups: &mut BTreeSet<String>) {
    for (at, _) in source.match_indices('@') {
        let rest = &source[at + 1..];
        let tag_len = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let (tag, rest) = rest.split_at(tag_len);
        match tag {
            "group" | "ticket" | "author" if rest.starts_with([' ', '\t']) => {
                let name = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .trim_end_matches("*/");
                if !name.is_empty() {
                    groups.insert(name.to_string());
                }
            }
            "small" | "medium" | "large"
                if rest.is_empty()
                    || rest.starts_with(char::is_whitespace)
                    || rest.starts_with('*') =>
            {
                groups.insert(tag.to_string());
            }
            _ => {}
        }
    }
}

/// Every group name a test file declares. `annotations` is whether the
/// project's PHPUnit still reads docblock annotations (it stopped in 12).
pub fn groups_in(source: &str, annotations: bool) -> BTreeSet<String> {
    let mut groups = BTreeSet::new();
    attribute_groups(source, &mut groups);
    if annotations {
        annotation_groups(source, &mut groups);
    }
    groups
}

/// Whether the project's PHPUnit reads docblock annotations: below 12, or
/// when `composer.lock` does not say which PHPUnit is installed.
pub fn reads_annotations(lock: Option<&crate::composer::ComposerLock>) -> bool {
    let major = lock
        .and_then(|lock| lock.version_of("phpunit/phpunit"))
        .and_then(|version| {
            version
                .trim_start_matches('v')
                .split('.')
                .next()?
                .parse::<u32>()
                .ok()
        });
    major.is_none_or(|major| major < 12)
}

/// The scan's shared limits: test files read, directory entries visited.
pub(crate) struct Budget {
    pub(crate) files: usize,
    pub(crate) entries: usize,
}

impl Budget {
    pub(crate) fn spent(&self) -> bool {
        self.files == 0 || self.entries == 0
    }
}

/// Directories a suite's walk does not enter below the directory the suite
/// names: dependencies, never the project's own tests.
const SKIPPED_DIRECTORIES: &[&str] = &["vendor", "node_modules", ".git"];

pub(crate) fn is_skipped(dir: &Path) -> bool {
    dir.file_name()
        .is_some_and(|name| SKIPPED_DIRECTORIES.iter().any(|skipped| name == *skipped))
}

fn has_glob(path: &str) -> bool {
    path.contains(['*', '?', '['])
}

/// `path` with `.` components dropped, so `./tests` and `tests` compare equal.
fn clean(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

/// A directory's entries, sorted, each with whether it is a directory and
/// whether it is a file. A symlinked directory is neither, so a link cycle
/// is never walked; a symlinked file still counts. Uses the entry's own
/// type, which `read_dir` already has, rather than a stat per entry.
pub(crate) fn entries_of(dir: &Path, budget: &mut Budget) -> Vec<(PathBuf, bool, bool)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read
        .flatten()
        .take(budget.entries)
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            let is_file = kind.is_file() || (kind.is_symlink() && entry.path().is_file());
            Some((entry.path(), kind.is_dir(), is_file))
        })
        .collect();
    budget.entries = budget.entries.saturating_sub(entries.len());
    entries.sort();
    entries
}

/// The directories (`want_dirs`) or files `pattern` matches under `base`.
/// `*` and `?` stay within one path segment, `**` spans any number.
fn expand_glob(base: &Path, pattern: &str, want_dirs: bool, budget: &mut Budget) -> Vec<PathBuf> {
    let pattern = pattern.replace('\\', "/");
    let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
    let Ok(glob) = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
    else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    // Walk from the last directory before the first wildcard segment.
    let literal: Vec<&str> = pattern
        .split('/')
        .take_while(|segment| !has_glob(segment))
        .collect();
    let deepest = (!pattern.contains("**")).then(|| pattern.split('/').count());
    let mut found = Vec::new();
    let mut pending =
        std::collections::VecDeque::from([(base.join(literal.join("/")), literal.len())]);
    while let Some((dir, depth)) = pending.pop_front() {
        if budget.spent() || deepest.is_some_and(|deepest| depth >= deepest) {
            continue;
        }
        for (path, is_dir, is_file) in entries_of(&dir, budget) {
            let relative = path.strip_prefix(base).unwrap_or(&path);
            if (if want_dirs { is_dir } else { is_file }) && matcher.is_match(relative) {
                found.push(path.clone());
            }
            if is_dir && !is_skipped(&path) {
                pending.push_back((path, depth + 1));
            }
        }
    }
    found
}

/// The test files of `suite` under `config_dir`, added to `found` until the
/// budget runs out. Walks breadth first in name order, so a suite rooted at
/// `.` reaches `tests/` long before a deep dependency tree.
fn suite_files(
    config_dir: &Path,
    suite: &TestSuite,
    budget: &mut Budget,
    found: &mut BTreeSet<PathBuf>,
) {
    let excluded: Vec<PathBuf> = suite
        .excludes
        .iter()
        .map(|exclude| clean(&config_dir.join(exclude)))
        .collect();
    let is_excluded = |path: &Path| excluded.iter().any(|ex| path.starts_with(ex));
    let add = |path: PathBuf, found: &mut BTreeSet<PathBuf>, budget: &mut Budget| {
        let path = clean(&path);
        if budget.files > 0 && !is_excluded(&path) && found.insert(path) {
            budget.files -= 1;
        }
    };
    for file in &suite.files {
        if has_glob(file) {
            for path in expand_glob(config_dir, file, false, budget) {
                add(path, found, budget);
            }
        } else if config_dir.join(file).is_file() {
            add(config_dir.join(file), found, budget);
        }
    }
    for directory in &suite.directories {
        let roots = if has_glob(&directory.path) {
            expand_glob(config_dir, &directory.path, true, budget)
        } else {
            vec![config_dir.join(&directory.path)]
        };
        let mut pending: std::collections::VecDeque<PathBuf> =
            roots.into_iter().map(|r| clean(&r)).collect();
        while let Some(dir) = pending.pop_front() {
            if budget.spent() || is_excluded(&dir) {
                continue;
            }
            for (path, is_dir, is_file) in entries_of(&dir, budget) {
                if is_dir && !is_skipped(&path) {
                    pending.push_back(path);
                } else if is_file && path.to_string_lossy().ends_with(&directory.suffix) {
                    add(path, found, budget);
                }
            }
        }
    }
}

/// Why a configuration file could not be offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelProblem {
    /// The configuration field names a file that does not exist.
    NotFound(String),
    /// The file exists but cannot be read.
    Unreadable(String),
    Malformed(ParseError),
}

impl std::fmt::Display for ModelProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "{name} was not found"),
            Self::Unreadable(reason) => write!(f, "cannot be read: {reason}"),
            Self::Malformed(error) => error.fmt(f),
        }
    }
}

/// What the run configuration editor offers for a project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhpUnitModel {
    /// The configuration file used, relative to the project root (`/`).
    pub file: Option<String>,
    pub suites: Vec<String>,
    /// Every group the configuration or its test files declare, sorted.
    pub groups: Vec<String>,
    /// Why the configuration could not be read.
    pub problem: Option<ModelProblem>,
}

/// The scan reads at most this many directory entries in all, so a suite
/// rooted at a huge tree cannot stall the editor even when few files match.
pub(crate) const MAX_VISITED_ENTRIES: usize = 20_000;

/// Read the configuration `explicit` names (default: the project's own) and
/// scan its suites' test files for groups. Blocking filesystem work: call it
/// off the GUI thread.
pub fn model(root: &Path, explicit: &str) -> PhpUnitModel {
    model_with(root, explicit, MAX_SCANNED_FILES, MAX_VISITED_ENTRIES)
}

/// The configuration PHPUnit uses for a run in the project-relative
/// `work_dir` (#424): `explicit` when set, else the one PHPUnit finds in the
/// working directory. `None` when there is none.
pub fn discover_in(root: &Path, work_dir: &str, explicit: &str) -> Option<PathBuf> {
    match explicit.trim() {
        "" => discover(&root.join(work_dir.trim()), ""),
        explicit => discover(root, explicit),
    }
}

/// [`model`] for a run in the project-relative `work_dir`: without an
/// explicit file, the working directory's configuration, and none (no
/// problem either) when it has none, since PHPUnit then runs without one.
pub fn model_in(root: &Path, work_dir: &str, explicit: &str) -> PhpUnitModel {
    if !explicit.trim().is_empty() || work_dir.trim().is_empty() {
        return model(root, explicit);
    }
    match discover_in(root, work_dir, "") {
        Some(file) => model(root, &file.to_string_lossy()),
        None => PhpUnitModel::default(),
    }
}

fn model_with(root: &Path, explicit: &str, max_files: usize, max_entries: usize) -> PhpUnitModel {
    let Some(file) = discover(root, explicit) else {
        return PhpUnitModel {
            problem: (!explicit.trim().is_empty())
                .then(|| ModelProblem::NotFound(explicit.trim().to_string())),
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
        .map_err(|error| ModelProblem::Unreadable(error.to_string()))
        .and_then(|text| parse(&text).map_err(ModelProblem::Malformed))
    {
        Ok(config) => config,
        Err(problem) => {
            model.problem = Some(problem);
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
    let mut budget = Budget {
        files: max_files,
        entries: max_entries,
    };
    // Overlapping suites name the same file once.
    let mut files = BTreeSet::new();
    for suite in &config.suites {
        suite_files(config_dir, suite, &mut budget, &mut files);
    }
    let lock = crate::composer::ComposerLock::read(root).ok().flatten();
    let annotations = reads_annotations(lock.as_ref());
    for path in files {
        let small = std::fs::metadata(&path).is_ok_and(|meta| meta.len() <= MAX_FILE_BYTES);
        if let Some(source) = small.then(|| std::fs::read_to_string(&path).ok()).flatten() {
            groups.extend(groups_in(&source, annotations));
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
        let groups: Vec<_> = groups_in(source, true).into_iter().collect();
        assert_eq!(groups, ["api", "db", "legacy", "multi"]);
        // PHPUnit 12 reads attributes only.
        let groups: Vec<_> = groups_in(source, false).into_iter().collect();
        assert_eq!(groups, ["api", "db", "multi"]);
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
        let missing = model(root, "nope.xml").problem.unwrap();
        assert_eq!(missing, ModelProblem::NotFound("nope.xml".into()));
        assert_eq!(missing.to_string(), "nope.xml was not found");
        write(root, "phpunit.xml", "<phpunit><testsuites></phpunit>");
        let broken = model(root, "");
        assert_eq!(broken.file.as_deref(), Some("phpunit.xml"));
        let problem = broken.problem.unwrap();
        assert!(matches!(problem, ModelProblem::Malformed(_)));
        assert!(problem.to_string().starts_with("malformed XML"));
    }

    #[test]
    fn entities_and_cdata_are_part_of_a_path() {
        let config = parse(
            r#"<phpunit><testsuites><testsuite name="a&amp;b">
                 <directory>tests/A&amp;B&#47;x</directory>
                 <file><![CDATA[tests/<odd>.php]]></file>
               </testsuite></testsuites></phpunit>"#,
        )
        .unwrap();
        assert_eq!(config.suites[0].name, "a&b");
        assert_eq!(config.suites[0].directories[0].path, "tests/A&B/x");
        assert_eq!(config.suites[0].files, ["tests/<odd>.php"]);
    }

    #[test]
    fn implicit_and_named_groups_are_offered() {
        let source = r#"<?php
#[Group(name: 'named'), Ticket('T-1')]
#[\PHPUnit\Framework\Attributes\Small]
final class FooTest extends TestCase {
    /** @group db*/
    public function testA(): void {}
    /**
     * @ticket 42
     * @author jane
     * @large
     */
    public function testB(): void {}
    #[Group('unterminated
    public function testC(): void {}
}"#;
        let groups: Vec<_> = groups_in(source, true).into_iter().collect();
        assert_eq!(
            groups,
            ["42", "T-1", "db", "jane", "large", "named", "small"]
        );
        // `SmallTest` or a `$small` variable is no size attribute.
        assert!(groups_in("<?php class SmallTest { #[Group('x')] }", true)
            .iter()
            .all(|g| g != "small"));
    }

    #[test]
    fn annotations_count_below_phpunit_12_or_when_unknown() {
        let lock = |version: &str| {
            crate::composer::ComposerLock::parse(&format!(
                r#"{{"packages-dev":[{{"name":"phpunit/phpunit","version":"{version}"}}]}}"#
            ))
            .unwrap()
        };
        assert!(reads_annotations(None));
        assert!(reads_annotations(Some(&lock("9.6.1"))));
        assert!(reads_annotations(Some(&lock("v11.5.0"))));
        assert!(!reads_annotations(Some(&lock("12.0.9"))));
    }

    fn suite_config(body: &str) -> String {
        format!(r#"<phpunit><testsuites>{body}</testsuites></phpunit>"#)
    }

    #[test]
    fn globbed_suite_directories_and_files_are_expanded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "phpunit.xml",
            &suite_config(
                r#"<testsuite name="unit">
                     <directory>tests/*/Unit</directory>
                     <directory>modules/**/Spec</directory>
                     <file>extra/*Check.php</file>
                   </testsuite>"#,
            ),
        );
        write(root, "tests/A/Unit/ATest.php", "<?php #[Group('a')]");
        write(root, "tests/B/Unit/deep/BTest.php", "<?php #[Group('b')]");
        write(root, "tests/C/Other/CTest.php", "<?php #[Group('no-c')]");
        write(root, "tests/A/X/Unit/DTest.php", "<?php #[Group('no-d')]");
        write(root, "modules/m/n/Spec/ETest.php", "<?php #[Group('e')]");
        write(root, "extra/OneCheck.php", "<?php #[Group('f')]");
        assert_eq!(model(root, "").groups, ["a", "b", "e", "f"]);
    }

    #[test]
    fn files_dot_prefixes_overlaps_and_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "phpunit.xml",
            &suite_config(
                r#"<testsuite name="all">
                     <directory>./</directory>
                     <exclude>./tests/Slow</exclude>
                   </testsuite>
                   <testsuite name="again"><directory>tests</directory></testsuite>
                   <testsuite name="one"><file>./lone/Only.php</file></testsuite>
                   <testsuite name="deps"><directory>vendor/acme</directory></testsuite>"#,
            ),
        );
        write(root, "tests/ATest.php", "<?php #[Group('a')]");
        write(root, "tests/Slow/STest.php", "<?php #[Group('slow-only')]");
        write(root, "lone/Only.php", "<?php #[Group('only')]");
        write(root, "vendor/other/XTest.php", "<?php #[Group('vendor')]");
        write(
            root,
            "vendor/acme/YTest.php",
            "<?php #[Group('named-vendor')]",
        );
        write(root, "node_modules/p/ZTest.php", "<?php #[Group('npm')]");
        // `again` reaches tests/Slow, which only `all` excludes.
        assert_eq!(
            model(root, "").groups,
            ["a", "named-vendor", "only", "slow-only"]
        );
    }

    #[test]
    fn the_scan_shares_one_budget_and_skips_big_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "phpunit.xml",
            &suite_config(
                r#"<testsuite name="a"><directory>a</directory></testsuite>
                   <testsuite name="b"><directory>b</directory></testsuite>"#,
            ),
        );
        write(root, "a/OneTest.php", "<?php #[Group('one')]");
        write(root, "a/TwoTest.php", "<?php #[Group('two')]");
        write(root, "b/ThreeTest.php", "<?php #[Group('three')]");
        let big = format!(
            "<?php #[Group('big')] /*{}*/",
            "x".repeat(MAX_FILE_BYTES as usize)
        );
        write(root, "a/BigTest.php", &big);
        assert_eq!(model(root, "").groups, ["one", "three", "two"]);
        // Three files in all, across both suites: suite `b` gets none.
        assert_eq!(model_with(root, "", 3, 1000).groups, ["one", "two"]);
        // The entry cap stops a walk even when nothing matches.
        assert!(model_with(root, "", 100, 1).groups.len() <= 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_working_directory_reads_its_own_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("phpunit.xml"), SYMFONY).unwrap();
        let package = root.join("packages/mailer");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("phpunit.dist.xml"),
            r#"<phpunit><testsuites><testsuite name="mailer"><directory>tests</directory></testsuite></testsuites></phpunit>"#,
        )
        .unwrap();

        let package_model = model_in(root, "packages/mailer", "");
        assert_eq!(
            package_model.file.as_deref(),
            Some("packages/mailer/phpunit.dist.xml")
        );
        assert_eq!(package_model.suites, ["mailer"]);
        assert_eq!(
            discover_in(root, "packages/mailer", "phpunit.xml"),
            Some(root.join("phpunit.xml"))
        );
        assert_eq!(model_in(root, "", ""), model(root, ""));

        // A package without one runs without one: nothing to offer, no problem.
        std::fs::create_dir_all(root.join("packages/util")).unwrap();
        assert_eq!(model_in(root, "packages/util", ""), PhpUnitModel::default());
        assert_eq!(discover_in(root, "packages/util", ""), None);
    }

    #[test]
    fn a_symlink_cycle_is_not_walked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "phpunit.xml",
            &suite_config(r#"<testsuite name="t"><directory>tests</directory></testsuite>"#),
        );
        write(root, "tests/ATest.php", "<?php #[Group('a')]");
        std::os::unix::fs::symlink(root.join("tests"), root.join("tests/loop")).unwrap();
        std::os::unix::fs::symlink(
            root.join("tests/ATest.php"),
            root.join("tests/LinkTest.php"),
        )
        .unwrap();
        let model = model(root, "");
        assert_eq!(model.groups, ["a"]);
    }
}
