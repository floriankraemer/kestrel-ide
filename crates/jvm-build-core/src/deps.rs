//! The dependency analyzer (D8): the Build Tools dock's Dependencies
//! subtree's scope/configuration filter and "Conflicts only" toggle
//! ([`filter`]), and "Go to declaration" jump-target resolution
//! ([`declaration_site`]) — both pure, reusing what a synced
//! [`crate::model::Dependency`] and D2's build-file scanner already carry
//! rather than re-deriving anything. State (which filter is active) lives
//! in the bridge (B1's `BuildToolsService`), the same place B2's own
//! view-state lives; this module is only the rule.

use std::path::Path;

use crate::editing::context;
use crate::model::Dependency;

/// Which of a `Dependency` list to show. `scope: None` means "every
/// scope" — the dock combo's starting state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyFilter {
    /// A configuration/scope name to match exactly (Gradle's
    /// `implementation`/`testImplementation`/…, Maven's
    /// `compile`/`test`/…).
    pub scope: Option<String>,
    pub conflicts_only: bool,
}

/// Applies `filter` over `dependencies`, recursing into `children` — the
/// dock's tree stays a tree; filtering removes rows, it never flattens
/// one. A dependency that does not itself match is kept anyway when one
/// of its children does, so a matching transitive dependency stays
/// reachable under its non-matching parent rather than being orphaned.
pub fn filter(dependencies: &[Dependency], filter: &DependencyFilter) -> Vec<Dependency> {
    dependencies
        .iter()
        .filter_map(|dep| filter_one(dep, filter))
        .collect()
}

fn filter_one(dep: &Dependency, filter: &DependencyFilter) -> Option<Dependency> {
    let children: Vec<Dependency> = dep
        .children
        .iter()
        .filter_map(|child| filter_one(child, filter))
        .collect();
    let matches_here = filter
        .scope
        .as_deref()
        .is_none_or(|scope| dep.scope == scope)
        && (!filter.conflicts_only || dep.conflict.is_some());
    if matches_here || !children.is_empty() {
        let mut kept = dep.clone();
        kept.children = children;
        Some(kept)
    } else {
        None
    }
}

/// The 1-based line `dep` is declared on in `build_file_text` (the file
/// named by `path` — `dep.file` when a caller has a `Dependency` from a
/// synced model, which is where this is normally read from). `None` when
/// D2's scanner finds no matching `group:artifact` at all — the ordinary
/// case for a transitive dependency, which never appears in the build
/// file itself, only in the resolved graph.
///
/// Two passes (review fix #9): `declared_versions` first, which lands on
/// the version literal itself when there is one; `declaration_range`
/// (broader, no version required) when there is not — a dependency whose
/// version comes from `<dependencyManagement>`/an imported BOM, or a
/// Gradle `platform(...)` constraint, still has a real `group:artifact`
/// declaration to jump to even though nothing here can resolve its
/// version.
pub fn declaration_site(dep: &Dependency, path: &Path, build_file_text: &str) -> Option<u32> {
    let declared = context::declared_versions(path, build_file_text);
    let start = declared
        .iter()
        .find(|d| d.group_id == dep.group && d.artifact_id == dep.artifact)
        .map(|d| d.range.start)
        .or_else(|| {
            context::declaration_range(path, build_file_text, &dep.group, &dep.artifact)
                .map(|r| r.start)
        })?;
    Some(line_of(build_file_text, start))
}

