//! Parsing `mvn help:effective-pom`'s output (A5): every property already
//! substituted, the parent already merged in, so — unlike [`super::pom`] —
//! nothing here needs to interpolate `${}` itself.
//!
//! `tests/fixtures/xml/maven-single-effective-pom.xml` is a real capture
//! (`mvn -B help:effective-pom -Doutput=...` against `tests/fixtures/maven-single`
//! inside the `linux-jvm` image), including the Maven Help Plugin's leading
//! XML comments — this module's parser has to skip those the way a real
//! caller's input would require, not a hand-trimmed file with them removed.

use std::path::PathBuf;

use super::xml::{parse as parse_xml, XmlError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveDependency {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub scope: String,
}

/// A plugin bound into the build, its goals read from its own
/// `<executions>` — what actually runs, not the plugin's full goal
/// catalogue (that is `super::goals::goals_from_jar`'s answer, for
/// build-file editing rather than this dock's tree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePlugin {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub goals: Vec<String>,
}

/// Maven core plugins' effective-pom entry omits `groupId` when it is this
/// default — the same convention `super::pom::DEFAULT_PLUGIN_GROUP` reads
/// for the static POM.
const DEFAULT_PLUGIN_GROUP: &str = "org.apache.maven.plugins";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EffectivePom {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
    pub packaging: String,
    pub dependencies: Vec<EffectiveDependency>,
    pub plugins: Vec<EffectivePlugin>,
    pub source_directory: Option<PathBuf>,
    pub test_source_directory: Option<PathBuf>,
    pub output_directory: Option<PathBuf>,
    pub test_output_directory: Option<PathBuf>,
}

pub fn parse(text: &str) -> Result<EffectivePom, XmlError> {
    let root = parse_xml(text)?;

    let dependencies = root
        .child("dependencies")
        .map(|d| {
            d.children_named("dependency")
                .map(|dep| EffectiveDependency {
                    group_id: dep.text_of("groupId").unwrap_or_default(),
                    artifact_id: dep.text_of("artifactId").unwrap_or_default(),
                    version: dep.text_of("version").unwrap_or_default(),
                    scope: dep
                        .text_of("scope")
                        .unwrap_or_else(|| "compile".to_string()),
                })
                .collect()
        })
        .unwrap_or_default();

    let build = root.child("build");
    let plugins = build
        .and_then(|b| b.child("plugins"))
        .map(|p| {
            p.children_named("plugin")
                .map(|plugin| {
                    let mut goals: Vec<String> = Vec::new();
                    if let Some(executions) = plugin.child("executions") {
                        for execution in executions.children_named("execution") {
                            if let Some(goal_list) = execution.child("goals") {
                                for goal in goal_list.children_named("goal") {
                                    let name = goal.text.trim();
                                    if !name.is_empty() && !goals.iter().any(|g| g == name) {
                                        goals.push(name.to_string());
                                    }
                                }
                            }
                        }
                    }
                    EffectivePlugin {
                        group_id: plugin
                            .text_of("groupId")
                            .unwrap_or_else(|| DEFAULT_PLUGIN_GROUP.to_string()),
                        artifact_id: plugin.text_of("artifactId").unwrap_or_default(),
                        version: plugin.text_of("version").unwrap_or_default(),
                        goals,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(EffectivePom {
        group_id: root.text_of("groupId").unwrap_or_default(),
        artifact_id: root.text_of("artifactId").unwrap_or_default(),
        version: root.text_of("version").unwrap_or_default(),
        packaging: root
            .text_of("packaging")
            .unwrap_or_else(|| "jar".to_string()),
        dependencies,
        plugins,
        source_directory: build
            .and_then(|b| b.text_of("sourceDirectory"))
            .map(PathBuf::from),
        test_source_directory: build
            .and_then(|b| b.text_of("testSourceDirectory"))
            .map(PathBuf::from),
        output_directory: build
            .and_then(|b| b.text_of("outputDirectory"))
            .map(PathBuf::from),
        test_output_directory: build
            .and_then(|b| b.text_of("testOutputDirectory"))
            .map(PathBuf::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/xml/maven-single-effective-pom.xml");

    #[test]
    fn the_real_captured_effective_pom_parses() {
        let pom = parse(FIXTURE).expect("valid");
        assert_eq!(pom.group_id, "com.example");
        assert_eq!(pom.artifact_id, "maven-single");
        assert_eq!(pom.version, "1.0.0");
        assert_eq!(pom.packaging, "jar");
    }

    #[test]
    fn bound_plugins_read_their_own_executions_goals() {
        let pom = parse(FIXTURE).expect("valid");
        let surefire = pom
            .plugins
            .iter()
            .find(|p| p.artifact_id == "maven-surefire-plugin")
            .expect("surefire plugin");
        assert_eq!(surefire.group_id, "org.apache.maven.plugins");
        assert_eq!(surefire.version, "3.3.1");
        assert_eq!(surefire.goals, vec!["test".to_string()]);

        // pluginManagement's own <plugins> (declared but not bound into this
        // build) must never be read as if it were the real, active list.
        assert!(!pom
            .plugins
            .iter()
            .any(|p| p.artifact_id == "maven-release-plugin"));
    }

    #[test]
    fn dependencies_are_fully_resolved_with_no_interpolation_needed() {
        let pom = parse(FIXTURE).expect("valid");
        assert_eq!(pom.dependencies.len(), 1);
        let dep = &pom.dependencies[0];
        assert_eq!(dep.group_id, "org.junit.jupiter");
        assert_eq!(dep.artifact_id, "junit-jupiter");
        assert_eq!(dep.version, "5.10.3");
        assert_eq!(dep.scope, "test");
    }

    #[test]
    fn build_paths_are_read() {
        let pom = parse(FIXTURE).expect("valid");
        assert!(pom
            .source_directory
            .unwrap()
            .to_string_lossy()
            .ends_with("src/main/java"));
        assert!(pom
            .test_source_directory
            .unwrap()
            .to_string_lossy()
            .ends_with("src/test/java"));
    }

    #[test]
    fn leading_help_plugin_comments_do_not_break_parsing() {
        assert!(FIXTURE.trim_start().starts_with("<?xml"));
        assert!(FIXTURE.contains("Generated by Maven Help Plugin"));
        assert!(parse(FIXTURE).is_ok());
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        assert!(parse("<project><unclosed></project>").is_err());
    }
}
