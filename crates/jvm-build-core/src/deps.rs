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
/// D2's scanner finds no matching `group:artifact` — the ordinary case
/// for a transitive dependency, which never appears in the build file at
/// all, only in the resolved graph.
pub fn declaration_site(dep: &Dependency, path: &Path, build_file_text: &str) -> Option<u32> {
    let declared = context::declared_versions(path, build_file_text);
    let found = declared
        .iter()
        .find(|d| d.group_id == dep.group && d.artifact_id == dep.artifact)?;
    Some(line_of(build_file_text, found.range.start))
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
}