fn line_of(text: &str, byte_offset: usize) -> u32 {
    text.get(..byte_offset)
        .unwrap_or(text)
        .bytes()
        .filter(|&b| b == b'\n')
        .count() as u32
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Conflict;

    fn dep(group: &str, artifact: &str, scope: &str, conflict: Option<Conflict>) -> Dependency {
        Dependency {
            group: group.to_string(),
            artifact: artifact.to_string(),
            requested: "1.0".to_string(),
            resolved: "1.0".to_string(),
            scope: scope.to_string(),
            transitive: false,
            file: None,
            conflict,
            children: Vec::new(),
        }
    }

    #[test]
    fn no_filter_keeps_everything() {
        let deps = vec![
            dep("g", "a", "implementation", None),
            dep("g", "b", "testImplementation", None),
        ];
        assert_eq!(filter(&deps, &DependencyFilter::default()).len(), 2);
    }

    #[test]
    fn scope_filter_keeps_only_the_matching_configuration() {
        let deps = vec![
            dep("g", "a", "implementation", None),
            dep("g", "b", "testImplementation", None),
        ];
        let found = filter(
            &deps,
            &DependencyFilter {
                scope: Some("testImplementation".to_string()),
                conflicts_only: false,
            },
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].artifact, "b");
    }

    #[test]
    fn conflicts_only_drops_rows_with_no_conflict() {
        let deps = vec![
            dep("g", "a", "implementation", None),
            dep(
                "g",
                "b",
                "implementation",
                Some(Conflict::OmittedForConflict {
                    winner: "2.0".to_string(),
                }),
            ),
        ];
        let found = filter(
            &deps,
            &DependencyFilter {
                scope: None,
                conflicts_only: true,
            },
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].artifact, "b");
    }

    #[test]
    fn a_matching_transitive_child_keeps_its_non_matching_parent_reachable() {
        let mut parent = dep("g", "parent", "implementation", None);
        parent.children = vec![dep(
            "g",
            "child",
            "implementation",
            Some(Conflict::OmittedForDuplicate),
        )];
        let found = filter(
            &[parent],
            &DependencyFilter {
                scope: None,
                conflicts_only: true,
            },
        );
        assert_eq!(found.len(), 1, "the parent survives to carry its child");
        assert_eq!(found[0].artifact, "parent");
        assert_eq!(found[0].children.len(), 1);
        assert_eq!(found[0].children[0].artifact, "child");
    }

    #[test]
    fn both_filters_combine_as_an_and() {
        let deps = vec![
            dep("g", "a", "implementation", None),
            dep(
                "g",
                "b",
                "testImplementation",
                Some(Conflict::OmittedForDuplicate),
            ),
            dep(
                "g",
                "c",
                "implementation",
                Some(Conflict::OmittedForDuplicate),
            ),
        ];
        let found = filter(
            &deps,
            &DependencyFilter {
                scope: Some("implementation".to_string()),
                conflicts_only: true,
            },
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].artifact, "c");
    }

    #[test]
    fn declaration_site_finds_the_line_in_a_pom() {
        let text = "<project>\n  <dependencies>\n    <dependency>\n      <groupId>com.google.guava</groupId>\n      <artifactId>guava</artifactId>\n      <version>32.1.3-jre</version>\n    </dependency>\n  </dependencies>\n</project>\n";
        let d = dep("com.google.guava", "guava", "compile", None);
        let line = declaration_site(&d, Path::new("/proj/pom.xml"), text).expect("found");
        // The <version> line — line 6 (1-based).
        assert_eq!(line, 6);
    }

    #[test]
    fn declaration_site_returns_none_for_a_transitive_dependency_not_in_the_file() {
        let text = "<project><dependencies><dependency><groupId>a</groupId><artifactId>a</artifactId><version>1.0</version></dependency></dependencies></project>";
        let d = dep("com.transitive", "nowhere", "compile", None);
        assert_eq!(declaration_site(&d, Path::new("/proj/pom.xml"), text), None);
    }

    /// Review fix #9: a dependency managed by an imported BOM (no
    /// `<version>` next to its own coordinate at all) still has a real
    /// `<artifactId>` declaration to jump to, via `declaration_range`'s
    /// fallback.
    #[test]
    fn declaration_site_falls_back_to_the_artifact_line_for_a_bom_managed_dependency() {
        let text = "<project>\n  <dependencies>\n    <dependency>\n      <groupId>org.springframework.boot</groupId>\n      <artifactId>spring-boot-starter-web</artifactId>\n    </dependency>\n  </dependencies>\n</project>\n";
        let d = dep(
            "org.springframework.boot",
            "spring-boot-starter-web",
            "compile",
            None,
        );
        let line = declaration_site(&d, Path::new("/proj/pom.xml"), text).expect("found");
        // The <artifactId> line — line 5 (1-based).
        assert_eq!(line, 5);
    }

    /// Review fix #9: a two-module POM — the same `group:artifact` is
    /// declared once at each module's own `pom.xml`, and a caller passing
    /// module A's own text must find module A's line, not be confused by
    /// module B's identical dependency existing elsewhere.
    #[test]
    fn declaration_site_scoped_to_the_module_whose_text_was_passed() {
        let module_a = "<project>\n  <dependencies>\n    <dependency>\n      <groupId>com.google.guava</groupId>\n      <artifactId>guava</artifactId>\n      <version>32.1.3-jre</version>\n    </dependency>\n  </dependencies>\n</project>\n";
        let module_b = "<project>\n\n\n\n\n\n  <dependencies>\n    <dependency>\n      <groupId>com.google.guava</groupId>\n      <artifactId>guava</artifactId>\n      <version>33.0.0-jre</version>\n    </dependency>\n  </dependencies>\n</project>\n";
        let d = dep("com.google.guava", "guava", "compile", None);
        assert_eq!(
            declaration_site(&d, Path::new("/proj/app/pom.xml"), module_a),
            Some(6)
        );
        assert_eq!(
            declaration_site(&d, Path::new("/proj/lib/pom.xml"), module_b),
            Some(11)
        );
    }

    /// Review fix #9: a Gradle dependency managed through a version
    /// catalog / platform BOM (no literal version segment at all) also
    /// falls back to the coordinate's own line.
    #[test]
    fn declaration_site_finds_the_line_in_gradle() {
        let text = "dependencies {\n    implementation(\"com.google.guava:guava:32.1.3-jre\")\n}\n";
        let d = dep("com.google.guava", "guava", "implementation", None);
        let line = declaration_site(&d, Path::new("/proj/build.gradle.kts"), text).expect("found");
        assert_eq!(line, 2);
    }
}
