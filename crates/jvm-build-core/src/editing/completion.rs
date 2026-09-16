//! Completion ranking (D5): turns a raw candidate list (group ids,
//! artifact ids, or versions — gathered by the caller from D3's
//! `RepoIndex` and/or D4's `CentralClient`) into ranked, deduplicated
//! [`Completion`] items for whichever coordinate part [`EditContext`]
//! says the caret is inside.
//!
//! Deliberately provider-agnostic: this module knows nothing about
//! `RepoIndex` or `CentralClient`, only strings. That is what makes the
//! plan's two-phase delivery (local index synchronously, Central's answer
//! merged in once the network call returns) a matter of calling this
//! function twice with a longer candidate list the second time, rather
//! than this module owning any notion of "still waiting on the network" —
//! that state belongs to whichever caller is doing the waiting (the
//! bridge's `CompletionTracker`, ADR-agnostic to this crate).
//!
//! Ranking: an exact-prefix match beats a mere substring match (both
//! case-insensitive); within a tier, alphabetical. A `Version` part
//! ignores that tiering and sorts newest-first instead
//! ([`super::version_order::compare_descending`]) — nobody wants
//! `4.0.0-alpha` offered above `3.2.1` just because it starts with the
//! text typed so far.

use std::collections::BTreeSet;
use std::ops::Range;

use super::context::{CoordinatePart, EditContext};
use super::version_order::compare_descending;

/// One ranked completion candidate — this crate's own type. The bridge
/// (D5's `ui-shell` half) maps this to `lsp_core::CompletionItem`, never
/// the reverse: `jvm-build-core` must not depend on `lsp-core` (the
/// plan's core constraint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// What accepting this item types.
    pub insert_text: String,
    /// What the popup shows. Equal to `insert_text` here — none of these
    /// three formats has a separate display form (no snippets, no
    /// parameter hints).
    pub label: String,
    /// The span this item's insertion replaces — exactly the coordinate
    /// part's own value range from [`EditContext`], never the whole
    /// literal it lives in.
    pub range: Range<usize>,
}

fn context_range(ctx: &EditContext) -> Range<usize> {
    match ctx {
        EditContext::PomCoordinate { range, .. }
        | EditContext::GradleCoordinate { range, .. }
        | EditContext::GradlePluginVersion { range, .. }
        | EditContext::TomlVersion { range, .. }
        | EditContext::TomlLibraryField { range, .. } => range.clone(),
    }
}

/// Is `ctx` a version-*literal* context — the one case ranked newest-first
/// instead of by prefix/contains tier?
///
/// `TomlLibraryField::VersionRef` is deliberately excluded even though its
/// name suggests otherwise: `version.ref = "guava"` holds the *key* of a
/// `[versions]` table entry, not a version string — `"guava"` is a name,
/// not a release to sort by recency. Its candidates (every key the
/// `[versions]` table declares) are ranked like any other name.
fn is_version_context(ctx: &EditContext) -> bool {
    match ctx {
        EditContext::PomCoordinate { part, .. } | EditContext::GradleCoordinate { part, .. } => {
            *part == CoordinatePart::Version
        }
        EditContext::GradlePluginVersion { .. } | EditContext::TomlVersion { .. } => true,
        EditContext::TomlLibraryField { .. } => false,
    }
}

/// The trailing run of word characters (alphanumeric or `_`) in `typed` —
/// deliberately the same rule `lsp_core::completion_prefix` uses, but
/// reimplemented here rather than pulled in as a dependency (this crate
/// stays free of `lsp-core`, the plan's core constraint).
///
/// A coordinate literal is not a bare word: `"org.springframework.boot"`,
/// `"spring-boot-starter"`, `"5.10"` all contain `.`/`-` a bridge's own
/// `CompletionTracker` — built for a single identifier-shaped prefix —
/// cannot see past. Passing the *whole* literal to `begin`/`needs_request`
/// stores it as the tracker's remembered prefix; the next keystroke's
/// `still_typing` check then asks "does the new bare word start with the
/// entire dotted literal", which is false for every keystroke after the
/// first dot or hyphen, and the popup silently stays empty forever after.
/// This is the piece of the literal that must go to the tracker instead —
/// [`Completion::range`] (already the whole literal's own segment) is
/// unaffected and still what an accepted item replaces.
pub fn tracker_prefix(typed: &str) -> &str {
    let start = typed
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map(|(i, _)| i)
        .unwrap_or(typed.len());
    &typed[start..]
}

