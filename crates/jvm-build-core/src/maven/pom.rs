//! A static `pom.xml` read: coordinates, parent, `<modules>`, properties,
//! declared dependencies/plugins/profiles, with `${}` interpolation for the
//! simple case — a property this POM's own `<properties>` defines, or one
//! of the three `project.*` self-references (A5).
//!
//! Deliberately not full Maven inheritance: a child POM's own `${}` may
//! reference a property only its *parent* defines, which this reader
//! cannot see without opening a second file (and the parent may itself
//! come from a repository, not a sibling directory). That resolution needs
//! the *effective* POM Maven itself computes — see [`super::effective_pom`]
//! — which is why this module's doc comment says "simple cases" rather
//! than claiming full resolution.

use std::collections::BTreeMap;

use super::xml::{parse as parse_xml, Node, XmlError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentRef {
    pub group_id: String,
    pub artifact_id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PomDependency {
    pub group_id: String,
    pub artifact_id: String,
    /// Absent for a dependency that relies entirely on
    /// `<dependencyManagement>` (its own or an inherited one this reader
    /// cannot see) for its version.
    pub version: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PomPlugin {
    /// Defaults to `org.apache.maven.plugins`, Maven's own rule for a
    /// plugin declared with no `<groupId>`.
    pub group_id: String,
    pub artifact_id: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pom {
    /// Absent when inherited from a parent this reader did not open.
    pub group_id: Option<String>,
    pub artifact_id: String,
    /// Absent when inherited from a parent.
    pub version: Option<String>,
    /// Defaults to `"jar"`, Maven's own default.
    pub packaging: String,
    pub parent: Option<ParentRef>,
    pub modules: Vec<String>,
    pub properties: BTreeMap<String, String>,
    pub dependencies: Vec<PomDependency>,
    pub dependency_management: Vec<PomDependency>,
    pub plugins: Vec<PomPlugin>,
    /// Just the declared profile ids — the settings page's job is "which
    /// profiles exist to toggle", not what each one changes.
    pub profiles: Vec<String>,
}

const DEFAULT_PLUGIN_GROUP: &str = "org.apache.maven.plugins";
const DEFAULT_PACKAGING: &str = "jar";

pub fn parse(text: &str) -> Result<Pom, XmlError> {
    let root = parse_xml(text)?;
    Ok(to_pom(&root))
}

fn to_pom(root: &Node) -> Pom {
    let parent = root.child("parent").map(|p| ParentRef {
        group_id: p.text_of("groupId").unwrap_or_default(),
        artifact_id: p.text_of("artifactId").unwrap_or_default(),
        version: p.text_of("version").unwrap_or_default(),
    });

    let group_id = root.text_of("groupId").or_else(|| {
        parent
            .as_ref()
            .map(|parent_ref| parent_ref.group_id.clone())
    });
    let version = root
        .text_of("version")
        .or_else(|| parent.as_ref().map(|parent_ref| parent_ref.version.clone()));

    let properties: BTreeMap<String, String> = root
        .child("properties")
        .map(|props| {
            props
                .children
                .iter()
                .map(|c| (c.name.clone(), c.text.trim().to_string()))
                .collect()
        })
        .unwrap_or_default();

    let modules = root
        .child("modules")
        .map(|m| {
            m.children_named("module")
                .map(|c| c.text.trim().to_string())
                .collect()
        })
        .unwrap_or_default();

    let self_refs = SelfRefs {
        group_id: group_id.clone().unwrap_or_default(),
        artifact_id: root.text_of("artifactId").unwrap_or_default(),
        version: version.clone().unwrap_or_default(),
    };

    let dependencies = root
        .child("dependencies")
        .map(|d| to_dependencies(d, &properties, &self_refs))
        .unwrap_or_default();

    let dependency_management = root
        .child("dependencyManagement")
        .and_then(|dm| dm.child("dependencies"))
        .map(|d| to_dependencies(d, &properties, &self_refs))
        .unwrap_or_default();

    let plugins = root
        .child("build")
        .and_then(|b| b.child("plugins"))
        .map(|p| to_plugins(p, &properties, &self_refs))
        .unwrap_or_default();

    let profiles = root
        .child("profiles")
        .map(|p| {
            p.children_named("profile")
                .filter_map(|profile| profile.text_of("id"))
                .collect()
        })
        .unwrap_or_default();

    Pom {
        group_id,
        artifact_id: root.text_of("artifactId").unwrap_or_default(),
        version,
        packaging: root
            .text_of("packaging")
            .unwrap_or_else(|| DEFAULT_PACKAGING.to_string()),
        parent,
        modules,
        properties,
        dependencies,
        dependency_management,
        plugins,
        profiles,
    }
}

/// The three `${project.*}` tokens this reader resolves without a second
/// file — a POM's own coordinates, spelled the way Maven's interpolator
/// spells them.
struct SelfRefs {
    group_id: String,
    artifact_id: String,
    version: String,
}

fn to_dependencies(
    node: &Node,
    properties: &BTreeMap<String, String>,
    self_refs: &SelfRefs,
) -> Vec<PomDependency> {
    node.children_named("dependency")
        .map(|d| PomDependency {
            group_id: interpolate(
                d.text_of("groupId").unwrap_or_default(),
                properties,
                self_refs,
            ),
            artifact_id: interpolate(
                d.text_of("artifactId").unwrap_or_default(),
                properties,
                self_refs,
            ),
            version: d
                .text_of("version")
                .map(|v| interpolate(v, properties, self_refs)),
            scope: d.text_of("scope"),
        })
        .collect()
}

fn to_plugins(
    node: &Node,
    properties: &BTreeMap<String, String>,
    self_refs: &SelfRefs,
) -> Vec<PomPlugin> {
    node.children_named("plugin")
        .map(|p| PomPlugin {
            group_id: p
                .text_of("groupId")
                .map(|g| interpolate(g, properties, self_refs))
                .unwrap_or_else(|| DEFAULT_PLUGIN_GROUP.to_string()),
            artifact_id: interpolate(
                p.text_of("artifactId").unwrap_or_default(),
                properties,
                self_refs,
            ),
            version: p
                .text_of("version")
                .map(|v| interpolate(v, properties, self_refs)),
        })
        .collect()
}

/// Replace every `${token}` in `value` with a property of the same name
/// (from this POM's own `<properties>` or one of the three
/// `project.groupId`/`project.artifactId`/`project.version` self-refs),
/// left exactly as written when nothing this reader knows about matches —
/// visibly unresolved beats a silently empty string, the same rule
/// `run_core::macros` follows for an unresolvable run-configuration token.
fn interpolate(
    value: String,
    properties: &BTreeMap<String, String>,
    self_refs: &SelfRefs,
) -> String {
    if !value.contains("${") {
        return value;
    }
    let mut result = String::with_capacity(value.len());
    let mut rest = value.as_str();
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            result.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let token = &after[..end];
        let resolved = match token {
            "project.groupId" | "pom.groupId" => Some(self_refs.group_id.as_str()),
            "project.artifactId" | "pom.artifactId" => Some(self_refs.artifact_id.as_str()),
            "project.version" | "pom.version" => Some(self_refs.version.as_str()),
            _ => properties.get(token).map(String::as_str),
        };
        match resolved {
            Some(text) => result.push_str(text),
            None => {
                result.push_str("${");
                result.push_str(token);
                result.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE_MODULE_POM: &str = include_str!("../../tests/fixtures/maven-single/pom.xml");
    const MULTI_ROOT_POM: &str = include_str!("../../tests/fixtures/maven-multi/pom.xml");
    const MULTI_APP_POM: &str = include_str!("../../tests/fixtures/maven-multi/app/pom.xml");
    const MULTI_LIB_POM: &str = include_str!("../../tests/fixtures/maven-multi/lib/pom.xml");

    #[test]
    fn a_single_module_pom_reads_coordinates_and_a_property_interpolated_dependency() {
        let pom = parse(SINGLE_MODULE_POM).expect("valid");
        assert_eq!(pom.group_id.as_deref(), Some("com.example"));
        assert_eq!(pom.artifact_id, "maven-single");
        assert_eq!(pom.version.as_deref(), Some("1.0.0"));
        assert_eq!(pom.packaging, "jar");
        assert!(pom.parent.is_none());

        let dep = &pom.dependencies[0];
        assert_eq!(dep.group_id, "org.junit.jupiter");
        assert_eq!(dep.artifact_id, "junit-jupiter");
        // The pom spells this as ${junit.version}; interpolation resolves
        // it against the pom's own <properties>.
        assert_eq!(dep.version.as_deref(), Some("5.10.3"));
        assert_eq!(dep.scope.as_deref(), Some("test"));

        assert_eq!(pom.plugins[0].artifact_id, "maven-surefire-plugin");
        assert_eq!(pom.plugins[0].group_id, "org.apache.maven.plugins");
    }

    #[test]
    fn the_multi_module_root_lists_its_modules_and_dependency_management() {
        let pom = parse(MULTI_ROOT_POM).expect("valid");
        assert_eq!(pom.packaging, "pom");
        assert_eq!(pom.modules, vec!["lib", "app"]);
        assert_eq!(pom.dependency_management.len(), 1);
        assert_eq!(pom.dependency_management[0].artifact_id, "junit-jupiter");
        assert_eq!(
            pom.dependency_management[0].version.as_deref(),
            Some("5.10.3")
        );
    }

    #[test]
    fn a_child_pom_inherits_group_id_and_version_from_its_parent_declaration() {
        let pom = parse(MULTI_LIB_POM).expect("valid");
        // lib/pom.xml declares neither groupId nor version of its own —
        // both come from <parent>, the one inheritance step this reader
        // does resolve without opening a second file.
        assert_eq!(pom.group_id.as_deref(), Some("com.example"));
        assert_eq!(pom.version.as_deref(), Some("1.0.0"));
        assert_eq!(pom.artifact_id, "lib");
        assert_eq!(pom.parent.as_ref().unwrap().artifact_id, "maven-multi");
    }

    #[test]
    fn a_project_version_self_reference_resolves_to_this_poms_own_version() {
        let pom = parse(MULTI_APP_POM).expect("valid");
        let lib_dep = pom
            .dependencies
            .iter()
            .find(|d| d.artifact_id == "lib")
            .expect("lib dependency");
        // app/pom.xml spells this <version>${project.version}</version>;
        // app has no <version> of its own, so this also proves the
        // self-ref sees the *inherited* version, not an empty string.
        assert_eq!(lib_dep.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn a_dependency_with_no_version_relies_on_dependency_management() {
        let pom = parse(MULTI_APP_POM).expect("valid");
        let junit_dep = pom
            .dependencies
            .iter()
            .find(|d| d.artifact_id == "junit-jupiter")
            .expect("junit dependency");
        assert_eq!(junit_dep.version, None);
    }

    #[test]
    fn an_unresolvable_token_is_left_visible_rather_than_emptied() {
        let pom = parse(
            r#"<project>
                <groupId>g</groupId><artifactId>a</artifactId><version>1</version>
                <dependencies>
                    <dependency>
                        <groupId>g2</groupId>
                        <artifactId>a2</artifactId>
                        <version>${not.declared.anywhere}</version>
                    </dependency>
                </dependencies>
            </project>"#,
        )
        .expect("valid");
        assert_eq!(
            pom.dependencies[0].version.as_deref(),
            Some("${not.declared.anywhere}")
        );
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        assert!(parse("not xml at all").is_err());
    }
}
