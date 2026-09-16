//! Caret-context classification (D2): given `(path, text, caret)`, tells
//! D5's completion and D6/D7's version-hint machinery which dependency
//! coordinate the caret sits inside, and what is already typed around it.
//!
//! Three formats, three techniques — deliberately not a shared parser:
//!
//! * `pom.xml`: a quick-xml tag-stack scan tracking byte offsets, since a
//!   dependency's `<groupId>`/`<artifactId>`/`<version>` are separate
//!   elements with their own text-content ranges.
//! * `build.gradle(.kts)`: regex over the whole file for a dependency
//!   configuration call's string-literal argument (Groovy and Kotlin DSL
//!   share this shape closely enough that one pattern per quote style
//!   covers both) and for `plugins { id("…") version "…" }`.
//! * `libs.versions.toml`: line scanning with a running byte offset,
//!   tracking the current `[section]` the same way a hand-rolled TOML
//!   reader would for just these two tables.
//!
//! No tree-sitter dependency in this crate, per the plan: none of these
//! three needs a real parse tree, and pulling tree-sitter in here would
//! duplicate `syntax-core`'s job for three formats it already tokenizes
//! for other reasons (D1's Groovy row, the existing XML/TOML rows).

use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use quick_xml::events::Event;
use quick_xml::Reader;
use regex::Regex;

/// Which part of a `group:artifact:version` coordinate the caret sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatePart {
    GroupId,
    ArtifactId,
    Version,
}

/// The coordinate values already typed around the part being edited, so a
/// completion for one field can be filtered by the others (an `artifactId`
/// completion scoped to the `groupId` already on the line, say).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coordinate {
    pub group_id: Option<String>,
    pub artifact_id: Option<String>,
    pub version: Option<String>,
}

/// One field of a `libs.versions.toml` `[libraries]` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TomlLibraryField {
    Module,
    Group,
    Name,
    VersionRef,
}

/// The values already typed on a `[libraries]` entry's line, mirroring
/// [`Coordinate`] for the version-catalog shape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TomlLibraryEntry {
    pub module: Option<String>,
    pub group: Option<String>,
    pub name: Option<String>,
    pub version_ref: Option<String>,
}

/// What the caret is sitting inside, and enough of its surroundings to
/// drive completion (D5), a "newer version" hint (D6) or a quick fix (D7).
///
/// Every variant's `range` is the exact span an accepted completion or fix
/// edit replaces — the one coordinate part's value, never the whole
/// literal it lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditContext {
    /// pom.xml: caret is inside the text content of a `<groupId>`/
    /// `<artifactId>`/`<version>` element whose immediate parent is
    /// `<dependency>`, `<plugin>` or `<parent>`.
    PomCoordinate {
        part: CoordinatePart,
        range: Range<usize>,
        coordinate: Coordinate,
    },
    /// build.gradle(.kts): caret inside one segment of a
    /// `"group:artifact:version"` string literal that is a dependency
    /// configuration call's argument.
    GradleCoordinate {
        part: CoordinatePart,
        range: Range<usize>,
        coordinate: Coordinate,
        configuration: String,
    },
    /// build.gradle(.kts): caret inside the version string literal of
    /// `plugins { id("…") version "…" }`.
    GradlePluginVersion {
        range: Range<usize>,
        plugin_id: Option<String>,
    },
    /// gradle/libs.versions.toml: caret inside a `[versions]` table's
    /// string value.
    TomlVersion { range: Range<usize>, key: String },
    /// gradle/libs.versions.toml: caret inside one field of a
    /// `[libraries]` entry's inline table.
    TomlLibraryField {
        field: TomlLibraryField,
        range: Range<usize>,
        entry: TomlLibraryEntry,
    },
}

/// Which build-file format `path` names, so `context` never has to repeat
/// the three formats' filename rules.
enum FileKind {
    Pom,
    Gradle,
    VersionCatalog,
}

fn classify(path: &Path) -> Option<FileKind> {
    let name = path.file_name()?.to_str()?;
    if name == "pom.xml" {
        return Some(FileKind::Pom);
    }
    if name == "libs.versions.toml" {
        return Some(FileKind::VersionCatalog);
    }
    if name.ends_with(".gradle") || name.ends_with(".gradle.kts") {
        return Some(FileKind::Gradle);
    }
    None
}

/// Is `path` a build file this module can classify at all — `pom.xml`,
/// `build.gradle(.kts)` (any module's, anywhere in the tree), or
/// `libs.versions.toml`? A basename-only rule (review fix #7),
/// deliberately independent of a plugin's `BuildToolContribution::
/// build_files` globs: those are root-relative, meant for "does this
/// project have a Gradle/Maven root at all" detection
/// (`jvm_build_core::sync::is_build_file`), and a literal `"build.gradle"`
/// pattern with no `**/` prefix never matches a module's own
/// `app/build.gradle` — which left a multi-module project's non-root
/// build files out of `open_docs` (D0) entirely, so D7's quick fix wrote
/// straight to disk under a dirty tab instead of splicing into the open
/// buffer. This is the one rule every reader of "is this a build file"
/// (`context`, D0's registration, D5/D7's own file-kind checks) now
/// shares.
pub fn is_build_file(path: &Path) -> bool {
    classify(path).is_some()
}

/// `caret` is inside `range`, inclusive of both ends — a completion or fix
/// must trigger while the caret sits right after the last typed character
/// too, which a half-open `Range::contains` would miss.
fn caret_in(range: &Range<usize>, caret: usize) -> bool {
    range.start <= caret && caret <= range.end
}

