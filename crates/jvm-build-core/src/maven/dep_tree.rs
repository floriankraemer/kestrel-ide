//! Parsing `mvn org.apache.maven.plugins:maven-dependency-plugin:3.6.1:tree
//! -Dverbose`'s text output (A5) — pinned exactly, per the plan: `-Dverbose`
//! was a documented no-op on the dependency plugin's 3.0-3.1 releases, so an
//! unpinned invocation could silently stop reporting conflicts at all.
//!
//! The format has been stable for years and is deliberately *not* parsed
//! structurally (there is no `--output-format=json` for this goal): each
//! line is `[INFO] ` plus an ASCII tree drawn from `+- `/`\- `/`|  `/`   `
//! prefixes, then a Maven coordinate, optionally wrapped in parentheses
//! with a trailing ` - <reason>` when the artifact was not actually
//! resolved at that position in the tree.

use crate::model::Conflict;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepTreeNode {
    pub group_id: String,
    pub artifact_id: String,
    pub packaging: String,
    pub classifier: Option<String>,
    pub version: String,
    pub scope: String,
    /// How deep this line was drawn — 0 is the project's own root
    /// coordinate, 1 its direct dependencies, and so on. What a sibling
    /// vs. a child line is, is exactly this depth compared to the
    /// previous line's.
    pub depth: usize,
    pub conflict: Option<Conflict>,
}

/// Parse every line of a verbose `dependency:tree` run into a flat,
/// depth-tagged list — the shape [`build_children`] turns into the nested
/// form [`crate::model::Dependency::children`] expects.
pub fn parse(text: &str) -> Vec<DepTreeNode> {
    text.lines().filter_map(parse_line).collect()
}

/// Rebuild the nested `children` shape from [`parse`]'s flat, depth-tagged
/// list — every node whose depth is exactly one more than its immediately
/// preceding shallower-or-equal sibling is that sibling's child.
pub fn build_children(nodes: &[DepTreeNode]) -> Vec<crate::model::Dependency> {
    fn build(
        nodes: &[DepTreeNode],
        index: &mut usize,
        depth: usize,
    ) -> Vec<crate::model::Dependency> {
        let mut result = Vec::new();
        while *index < nodes.len() && nodes[*index].depth == depth {
            let node = &nodes[*index];
            *index += 1;
            let children = if *index < nodes.len() && nodes[*index].depth == depth + 1 {
                build(nodes, index, depth + 1)
            } else {
                Vec::new()
            };
            result.push(crate::model::Dependency {
                group: node.group_id.clone(),
                artifact: node.artifact_id.clone(),
                requested: node.version.clone(),
                resolved: node.version.clone(),
                scope: node.scope.clone(),
                transitive: depth > 1,
                file: None,
                conflict: node.conflict.clone(),
                children,
            });
        }
        result
    }
    let mut index = 0;
    // Depth 0 is the project's own coordinate line, never a dependency in
    // the model sense; its direct dependencies start at depth 1.
    if nodes.first().is_some_and(|n| n.depth == 0) {
        index = 1;
    }
    build(nodes, &mut index, 1)
}

fn parse_line(line: &str) -> Option<DepTreeNode> {
    let rest = line.strip_prefix("[INFO] ")?;
    if rest.trim().is_empty() {
        return None;
    }
    let (depth, coordinate_part) = strip_tree_prefix(rest);
    let (coordinate, conflict) = split_conflict(coordinate_part);
    parse_coordinate(coordinate, depth, conflict)
}

/// A tree line's prefix is drawn in fixed-width, 3-character units: `+- `
/// or `\- ` marks the line's own branch, and each ancestor still "open" to
/// the right of it is either `|  ` (a sibling follows) or `   ` (it was the
/// last child). Depth is simply how many such units precede the artifact
/// text — the root coordinate itself (depth 0) has no prefix at all.
fn strip_tree_prefix(rest: &str) -> (usize, &str) {
    let mut depth = 0;
    let mut remaining = rest;
    loop {
        if let Some(next) = remaining
            .strip_prefix("+- ")
            .or_else(|| remaining.strip_prefix("\\- "))
        {
            depth += 1;
            return (depth, next);
        }
        if let Some(next) = remaining
            .strip_prefix("|  ")
            .or_else(|| remaining.strip_prefix("   "))
        {
            depth += 1;
            remaining = next;
            continue;
        }
        break;
    }
    (depth, remaining)
}

