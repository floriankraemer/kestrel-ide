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

fn pom_context(text: &str, caret: usize) -> Option<EditContext> {
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
                if let Some(parent) = element_stack.last() {
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
                        match parent.as_str() {
                            "groupId" => block.group_id = Some((value, start..end)),
                            "artifactId" => block.artifact_id = Some((value, start..end)),
                            "version" => block.version = Some((value, start..end)),
                            _ => {}
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
}