/// Classify the caret position in an open build file. `path` only decides
/// *which* of the three formats below applies; `text` is the buffer's
/// current content (not necessarily what is on disk), `caret` a byte
/// offset into it.
pub fn context(path: &Path, text: &str, caret: usize) -> Option<EditContext> {
    match classify(path)? {
        FileKind::Pom => pom_context(text, caret),
        FileKind::Gradle => gradle_context(text, caret),
        FileKind::VersionCatalog => toml_context(text, caret),
    }
}

// ---------------------------------------------------------------------
// pom.xml
// ---------------------------------------------------------------------

const POM_CONTAINERS: [&str; 3] = ["dependency", "plugin", "parent"];

#[derive(Default)]
struct PomBlock {
    group_id: Option<(String, Range<usize>)>,
    artifact_id: Option<(String, Range<usize>)>,
    version: Option<(String, Range<usize>)>,
}

fn local_name(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    s.rsplit(':').next().unwrap_or(&s).to_string()
}

/// Every `<dependency>`/`<plugin>`/`<parent>` block in `text`, each with
/// whichever of its `<groupId>`/`<artifactId>`/`<version>` children it
/// has and their own text-content ranges. Shared by [`pom_context`] (which
/// searches these for the one the caret is in) and D6's whole-document
/// version scan (which wants all of them).
fn pom_blocks(text: &str) -> Vec<PomBlock> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut element_stack: Vec<String> = Vec::new();
    let mut container_stack: Vec<PomBlock> = Vec::new();
    let mut blocks: Vec<PomBlock> = Vec::new();

    while let Ok(event) = reader.read_event_into(&mut buf) {
        let pos_after = reader.buffer_position() as usize;
        match event {
            Event::Eof => break,
            Event::Start(tag) => {
                let name = local_name(tag.name().as_ref());
                if POM_CONTAINERS.contains(&name.as_str()) {
                    container_stack.push(PomBlock::default());
                }
                element_stack.push(name);
            }
            Event::Text(text_event) => {
                let raw_len = text_event.len();
                let start = pos_after.saturating_sub(raw_len);
                let end = pos_after;
                // Only a *direct* child of the container currently on top
                // of the stack may set its coordinate — an `<exclusion>`'s
                // own `<groupId>`/`<artifactId>` (nested inside
                // `<dependency><exclusions><exclusion>`), or a plugin's
                // `<configuration><artifactItems><artifactItem>`'s, sit
                // several levels deeper and must never overwrite the
                // enclosing `<dependency>`/`<plugin>`'s real coordinate.
                let direct_child_of_container = element_stack
                    .len()
                    .checked_sub(2)
                    .and_then(|i| element_stack.get(i))
                    .is_some_and(|grandparent| POM_CONTAINERS.contains(&grandparent.as_str()));
                if direct_child_of_container {
                    if let Some(element_name) = element_stack.last() {
                        if let Some(block) = container_stack.last_mut() {
                            let value = text_event
                                .decode()
                                .ok()
                                .and_then(|raw| {
                                    quick_xml::escape::unescape(&raw)
                                        .ok()
                                        .map(|s| s.into_owned())
                                })
                                .map(|s| s.trim().to_string())
                                .unwrap_or_default();
                            match element_name.as_str() {
                                "groupId" => block.group_id = Some((value, start..end)),
                                "artifactId" => block.artifact_id = Some((value, start..end)),
                                "version" => block.version = Some((value, start..end)),
                                _ => {}
                            }
                        }
                    }
                }
            }
            Event::End(tag) => {
                let name = local_name(tag.name().as_ref());
                element_stack.pop();
                if POM_CONTAINERS.contains(&name.as_str()) {
                    if let Some(block) = container_stack.pop() {
                        blocks.push(block);
                    }
                }
            }
            _ => {}
        }
    }
    // A malformed document (unclosed tags) leaves containers still open —
    // still worth searching, since the caret is very likely inside the
    // element the user has not finished typing yet.
    blocks.extend(container_stack);
    blocks
}

fn pom_context(text: &str, caret: usize) -> Option<EditContext> {
    let blocks = pom_blocks(text);
    for block in &blocks {
        let coordinate = Coordinate {
            group_id: block.group_id.as_ref().map(|(v, _)| v.clone()),
            artifact_id: block.artifact_id.as_ref().map(|(v, _)| v.clone()),
            version: block.version.as_ref().map(|(v, _)| v.clone()),
        };
        for (part, field) in [
            (CoordinatePart::GroupId, &block.group_id),
            (CoordinatePart::ArtifactId, &block.artifact_id),
            (CoordinatePart::Version, &block.version),
        ] {
            if let Some((_, range)) = field {
                if caret_in(range, caret) {
                    return Some(EditContext::PomCoordinate {
                        part,
                        range: range.clone(),
                        coordinate: coordinate.clone(),
                    });
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------
// build.gradle(.kts)
// ---------------------------------------------------------------------

/// `plan_changes`'s own list, mirrored here rather than shared: the plan
/// names it explicitly and it is Gradle-vocabulary, not something
/// `jvm-build-core::model` already carries as data.
const GRADLE_CONFIGURATIONS: &str = "implementation|testImplementation|api|compileOnly|runtimeOnly|annotationProcessor|kapt|classpath";

static GRADLE_COORDINATE_DQ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\b(?P<config>{GRADLE_CONFIGURATIONS})\b\s*\(?\s*"(?P<coord>[^"]*)""#
    ))
    .expect("static regex")
});
static GRADLE_COORDINATE_SQ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\b(?P<config>{GRADLE_CONFIGURATIONS})\b\s*\(?\s*'(?P<coord>[^']*)'"#
    ))
    .expect("static regex")
});
static GRADLE_PLUGIN_VERSION_DQ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bid\(\s*"(?P<id>[^"]*)"\s*\)\s*version\s*"(?P<version>[^"]*)""#)
        .expect("static regex")
});
static GRADLE_PLUGIN_VERSION_SQ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bid\(\s*'(?P<id>[^']*)'\s*\)\s*version\s*'(?P<version>[^']*)'"#)
        .expect("static regex")
});