/// A plain coordinate stays as-is. One wrapped in parentheses, with a
/// trailing `" - <reason>"`, has its parentheses stripped and the reason
/// text returned separately.
fn split_conflict(text: &str) -> (&str, Option<&str>) {
    let Some(inner) = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')) else {
        return (text, None);
    };
    match inner.split_once(" - ") {
        Some((coordinate, reason)) => (coordinate, Some(reason)),
        None => (inner, None),
    }
}

fn parse_coordinate(text: &str, depth: usize, reason: Option<&str>) -> Option<DepTreeNode> {
    let parts: Vec<&str> = text.split(':').collect();
    // The tree's own root line (depth 0, the project's own coordinate) has
    // no scope: `group:artifact:packaging:version`. Every dependency line
    // does: `group:artifact:packaging:version:scope`, or with a classifier,
    // `group:artifact:packaging:classifier:version:scope`.
    let (group_id, artifact_id, packaging, classifier, version, scope) = match parts.as_slice() {
        [g, a, p, v] => (*g, *a, *p, None, *v, ""),
        [g, a, p, v, s] => (*g, *a, *p, None, *v, *s),
        [g, a, p, c, v, s] => (*g, *a, *p, Some(*c), *v, *s),
        _ => return None,
    };
    Some(DepTreeNode {
        group_id: group_id.to_string(),
        artifact_id: artifact_id.to_string(),
        packaging: packaging.to_string(),
        classifier: classifier.map(str::to_string),
        version: version.to_string(),
        scope: scope.to_string(),
        depth,
        conflict: reason.map(to_conflict),
    })
}