/// Rank `candidates` for the coordinate part `ctx` names, replacing
/// exactly its value range. `text` is the buffer `ctx` was classified
/// against — the already-typed prefix for ranking is read straight out of
/// it via `ctx`'s own range, rather than being threaded through as a
/// separate parameter every caller would otherwise have to keep in sync
/// with `ctx`.
///
/// Call this once with the local index's candidates for an immediate
/// popup, then again with the local and Central candidates merged once
/// the network answer arrives — the bridge's `CompletionTracker` decides
/// whether that second call's answer is still wanted (the caret may have
/// moved on), this function does not need to know.
pub fn items(ctx: &EditContext, text: &str, candidates: &[String]) -> Vec<Completion> {
    let range = context_range(ctx);
    let typed = text.get(range.clone()).unwrap_or_default();
    let ranked = rank(candidates, typed, is_version_context(ctx));
    ranked
        .into_iter()
        .map(|value| Completion {
            insert_text: value.clone(),
            label: value,
            range: range.clone(),
        })
        .collect()
}

/// Tier 0: case-insensitive prefix match. Tier 1: contains but is not a
/// prefix match. Anything matching neither is dropped — Ctrl+Space should
/// not offer a coordinate with no relation at all to what is typed.
fn score(candidate: &str, typed_lower: &str) -> Option<u8> {
    if typed_lower.is_empty() {
        return Some(1);
    }
    let candidate_lower = candidate.to_lowercase();
    if candidate_lower.starts_with(typed_lower) {
        Some(0)
    } else if candidate_lower.contains(typed_lower) {
        Some(1)
    } else {
        None
    }
}

fn rank(candidates: &[String], typed: &str, is_version: bool) -> Vec<String> {
    let typed_lower = typed.to_lowercase();
    let mut seen = BTreeSet::new();
    let mut scored: Vec<(u8, String)> = candidates
        .iter()
        .filter(|c| seen.insert(c.as_str()))
        .filter_map(|c| score(c, &typed_lower).map(|s| (s, c.clone())))
        .collect();

    if is_version {
        scored.sort_by(|(_, a), (_, b)| compare_descending(a, b));
    } else {
        scored.sort_by(|(sa, a), (sb, b)| sa.cmp(sb).then_with(|| a.cmp(b)));
    }
    scored.into_iter().map(|(_, value)| value).collect()
}

#[cfg(test)]
mod tests {
    use super::super::context::{Coordinate, TomlLibraryEntry, TomlLibraryField};
    use super::*;

