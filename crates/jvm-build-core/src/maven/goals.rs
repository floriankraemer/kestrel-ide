//! Plugin goals from a local-repo jar's `META-INF/maven/plugin.xml` (A5),
//! and the static Maven lifecycle phase list.
//!
//! Offline and instant: the jar is already on disk the moment a project
//! declares the plugin as a dependency (Maven fetched it to resolve the
//! build in the first place), so this never runs a process — unlike
//! [`super::sync`], which does.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::xml::parse as parse_xml;

/// The default lifecycle's phases, in order — Maven's own, unchanged since
/// Maven 3 and not worth re-deriving per project.
pub const LIFECYCLE_PHASES: &[&str] = &[
    "validate",
    "initialize",
    "generate-sources",
    "process-sources",
    "generate-resources",
    "process-resources",
    "compile",
    "process-classes",
    "generate-test-sources",
    "process-test-sources",
    "generate-test-resources",
    "process-test-resources",
    "test-compile",
    "process-test-classes",
    "test",
    "prepare-package",
    "package",
    "pre-integration-test",
    "integration-test",
    "post-integration-test",
    "verify",
    "install",
    "deploy",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginGoal {
    pub goal: String,
    pub description: String,
}

#[derive(Debug)]
pub enum GoalsError {
    JarNotFound(PathBuf),
    Zip(String),
    NoPluginDescriptor,
    Xml(String),
}

impl std::fmt::Display for GoalsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoalsError::JarNotFound(path) => write!(f, "no such jar: {}", path.display()),
            GoalsError::Zip(message) => write!(f, "cannot read jar: {message}"),
            GoalsError::NoPluginDescriptor => {
                write!(f, "META-INF/maven/plugin.xml is not in this jar")
            }
            GoalsError::Xml(message) => write!(f, "malformed plugin.xml: {message}"),
        }
    }
}

/// Where a plugin's jar sits in a local Maven repository, Maven's own
/// layout: `<repo>/<group, dots as slashes>/<artifact>/<version>/<artifact>-<version>.jar`.
pub fn local_repo_jar_path(
    local_repo: &Path,
    group_id: &str,
    artifact_id: &str,
    version: &str,
) -> PathBuf {
    local_repo
        .join(group_id.replace('.', "/"))
        .join(artifact_id)
        .join(version)
        .join(format!("{artifact_id}-{version}.jar"))
}

/// Read `META-INF/maven/plugin.xml` out of a jar and return its every
/// declared goal.
pub fn goals_from_jar(jar_path: &Path) -> Result<Vec<PluginGoal>, GoalsError> {
    let file = std::fs::File::open(jar_path)
        .map_err(|_| GoalsError::JarNotFound(jar_path.to_path_buf()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| GoalsError::Zip(err.to_string()))?;
    let mut entry = archive
        .by_name("META-INF/maven/plugin.xml")
        .map_err(|_| GoalsError::NoPluginDescriptor)?;
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|err| GoalsError::Zip(err.to_string()))?;
    drop(entry);
    parse_plugin_xml(&text).map_err(|err| GoalsError::Xml(err.to_string()))
}

/// The text-parsing half, split out so a unit test can exercise it against
/// a real captured `plugin.xml` without needing a jar on disk — the zip
/// mechanics ([`goals_from_jar`]) are tested separately, against a jar this
/// crate writes and reads back itself.
fn parse_plugin_xml(text: &str) -> Result<Vec<PluginGoal>, super::xml::XmlError> {
    let root = parse_xml(text)?;
    let goals = root
        .child("mojos")
        .map(|mojos| {
            mojos
                .children_named("mojo")
                .map(|mojo| PluginGoal {
                    goal: mojo.text_of("goal").unwrap_or_default(),
                    description: mojo.text_of("description").unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(goals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const REAL_PLUGIN_XML: &str =
        include_str!("../../tests/fixtures/xml/maven-dependency-plugin.xml");

    #[test]
    fn the_real_captured_plugin_descriptor_yields_its_goals() {
        let goals = parse_plugin_xml(REAL_PLUGIN_XML).expect("valid");
        assert_eq!(goals.len(), 2);
        let tree = goals.iter().find(|g| g.goal == "tree").expect("tree goal");
        assert!(tree.description.contains("dependency tree"));
        assert!(goals.iter().any(|g| g.goal == "list"));
    }

    #[test]
    fn local_repo_jar_path_follows_mavens_own_layout() {
        let path = local_repo_jar_path(
            Path::new("/home/u/.m2/repository"),
            "org.apache.maven.plugins",
            "maven-dependency-plugin",
            "3.6.1",
        );
        assert_eq!(
            path,
            PathBuf::from(
                "/home/u/.m2/repository/org/apache/maven/plugins/maven-dependency-plugin/3.6.1/maven-dependency-plugin-3.6.1.jar"
            )
        );
    }

    #[test]
    fn a_missing_jar_is_reported_not_panicked() {
        let err = goals_from_jar(Path::new("/does/not/exist.jar")).unwrap_err();
        assert!(matches!(err, GoalsError::JarNotFound(_)));
    }

    #[test]
    fn a_real_jar_this_test_writes_and_reads_back_round_trips_its_goals() {
        // Proves the zip half end-to-end without a committed binary
        // fixture: build a jar with exactly the shape a real plugin jar
        // has (one entry, META-INF/maven/plugin.xml), then read it back
        // through the same goals_from_jar a real sync would call.
        let dir = tempfile::tempdir().expect("temp dir");
        let jar_path = dir.path().join("plugin.jar");
        let file = std::fs::File::create(&jar_path).expect("create jar");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file::<_, ()>("META-INF/maven/plugin.xml", Default::default())
            .expect("start entry");
        writer
            .write_all(REAL_PLUGIN_XML.as_bytes())
            .expect("write entry");
        writer.finish().expect("finish jar");

        let goals = goals_from_jar(&jar_path).expect("read jar");
        assert_eq!(goals.len(), 2);
    }

    #[test]
    fn lifecycle_phases_are_in_order_and_include_the_common_ones() {
        assert_eq!(LIFECYCLE_PHASES.first(), Some(&"validate"));
        assert_eq!(LIFECYCLE_PHASES.last(), Some(&"deploy"));
        assert!(LIFECYCLE_PHASES.contains(&"compile"));
        assert!(LIFECYCLE_PHASES.contains(&"test"));
        assert!(LIFECYCLE_PHASES.contains(&"package"));
    }
}