fn to_conflict(reason: &str) -> Conflict {
    if let Some(winner) = reason.strip_prefix("omitted for conflict with ") {
        Conflict::OmittedForConflict {
            winner: winner.to_string(),
        }
    } else if reason == "omitted for duplicate" {
        Conflict::OmittedForDuplicate
    } else if let Some(from) = reason.strip_prefix("version managed from ") {
        Conflict::VersionManagedFrom {
            from: from.to_string(),
        }
    } else {
        // A reason text this build has not seen yet — kept as a conflict
        // with no more specific classification than "the tool flagged
        // this artifact", the same "note, never a guess" rule
        // build-core's severity table follows for a word it does not
        // recognise.
        Conflict::OmittedForDuplicate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real capture (`mvn -B org.apache.maven.plugins:maven-dependency-plugin:3.6.1:tree
    /// -Dverbose`, inside linux-jvm, against tests/fixtures/maven-single) —
    /// A4/A5's "verified against the real tool" rule applies to text
    /// parsers too, not only JSON/XML ones.
    const REAL_CAPTURE: &str = "\
[INFO] com.example:maven-single:jar:1.0.0
[INFO] \\- org.junit.jupiter:junit-jupiter:jar:5.10.3:test
[INFO]    +- org.junit.jupiter:junit-jupiter-api:jar:5.10.3:test
[INFO]    |  +- org.opentest4j:opentest4j:jar:1.3.0:test
[INFO]    |  +- org.junit.platform:junit-platform-commons:jar:1.10.3:test
[INFO]    |  |  \\- (org.apiguardian:apiguardian-api:jar:1.1.2:test - omitted for duplicate)
[INFO]    |  \\- org.apiguardian:apiguardian-api:jar:1.1.2:test
[INFO]    +- org.junit.jupiter:junit-jupiter-params:jar:5.10.3:test
[INFO]    |  +- (org.junit.jupiter:junit-jupiter-api:jar:5.10.3:test - omitted for duplicate)
[INFO]    |  \\- (org.apiguardian:apiguardian-api:jar:1.1.2:test - omitted for duplicate)
[INFO]    \\- org.junit.jupiter:junit-jupiter-engine:jar:5.10.3:test
[INFO]       +- org.junit.platform:junit-platform-engine:jar:1.10.3:test
[INFO]       |  +- (org.opentest4j:opentest4j:jar:1.3.0:test - omitted for duplicate)
[INFO]       |  +- (org.junit.platform:junit-platform-commons:jar:1.10.3:test - omitted for duplicate)
[INFO]       |  \\- (org.apiguardian:apiguardian-api:jar:1.1.2:test - omitted for duplicate)
[INFO]       +- (org.junit.jupiter:junit-jupiter-api:jar:5.10.3:test - omitted for duplicate)
[INFO]       \\- (org.apiguardian:apiguardian-api:jar:1.1.2:test - omitted for duplicate)
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
";

    #[test]
    fn the_real_capture_parses_the_root_and_every_dependency() {
        let nodes = parse(REAL_CAPTURE);
        assert_eq!(nodes[0].depth, 0);
        assert_eq!(nodes[0].artifact_id, "maven-single");
        assert_eq!(nodes[1].depth, 1);
        assert_eq!(nodes[1].artifact_id, "junit-jupiter");
        assert_eq!(nodes[1].version, "5.10.3");
        assert_eq!(nodes[1].scope, "test");
    }

    #[test]
    fn an_omitted_for_duplicate_line_is_flagged() {
        let nodes = parse(REAL_CAPTURE);
        let opentest4j_dup = nodes
            .iter()
            .find(|n| n.artifact_id == "junit-platform-commons" && n.depth == 3)
            .expect("junit-platform-commons at depth 3");
        // Its own child, apiguardian-api, is the omitted-for-duplicate one.
        let apiguardian = nodes
            .iter()
            .filter(|n| n.artifact_id == "apiguardian-api")
            .find(|n| n.depth == opentest4j_dup.depth + 1)
            .expect("apiguardian-api under junit-platform-commons");
        assert_eq!(apiguardian.conflict, Some(Conflict::OmittedForDuplicate));
    }

    #[test]
    fn a_non_flagged_line_has_no_conflict() {
        let nodes = parse(REAL_CAPTURE);
        assert_eq!(nodes[1].conflict, None);
    }

    #[test]
    fn build_children_nests_the_flat_list_back_into_a_tree() {
        let nodes = parse(REAL_CAPTURE);
        let children = build_children(&nodes);
        assert_eq!(children.len(), 1, "one direct dependency: junit-jupiter");
        assert_eq!(children[0].artifact, "junit-jupiter");
        assert_eq!(children[0].children.len(), 3, "api, params, engine");
    }

    #[test]
    fn a_hand_authored_conflict_with_line_is_recognised() {
        // "omitted for conflict with X" needs a real multi-version
        // conflict to trigger, which the fixture projects deliberately
        // avoid (a conflict is exactly the confusing state a fixture
        // should not accidentally depend on for its own build to
        // succeed) — the parsing rule itself is unit-tested directly
        // against Maven's own documented wording instead.
        let line =
            "[INFO]    +- (com.example:widget:jar:2.0:compile - omitted for conflict with 1.0)";
        let nodes = parse(line);
        assert_eq!(
            nodes[0].conflict,
            Some(Conflict::OmittedForConflict {
                winner: "1.0".to_string()
            })
        );
    }

    #[test]
    fn a_hand_authored_version_managed_from_line_is_recognised() {
        let line = "[INFO]    +- (com.example:widget:jar:2.0:compile - version managed from 1.5)";
        let nodes = parse(line);
        assert_eq!(
            nodes[0].conflict,
            Some(Conflict::VersionManagedFrom {
                from: "1.5".to_string()
            })
        );
    }

    #[test]
    fn a_classifier_coordinate_is_parsed() {
        let line = "[INFO] com.example:widget:jar:tests:1.0:test";
        let nodes = parse(line);
        assert_eq!(nodes[0].classifier.as_deref(), Some("tests"));
        assert_eq!(nodes[0].version, "1.0");
    }

    #[test]
    fn non_tree_lines_are_ignored() {
        let text = "[INFO] Scanning for projects...\n[INFO] BUILD SUCCESS\n";
        assert!(parse(text).is_empty());
    }
}