fn gradle_context(text: &str, caret: usize) -> Option<EditContext> {
    gradle_plugin_version_context(text, caret).or_else(|| gradle_coordinate_context(text, caret))
}

fn gradle_plugin_version_context(text: &str, caret: usize) -> Option<EditContext> {
    for re in [&*GRADLE_PLUGIN_VERSION_DQ, &*GRADLE_PLUGIN_VERSION_SQ] {
        for caps in re.captures_iter(text) {
            let version = caps.name("version").expect("named group");
            let range = version.start()..version.end();
            if caret_in(&range, caret) {
                let plugin_id = caps.name("id").map(|m| m.as_str().to_string());
                return Some(EditContext::GradlePluginVersion { range, plugin_id });
            }
        }
    }
    None
}

fn gradle_coordinate_context(text: &str, caret: usize) -> Option<EditContext> {
    for re in [&*GRADLE_COORDINATE_DQ, &*GRADLE_COORDINATE_SQ] {
        for caps in re.captures_iter(text) {
            let coord = caps.name("coord").expect("named group");
            let whole_range = coord.start()..coord.end();
            if !caret_in(&whole_range, caret) {
                continue;
            }
            let configuration = caps
                .name("config")
                .expect("named group")
                .as_str()
                .to_string();
            let (part, range, coordinate) =
                split_coordinate(coord.as_str(), caret - coord.start(), coord.start());
            return Some(EditContext::GradleCoordinate {
                part,
                range,
                coordinate,
                configuration,
            });
        }
    }
    None
}

/// Splits a `"group:artifact:version"` literal into its up-to-three
/// colon-separated segments, working out which one `caret_in_coord`
/// (relative to the literal's own start) falls inside. A fourth segment
/// (a classifier, `"g:a:v:sources"`) is treated as part of `Version` —
/// this crate has no notion of classifiers to give it its own part.
fn split_coordinate(
    coord_text: &str,
    caret_in_coord: usize,
    coord_start: usize,
) -> (CoordinatePart, Range<usize>, Coordinate) {
    let mut segments: Vec<Range<usize>> = Vec::new();
    let mut seg_start = 0usize;
    for (i, ch) in coord_text.char_indices() {
        if ch == ':' {
            segments.push(seg_start..i);
            seg_start = i + ch.len_utf8();
        }
    }
    segments.push(seg_start..coord_text.len());

    let mut coordinate = Coordinate::default();
    for (i, seg) in segments.iter().enumerate() {
        let value = coord_text[seg.clone()].to_string();
        match i {
            0 => coordinate.group_id = Some(value),
            1 => coordinate.artifact_id = Some(value),
            _ => coordinate.version = Some(value),
        }
    }

    let mut part = CoordinatePart::Version;
    let mut range = segments.last().cloned().unwrap_or(0..coord_text.len());
    for (i, seg) in segments.iter().enumerate() {
        if caret_in_coord >= seg.start && caret_in_coord <= seg.end {
            part = match i {
                0 => CoordinatePart::GroupId,
                1 => CoordinatePart::ArtifactId,
                _ => CoordinatePart::Version,
            };
            range = seg.clone();
            break;
        }
    }
    let absolute_range = (coord_start + range.start)..(coord_start + range.end);
    (part, absolute_range, coordinate)
}

/// Every `"group:artifact:version"` dependency-configuration-call literal
/// in `text`, with the coordinate it parses to and the version segment's
/// own absolute range — D6's whole-document version scan (D2's
/// [`gradle_coordinate_context`] answers "what is the caret in", this
/// answers "what is declared, everywhere").
fn all_gradle_coordinates(text: &str) -> Vec<(Range<usize>, Coordinate)> {
    let mut found = Vec::new();
    for re in [&*GRADLE_COORDINATE_DQ, &*GRADLE_COORDINATE_SQ] {
        for caps in re.captures_iter(text) {
            let coord = caps.name("coord").expect("named group");
            // Caret at the coordinate's own end reliably lands the split
            // on its last segment (`Version`), which is all this caller
            // wants — there is no real caret here to make ambiguous.
            let (_, range, coordinate) =
                split_coordinate(coord.as_str(), coord.len(), coord.start());
            found.push((range, coordinate));
        }
    }
    found
}

/// Every `plugins { id("…") version "…" }` literal in `text`, with the
/// version's own range and the plugin id, if named.
fn all_gradle_plugin_versions(text: &str) -> Vec<(Range<usize>, Option<String>)> {
    let mut found = Vec::new();
    for re in [&*GRADLE_PLUGIN_VERSION_DQ, &*GRADLE_PLUGIN_VERSION_SQ] {
        for caps in re.captures_iter(text) {
            let version = caps.name("version").expect("named group");
            let range = version.start()..version.end();
            let plugin_id = caps.name("id").map(|m| m.as_str().to_string());
            found.push((range, plugin_id));
        }
    }
    found
}