    fn candidates(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn exact_prefix_ranks_above_a_mere_contains_match() {
        let text = "implementation(\"com.example:lib:1.0\")";
        let artifact_start = text.find("lib").unwrap();
        let ctx = EditContext::GradleCoordinate {
            part: CoordinatePart::ArtifactId,
            range: artifact_start..(artifact_start + 3),
            coordinate: Coordinate {
                group_id: Some("com.example".to_string()),
                artifact_id: Some("lib".to_string()),
                version: Some("1.0".to_string()),
            },
            configuration: "implementation".to_string(),
        };
        let items = items(
            &ctx,
            text,
            &candidates(&["sublib", "libcore", "lib-extras"]),
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // "libcore" and "lib-extras" start with "lib"; "sublib" only
        // contains it.
        assert_eq!(labels, vec!["lib-extras", "libcore", "sublib"]);
    }

    #[test]
    fn candidates_matching_neither_prefix_nor_contains_are_dropped() {
        let text = "artifactId";
        let ctx = pom_artifact_ctx(0..text.len(), "artifactId");
        let items = items(&ctx, text, &candidates(&["unrelated", "artifactIdExtra"]));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "artifactIdExtra");
    }

    #[test]
    fn version_context_ranks_newest_stable_first_regardless_of_prefix_tier() {
        let text = "1";
        let ctx = pom_version_ctx(text, 0..text.len());
        let items = items(
            &ctx,
            text,
            &candidates(&["1.0.0", "1.5.0", "1.5.0-alpha", "2.0.0"]),
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // "2.0.0" does not start with "1" so it is filtered out entirely —
        // ranking is still scoped to what was typed, only the *order*
        // within the surviving set is newest-first rather than tiered.
        // Both stable releases sort ahead of the pre-release regardless of
        // numeric magnitude.
        assert_eq!(labels, vec!["1.5.0", "1.0.0", "1.5.0-alpha"]);
    }

    #[test]
    fn duplicate_candidates_collapse_to_one_item() {
        let text = "g";
        let ctx = pom_group_ctx(text, 0..text.len());
        let items = items(&ctx, text, &candidates(&["guava", "guava", "guava"]));
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn an_empty_typed_prefix_keeps_every_candidate() {
        let text = "";
        let ctx = pom_group_ctx(text, 0..0);
        let items = items(&ctx, text, &candidates(&["com.example", "org.other"]));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn toml_version_ref_field_ranks_by_name_not_by_version_order() {
        // `version.ref` holds a `[versions]` table *key* ("guava"), not a
        // version string — candidates are names, ranked like any other
        // prefix/contains match, never newest-first.
        let text = "gua";
        let ctx = EditContext::TomlLibraryField {
            field: TomlLibraryField::VersionRef,
            range: 0..text.len(),
            entry: TomlLibraryEntry::default(),
        };
        let items = items(&ctx, text, &candidates(&["guava", "languava", "okhttp"]));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["guava", "languava"]);
    }

    fn pom_group_ctx(text: &str, range: Range<usize>) -> EditContext {
        EditContext::PomCoordinate {
            part: CoordinatePart::GroupId,
            range,
            coordinate: Coordinate {
                group_id: Some(text.to_string()),
                artifact_id: None,
                version: None,
            },
        }
    }

    fn pom_artifact_ctx(range: Range<usize>, typed: &str) -> EditContext {
        EditContext::PomCoordinate {
            part: CoordinatePart::ArtifactId,
            range,
            coordinate: Coordinate {
                group_id: None,
                artifact_id: Some(typed.to_string()),
                version: None,
            },
        }
    }

    fn pom_version_ctx(text: &str, range: Range<usize>) -> EditContext {
        EditContext::PomCoordinate {
            part: CoordinatePart::Version,
            range,
            coordinate: Coordinate {
                group_id: None,
                artifact_id: None,
                version: Some(text.to_string()),
            },
        }
    }

    #[test]
    fn tracker_prefix_stops_at_a_dot() {
        assert_eq!(tracker_prefix("org.spring"), "spring");
    }

    #[test]
    fn tracker_prefix_stops_at_a_hyphen() {
        assert_eq!(tracker_prefix("spring-boot"), "boot");
    }

    #[test]
    fn tracker_prefix_stops_at_a_dot_in_a_version() {
        assert_eq!(tracker_prefix("5.10"), "10");
    }

    #[test]
    fn tracker_prefix_of_a_bare_word_is_the_whole_word() {
        assert_eq!(tracker_prefix("guava"), "guava");
    }

    #[test]
    fn tracker_prefix_of_an_empty_literal_is_empty() {
        assert_eq!(tracker_prefix(""), "");
    }
}