// ---------------------------------------------------------------------
// gradle/libs.versions.toml
// ---------------------------------------------------------------------

static TOML_SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[([A-Za-z0-9_.-]+)\]\s*$").expect("static regex"));
static TOML_VERSION_ENTRY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*[A-Za-z0-9_.-]+\s*=\s*"([^"]*)""#).expect("static regex"));
static TOML_FIELD_MODULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bmodule\s*=\s*"([^"]*)""#).expect("static regex"));
static TOML_FIELD_GROUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bgroup\s*=\s*"([^"]*)""#).expect("static regex"));
static TOML_FIELD_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bname\s*=\s*"([^"]*)""#).expect("static regex"));
static TOML_FIELD_VERSION_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"version\.ref\s*=\s*"([^"]*)""#).expect("static regex"));

/// Only the flat `key = "value"` shape for `[versions]` and the
/// inline-table-per-line shape for `[libraries]` — `libs.versions.toml`'s
/// two common forms, and the ones every Gradle version-catalog generator
/// (`./gradlew`'s own, Renovate, Dependabot) emits. A `[libraries.foo]`
/// dotted sub-table per entry is valid TOML but not produced by any tool
/// in practice; such a file's caret positions inside it simply resolve to
/// no context here rather than a wrong one.
fn toml_context(text: &str, caret: usize) -> Option<EditContext> {
    let mut section = String::new();
    let mut line_start = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let line_range = line_start..(line_start + trimmed.len());
        if let Some(caps) = TOML_SECTION.captures(trimmed) {
            section = caps[1].to_string();
        } else if caret_in(&line_range, caret) {
            return match section.as_str() {
                "versions" => toml_version_context(trimmed, line_start, caret),
                "libraries" => toml_library_context(trimmed, line_start, caret),
                _ => None,
            };
        }
        line_start += line.len();
    }
    None
}

/// Every `[versions]` key with its literal value and the value's own
/// range, and every `[libraries]` entry — D6's whole-document version
/// scan reads both: the version lives in the first, and is reached from
/// the second only through `version.ref`.
/// `(key, value, value's byte range)` for one `[versions]` table entry.
type TomlVersionEntry = (String, String, Range<usize>);

fn all_toml_sections(text: &str) -> (Vec<TomlVersionEntry>, Vec<TomlLibraryEntry>) {
    let mut versions = Vec::new();
    let mut libraries = Vec::new();
    let mut section = String::new();
    let mut line_start = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if let Some(caps) = TOML_SECTION.captures(trimmed) {
            section = caps[1].to_string();
        } else {
            match section.as_str() {
                "versions" => {
                    if let Some(caps) = TOML_VERSION_ENTRY.captures(trimmed) {
                        if let (Some(key), Some(value)) = (trimmed.split('=').next(), caps.get(1)) {
                            versions.push((
                                key.trim().to_string(),
                                value.as_str().to_string(),
                                (line_start + value.start())..(line_start + value.end()),
                            ));
                        }
                    }
                }
                "libraries" => {
                    let entry = TomlLibraryEntry {
                        module: TOML_FIELD_MODULE
                            .captures(trimmed)
                            .map(|c| c[1].to_string()),
                        group: TOML_FIELD_GROUP.captures(trimmed).map(|c| c[1].to_string()),
                        name: TOML_FIELD_NAME.captures(trimmed).map(|c| c[1].to_string()),
                        version_ref: TOML_FIELD_VERSION_REF
                            .captures(trimmed)
                            .map(|c| c[1].to_string()),
                    };
                    if entry.module.is_some() || entry.group.is_some() {
                        libraries.push(entry);
                    }
                }
                _ => {}
            }
        }
        line_start += line.len();
    }
    (versions, libraries)
}

fn toml_version_context(line: &str, line_start: usize, caret: usize) -> Option<EditContext> {
    let caps = TOML_VERSION_ENTRY.captures(line)?;
    let key = line.split('=').next()?.trim().to_string();
    let value = caps.get(1)?;
    let range = (line_start + value.start())..(line_start + value.end());
    caret_in(&range, caret).then_some(EditContext::TomlVersion { range, key })
}

fn toml_library_context(line: &str, line_start: usize, caret: usize) -> Option<EditContext> {
    let module = TOML_FIELD_MODULE.captures(line);
    let group = TOML_FIELD_GROUP.captures(line);
    let name = TOML_FIELD_NAME.captures(line);
    let version_ref = TOML_FIELD_VERSION_REF.captures(line);

    let entry = TomlLibraryEntry {
        module: module.as_ref().map(|c| c[1].to_string()),
        group: group.as_ref().map(|c| c[1].to_string()),
        name: name.as_ref().map(|c| c[1].to_string()),
        version_ref: version_ref.as_ref().map(|c| c[1].to_string()),
    };

    for (caps, field) in [
        (module, TomlLibraryField::Module),
        (group, TomlLibraryField::Group),
        (name, TomlLibraryField::Name),
        (version_ref, TomlLibraryField::VersionRef),
    ] {
        let Some(caps) = caps else { continue };
        let value = caps.get(1)?;
        let range = (line_start + value.start())..(line_start + value.end());
        if caret_in(&range, caret) {
            return Some(EditContext::TomlLibraryField {
                field,
                range,
                entry,
            });
        }
    }
    None
}

// ---------------------------------------------------------------------
// Whole-document version scan (D6)
// ---------------------------------------------------------------------

/// One dependency version declared anywhere in an open build file — D6's
/// "newer version available" hint works from every one of these, not just
/// the one under the caret [`context`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredVersion {
    pub group_id: String,
    pub artifact_id: String,
    pub current: String,
    /// Where an accepted "Update to X" fix replaces. For pom.xml and
    /// Gradle this is the version literal's own range; for
    /// `libs.versions.toml` it is the `[versions]` table's value range —
    /// the one place the literal actually lives, since a `[libraries]`
    /// entry only references it by key through `version.ref`, and
    /// editing the table's own value updates every entry that shares it.
    pub range: Range<usize>,
}

/// Every dependency version declared in `text`, across the whole
/// document — the scan D6's version-hint diagnostics run over on open,
/// save and after a sync.
pub fn declared_versions(path: &Path, text: &str) -> Vec<DeclaredVersion> {
    match classify(path) {
        Some(FileKind::Pom) => pom_declared_versions(text),
        Some(FileKind::Gradle) => gradle_declared_versions(text),
        Some(FileKind::VersionCatalog) => toml_declared_versions(text),
        None => Vec::new(),
    }
}

/// Review fix #5: does `s` look like an actual version literal a hint can
/// compare against, rather than a template reference no static scan can
/// resolve on its own (`${property}`, Gradle/Kotlin's `${ext.foo}` or
/// `$foo` string interpolation)? Treating an unresolved reference as a
/// literal produced a "newer version" hint on almost every real Maven
/// project (`${junit.version}` compares as a string no release is ever
/// "newer" than) and a quick fix that would have replaced the property
/// *reference* with a hardcoded literal, breaking the one place the
/// version is meant to be governed from.
fn is_resolvable_version(s: &str) -> bool {
    !s.is_empty() && !s.contains(['$', '{', '}'])
}

/// Resolves a Maven `${x}` property reference against this same
/// document's own `<properties>` table — never a parent POM's, which
/// would need opening a second file (`maven::pom`'s own reader documents
/// the identical limitation for the same reason). Returns the property's
/// own text-content range, not the `${x}` reference's: a "newer version"
/// fix must land on the declaration, not turn it into a hardcoded
/// literal. `None` for anything that is not a bare `${name}` reference,
/// a `project.*` built-in self-reference (`<properties>` cannot define
/// those), or a property this file does not declare.
fn resolve_pom_property(text: &str, reference: &str) -> Option<(String, Range<usize>)> {
    let name = reference.strip_prefix("${")?.strip_suffix('}')?;
    if name.starts_with("project.") {
        return None;
    }
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut element_stack: Vec<String> = Vec::new();
    while let Ok(event) = reader.read_event_into(&mut buf) {
        let pos_after = reader.buffer_position() as usize;
        match event {
            Event::Eof => break,
            Event::Start(tag) => element_stack.push(local_name(tag.name().as_ref())),
            Event::Text(text_event) => {
                let start = pos_after.saturating_sub(text_event.len());
                let is_the_property = element_stack.len() >= 2
                    && element_stack[element_stack.len() - 2] == "properties"
                    && element_stack.last().is_some_and(|e| e == name);
                if is_the_property {
                    let value = text_event
                        .decode()
                        .ok()
                        .and_then(|raw| {
                            quick_xml::escape::unescape(&raw)
                                .ok()
                                .map(|s| s.into_owned())
                        })
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                    return Some((value, start..pos_after));
                }
            }
            Event::End(_) => {
                element_stack.pop();
            }
            _ => {}
        }
        buf.clear();
    }
    None
}

fn pom_declared_versions(text: &str) -> Vec<DeclaredVersion> {
    pom_blocks(text)
        .into_iter()
        .filter_map(|block| {
            let (group_id, _) = block.group_id?;
            let (artifact_id, _) = block.artifact_id?;
            let (current, range) = block.version?;
            if group_id.is_empty() || artifact_id.is_empty() || current.is_empty() {
                return None;
            }
            let (current, range) = if current.starts_with("${") {
                resolve_pom_property(text, &current)?
            } else {
                (current, range)
            };
            if !is_resolvable_version(&current) {
                return None;
            }
            Some(DeclaredVersion {
                group_id,
                artifact_id,
                current,
                range,
            })
        })
        .collect()
}

fn gradle_declared_versions(text: &str) -> Vec<DeclaredVersion> {
    let mut found: Vec<DeclaredVersion> = all_gradle_coordinates(text)
        .into_iter()
        .filter_map(|(range, coordinate)| {
            let group_id = coordinate.group_id.filter(|v| !v.is_empty())?;
            let artifact_id = coordinate.artifact_id.filter(|v| !v.is_empty())?;
            let current = coordinate.version.filter(|v| is_resolvable_version(v))?;
            Some(DeclaredVersion {
                group_id,
                artifact_id,
                current,
                range,
            })
        })
        .collect();
    found.extend(
        all_gradle_plugin_versions(text)
            .into_iter()
            .filter_map(|(range, plugin_id)| {
                let id = plugin_id.filter(|v| !v.is_empty())?;
                let current = text.get(range.clone())?.to_string();
                if !is_resolvable_version(&current) {
                    return None;
                }
                Some(DeclaredVersion {
                    artifact_id: format!("{id}.gradle.plugin"),
                    group_id: id,
                    current,
                    range,
                })
            }),
    );
    found
}

fn toml_declared_versions(text: &str) -> Vec<DeclaredVersion> {
    let (versions, libraries) = all_toml_sections(text);
    let mut seen_ranges = std::collections::BTreeSet::new();
    let mut found = Vec::new();
    for library in &libraries {
        let Some(key) = library.version_ref.as_deref().filter(|v| !v.is_empty()) else {
            continue;
        };
        let Some((_, current, range)) = versions.iter().find(|(k, _, _)| k == key) else {
            continue;
        };
        if !is_resolvable_version(current) {
            continue;
        }
        if !seen_ranges.insert(range.start) {
            continue;
        }
        let (group_id, artifact_id) = match &library.module {
            Some(module) => match module.split_once(':') {
                Some((g, a)) => (g.to_string(), a.to_string()),
                None => continue,
            },
            None => match (&library.group, &library.name) {
                (Some(g), Some(a)) if !g.is_empty() && !a.is_empty() => (g.clone(), a.clone()),
                _ => continue,
            },
        };
        found.push(DeclaredVersion {
            group_id,
            artifact_id,
            current: current.clone(),
            range: range.clone(),
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(path: &str, text: &str, caret: usize) -> Option<EditContext> {
        context(Path::new(path), text, caret)
    }

    // ---- pom.xml -------------------------------------------------

    const POM: &str = r#"<project>
  <dependencies>
    <dependency>
      <groupId>com.google.guava</groupId>
      <artifactId>guava</artifactId>
      <version>32.1.3-jre</version>
    </dependency>
  </dependencies>
</project>
"#;

    #[test]
    fn pom_artifact_id_reports_its_own_range_and_sibling_values() {
        let caret = POM.find("guava</artifactId>").unwrap() + 2;
        let context = ctx("/proj/pom.xml", POM, caret).expect("context");
        let EditContext::PomCoordinate {
            part,
            range,
            coordinate,
        } = context
        else {
            panic!("expected PomCoordinate");
        };
        assert_eq!(part, CoordinatePart::ArtifactId);
        assert_eq!(&POM[range], "guava");
        assert_eq!(coordinate.group_id.as_deref(), Some("com.google.guava"));
        assert_eq!(coordinate.version.as_deref(), Some("32.1.3-jre"));
    }

    #[test]
    fn pom_version_outside_a_dependency_plugin_or_parent_is_no_context() {
        let text = "<project><version>1.0</version></project>";
        let caret = text.find("1.0").unwrap();
        assert_eq!(ctx("/proj/pom.xml", text, caret), None);
    }

    #[test]
    fn pom_parent_version_is_a_coordinate_context() {
        let text = "<project><parent><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter-parent</artifactId><version>3.2.0</version></parent></project>";
        let caret = text.find("3.2.0").unwrap() + 2;
        let context = ctx("/proj/pom.xml", text, caret).expect("context");
        assert!(matches!(
            context,
            EditContext::PomCoordinate {
                part: CoordinatePart::Version,
                ..
            }
        ));
    }

    // ---- build.gradle(.kts) --------------------------------------

    #[test]
    fn gradle_kotlin_dsl_artifact_segment() {
        let text = r#"dependencies {
    implementation("com.google.guava:guava:32.1.3-jre")
}
"#;
        let caret = text.find("guava:32").unwrap() + 2;
        let context = ctx("/proj/build.gradle.kts", text, caret).expect("context");
        let EditContext::GradleCoordinate {
            part,
            range,
            coordinate,
            configuration,
        } = context
        else {
            panic!("expected GradleCoordinate");
        };
        assert_eq!(part, CoordinatePart::ArtifactId);
        assert_eq!(&text[range], "guava");
        assert_eq!(configuration, "implementation");
        assert_eq!(coordinate.group_id.as_deref(), Some("com.google.guava"));
        assert_eq!(coordinate.version.as_deref(), Some("32.1.3-jre"));
    }

    #[test]
    fn gradle_groovy_single_quoted_version_segment_replaces_only_the_version() {
        let text =
            "dependencies {\n    testImplementation 'org.junit.jupiter:junit-jupiter:5.10.0'\n}\n";
        let caret = text.find("5.10.0").unwrap() + 3;
        let context = ctx("/proj/build.gradle", text, caret).expect("context");
        let EditContext::GradleCoordinate {
            part,
            range,
            configuration,
            ..
        } = context
        else {
            panic!("expected GradleCoordinate");
        };
        assert_eq!(part, CoordinatePart::Version);
        assert_eq!(&text[range], "5.10.0");
        assert_eq!(configuration, "testImplementation");
    }

    #[test]
    fn gradle_plugin_version_literal() {
        let text = "plugins {\n    id(\"org.springframework.boot\") version \"3.2.0\"\n}\n";
        let caret = text.find("3.2.0").unwrap() + 2;
        let context = ctx("/proj/build.gradle.kts", text, caret).expect("context");
        let EditContext::GradlePluginVersion { range, plugin_id } = context else {
            panic!("expected GradlePluginVersion");
        };
        assert_eq!(&text[range], "3.2.0");
        assert_eq!(plugin_id.as_deref(), Some("org.springframework.boot"));
    }

    #[test]
    fn gradle_caret_outside_any_configuration_call_is_no_context() {
        let text = "plugins {\n    id(\"java\")\n}\n";
        let caret = text.find("java").unwrap();
        assert_eq!(ctx("/proj/build.gradle.kts", text, caret), None);
    }

    // ---- libs.versions.toml ---------------------------------------

    const CATALOG: &str = r#"[versions]
guava = "32.1.3-jre"

[libraries]
guava = { module = "com.google.guava:guava", version.ref = "guava" }
okhttp = { group = "com.squareup.okhttp3", name = "okhttp", version.ref = "guava" }
"#;

    #[test]
    fn toml_versions_table_value() {
        let caret = CATALOG.find("32.1.3-jre").unwrap() + 2;
        let context = ctx("/proj/gradle/libs.versions.toml", CATALOG, caret).expect("context");
        let EditContext::TomlVersion { range, key } = context else {
            panic!("expected TomlVersion");
        };
        assert_eq!(&CATALOG[range], "32.1.3-jre");
        assert_eq!(key, "guava");
    }

    #[test]
    fn toml_libraries_module_field() {
        let caret = CATALOG.find("com.google.guava:guava").unwrap() + 2;
        let context = ctx("/proj/gradle/libs.versions.toml", CATALOG, caret).expect("context");
        let EditContext::TomlLibraryField {
            field,
            range,
            entry,
        } = context
        else {
            panic!("expected TomlLibraryField");
        };
        assert_eq!(field, TomlLibraryField::Module);
        assert_eq!(&CATALOG[range], "com.google.guava:guava");
        assert_eq!(entry.version_ref.as_deref(), Some("guava"));
    }

    #[test]
    fn toml_libraries_group_and_name_fields_and_sibling_values() {
        let caret = CATALOG.find("okhttp\", version").unwrap() + 1;
        let context = ctx("/proj/gradle/libs.versions.toml", CATALOG, caret).expect("context");
        let EditContext::TomlLibraryField {
            field,
            range,
            entry,
        } = context
        else {
            panic!("expected TomlLibraryField");
        };
        assert_eq!(field, TomlLibraryField::Name);
        assert_eq!(&CATALOG[range], "okhttp");
        assert_eq!(entry.group.as_deref(), Some("com.squareup.okhttp3"));
    }

    #[test]
    fn toml_caret_on_the_entry_key_itself_is_no_context() {
        let caret = CATALOG.find("okhttp = {").unwrap() + 2;
        assert_eq!(ctx("/proj/gradle/libs.versions.toml", CATALOG, caret), None);
    }

    #[test]
    fn non_build_file_path_is_no_context() {
        assert_eq!(ctx("/proj/src/Main.java", "class Main {}", 0), None);
    }

    // ---- is_build_file (review fix #7) -----------------------------

    #[test]
    fn is_build_file_recognises_every_format_at_any_depth() {
        assert!(is_build_file(Path::new("/proj/pom.xml")));
        // The regression review fix #7 exists for: a module's own build
        // file, not just the project root's — a literal root-relative
        // glob (`jvm_build_core::sync::is_build_file`'s own job) would
        // miss this entirely.
        assert!(is_build_file(Path::new("/proj/app/build.gradle")));
        assert!(is_build_file(Path::new(
            "/proj/lib/nested/build.gradle.kts"
        )));
        assert!(is_build_file(Path::new("/proj/gradle/libs.versions.toml")));
        assert!(!is_build_file(Path::new("/proj/src/Main.java")));
    }

    // ---- declared_versions (D6) ------------------------------------

    #[test]
    fn pom_declared_versions_finds_every_dependency_and_the_parent() {
        let text = r#"<project>
  <parent>
    <groupId>org.springframework.boot</groupId>
    <artifactId>spring-boot-starter-parent</artifactId>
    <version>3.2.0</version>
  </parent>
  <dependencies>
    <dependency>
      <groupId>com.google.guava</groupId>
      <artifactId>guava</artifactId>
      <version>32.1.3-jre</version>
    </dependency>
    <dependency>
      <groupId>org.example</groupId>
      <artifactId>managed-by-bom</artifactId>
    </dependency>
  </dependencies>
</project>
"#;
        let found = declared_versions(Path::new("/proj/pom.xml"), text);
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .any(|d| d.artifact_id == "spring-boot-starter-parent" && d.current == "3.2.0"));
        assert!(found
            .iter()
            .any(|d| d.artifact_id == "guava" && d.current == "32.1.3-jre"));
        // No version at all (relying on a BOM) is not a declared version.
        assert!(!found.iter().any(|d| d.artifact_id == "managed-by-bom"));
    }

    /// Review fix #5: a `${x}` version resolves through this same file's
    /// own `<properties>`, and the hint's range lands on the property's
    /// own declaration — not the `${x}` reference — since a fix must
    /// update the governed value, not turn it into a hardcoded literal.
    #[test]
    fn pom_property_referenced_version_resolves_through_properties() {
        let text = r#"<project>
  <properties>
    <junit.version>5.10.3</junit.version>
  </properties>
  <dependencies>
    <dependency>
      <groupId>org.junit.jupiter</groupId>
      <artifactId>junit-jupiter</artifactId>
      <version>${junit.version}</version>
    </dependency>
  </dependencies>
</project>
"#;
        let found = declared_versions(Path::new("/proj/pom.xml"), text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].current, "5.10.3");
        // The range covers the <properties> entry's own value, not the
        // dependency's `${junit.version}` reference.
        assert_eq!(&text[found[0].range.clone()], "5.10.3");
        assert!(text[..found[0].range.start].contains("<junit.version>"));
    }

    /// Review fix #5: a property this file does not declare (the common
    /// case — it lives in a parent POM this reader cannot open) yields no
    /// declared version rather than a bogus literal `"${x}"` string.
    #[test]
    fn pom_property_referenced_version_not_declared_here_is_skipped() {
        let text = r#"<project>
  <dependencies>
    <dependency>
      <groupId>org.junit.jupiter</groupId>
      <artifactId>junit-jupiter</artifactId>
      <version>${junit.version}</version>
    </dependency>
  </dependencies>
</project>
"#;
        assert_eq!(declared_versions(Path::new("/proj/pom.xml"), text), vec![]);
    }

    /// Review fix #4: a `<dependency>`'s own `<exclusions>` carry a nested
    /// `<exclusion>` with its own `<groupId>`/`<artifactId>` — several
    /// levels deeper than the dependency's own coordinate — which must
    /// not overwrite it.
    #[test]
    fn pom_exclusion_children_do_not_overwrite_the_enclosing_dependency() {
        let text = r#"<project>
  <dependencies>
    <dependency>
      <groupId>com.google.guava</groupId>
      <artifactId>guava</artifactId>
      <version>32.1.3-jre</version>
      <exclusions>
        <exclusion>
          <groupId>com.google.code.findbugs</groupId>
          <artifactId>jsr305</artifactId>
        </exclusion>
      </exclusions>
    </dependency>
  </dependencies>
</project>
"#;
        let found = declared_versions(Path::new("/proj/pom.xml"), text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].group_id, "com.google.guava");
        assert_eq!(found[0].artifact_id, "guava");
        assert_eq!(found[0].current, "32.1.3-jre");
    }

    /// Review fix #4: a plugin's `<configuration>` can carry its own
    /// deeply-nested `<groupId>`/`<artifactId>` (the dependency plugin's
    /// `<artifactItems><artifactItem>`, here), which must not overwrite
    /// the plugin's own coordinate either.
    #[test]
    fn pom_plugin_configuration_children_do_not_overwrite_the_plugin() {
        let text = r#"<project>
  <build>
    <plugins>
      <plugin>
        <groupId>org.apache.maven.plugins</groupId>
        <artifactId>maven-dependency-plugin</artifactId>
        <version>3.6.1</version>
        <configuration>
          <artifactItems>
            <artifactItem>
              <groupId>com.example</groupId>
              <artifactId>bundled-jar</artifactId>
              <version>9.9.9</version>
            </artifactItem>
          </artifactItems>
        </configuration>
      </plugin>
    </plugins>
  </build>
</project>
"#;
        let found = declared_versions(Path::new("/proj/pom.xml"), text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].group_id, "org.apache.maven.plugins");
        assert_eq!(found[0].artifact_id, "maven-dependency-plugin");
        assert_eq!(found[0].current, "3.6.1");
    }

    #[test]
    fn gradle_declared_versions_finds_dependencies_and_plugins_not_two_segment_calls() {
        let text = r#"plugins {
    id("org.springframework.boot") version "3.2.0"
}
dependencies {
    implementation("com.google.guava:guava:32.1.3-jre")
    api("org.example:no-version")
}
"#;
        let found = declared_versions(Path::new("/proj/build.gradle.kts"), text);
        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .any(|d| d.group_id == "org.springframework.boot"
                && d.artifact_id == "org.springframework.boot.gradle.plugin"
                && d.current == "3.2.0"));
        assert!(found
            .iter()
            .any(|d| d.artifact_id == "guava" && d.current == "32.1.3-jre"));
        assert!(!found.iter().any(|d| d.artifact_id == "no-version"));
    }

    /// Review fix #5: a Kotlin/Groovy string-interpolated version
    /// (`${extraProperty}`) cannot be resolved by a static scan and must
    /// not be treated as a literal to compare against Central.
    #[test]
    fn gradle_interpolated_version_is_skipped() {
        let text = r#"dependencies {
    implementation("com.google.guava:guava:${guavaVersion}")
}
"#;
        assert_eq!(
            declared_versions(Path::new("/proj/build.gradle.kts"), text),
            vec![]
        );
    }

    #[test]
    fn toml_declared_versions_resolves_version_ref() {
        let text = r#"[versions]
guava = "32.1.3-jre"
junit = "5.10.0"

[libraries]
guava = { module = "com.google.guava:guava", version.ref = "guava" }
junit-jupiter = { group = "org.junit.jupiter", name = "junit-jupiter", version.ref = "junit" }
"#;
        let found = declared_versions(Path::new("/proj/gradle/libs.versions.toml"), text);
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|d| d.group_id == "com.google.guava"
            && d.artifact_id == "guava"
            && d.current == "32.1.3-jre"));
        assert!(found.iter().any(|d| d.group_id == "org.junit.jupiter"
            && d.artifact_id == "junit-jupiter"
            && d.current == "5.10.0"));
    }

    #[test]
    fn toml_declared_versions_dedups_two_libraries_sharing_one_versions_key() {
        // Two artifacts referencing the same `[versions]` key share one
        // physical location in the document — one hint there, not two
        // competing ones, keyed off whichever library named the key
        // first.
        let text = r#"[versions]
guava = "32.1.3-jre"

[libraries]
guava = { module = "com.google.guava:guava", version.ref = "guava" }
guava-testlib = { group = "com.google.guava", name = "guava-testlib", version.ref = "guava" }
"#;
        let found = declared_versions(Path::new("/proj/gradle/libs.versions.toml"), text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].artifact_id, "guava");
    }
}
