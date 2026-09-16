//! "Newer version available" hints (D6): for a declared dependency
//! ([`super::context::DeclaredVersion`]) and the candidate versions
//! available for it (D3's local index, D4's Maven Central client when
//! online — assembled by the caller, this module stays provider-agnostic
//! the same way [`super::completion`] does), works out whether a newer
//! *stable* release exists.
//!
//! Pre-release versions never count as "newer": nobody wants a hint
//! nagging them toward `4.0.0-alpha` over a perfectly good `3.2.1`. The
//! marker list is [`super::version_order::is_pre_release`]'s own — same
//! rule, same place, checked once.

use super::context::DeclaredVersion;
use super::version_order::{compare_descending, is_pre_release};
use std::cmp::Ordering;
use std::ops::Range;

/// One "newer version available" hint, ready to become a `Diagnostic`
/// (severity Hint/Info, source `build-tools:versions`) and, from D7, a
/// synthesised "Update to `latest`" quick fix over `range`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionHint {
    pub range: Range<usize>,
    pub current: String,
    pub latest: String,
}

impl VersionHint {
    /// `request_intentions` (D7) and the diagnostics store both want this
    /// exact sentence — one place to write it so the popup and the
    /// Problems row never drift apart.
    pub fn message(&self) -> String {
        format!(
            "Newer version {} available (current {})",
            self.latest, self.current
        )
    }
}

/// The newest *stable* version in `candidates`, if any — pre-releases are
/// filtered out entirely, not merely ranked last, since this answers "is
/// there a real upgrade", not "what exists".
fn latest_stable(candidates: &[String]) -> Option<String> {
    candidates
        .iter()
        .filter(|v| !is_pre_release(v))
        .min_by(|a, b| compare_descending(a, b))
        .cloned()
}

/// Does `declared` have a newer stable release among `candidates`? `None`
/// when there is nothing newer (including when `candidates` is empty, or
/// the newest stable entry is `declared.current` itself or older than
/// it — a local/Central listing is not guaranteed sorted, and a stale
/// disk cache can echo back exactly what is already declared).
pub fn hint_for(declared: &DeclaredVersion, candidates: &[String]) -> Option<VersionHint> {
    let latest = latest_stable(candidates)?;
    if compare_descending(&latest, &declared.current) != Ordering::Less {
        return None;
    }
    Some(VersionHint {
        range: declared.range.clone(),
        current: declared.current.clone(),
        latest,
    })
}

/// [`hint_for`] over every declared version in one build file.
/// `candidates_for` gathers the local+Central answer for one
/// `(group_id, artifact_id)` — the caller's job, so this module never
/// touches `RepoIndex`/`CentralClient` directly (`jvm-build-core`'s own
/// completion module follows the same split).
pub fn hints(
    declared: &[DeclaredVersion],
    mut candidates_for: impl FnMut(&str, &str) -> Vec<String>,
) -> Vec<VersionHint> {
    declared
        .iter()
        .filter_map(|d| hint_for(d, &candidates_for(&d.group_id, &d.artifact_id)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(current: &str) -> DeclaredVersion {
        DeclaredVersion {
            group_id: "com.google.guava".to_string(),
            artifact_id: "guava".to_string(),
            current: current.to_string(),
            range: 10..20,
        }
    }

    fn versions(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_newer_stable_release_produces_a_hint() {
        let hint = hint_for(
            &declared("32.1.3-jre"),
            &versions(&["32.1.3-jre", "33.0.0-jre"]),
        )
        .expect("hint");
        assert_eq!(hint.latest, "33.0.0-jre");
        assert_eq!(hint.current, "32.1.3-jre");
        assert_eq!(hint.range, 10..20);
        assert_eq!(
            hint.message(),
            "Newer version 33.0.0-jre available (current 32.1.3-jre)"
        );
    }

    #[test]
    fn already_on_the_latest_stable_version_produces_no_hint() {
        assert_eq!(
            hint_for(
                &declared("33.0.0-jre"),
                &versions(&["32.1.3-jre", "33.0.0-jre"])
            ),
            None
        );
    }

    #[test]
    fn a_pre_release_candidate_is_never_offered_as_newer() {
        assert_eq!(
            hint_for(
                &declared("32.1.3-jre"),
                &versions(&["32.1.3-jre", "33.0.0-alpha", "33.0.0-rc1"])
            ),
            None
        );
    }

    #[test]
    fn no_candidates_at_all_produces_no_hint() {
        assert_eq!(hint_for(&declared("32.1.3-jre"), &[]), None);
    }

    #[test]
    fn a_stale_candidate_list_older_than_current_produces_no_hint() {
        // The local index or a stale Central cache entry can echo back an
        // older snapshot than what is actually declared.
        assert_eq!(
            hint_for(
                &declared("33.0.0-jre"),
                &versions(&["30.0.0-jre", "31.0.0-jre"])
            ),
            None
        );
    }

    #[test]
    fn hints_scans_every_declared_version_in_one_pass() {
        let declared = vec![declared("32.1.3-jre"), {
            let mut other = declared("1.0.0");
            other.artifact_id = "other".to_string();
            other.range = 30..40;
            other
        }];
        let found = hints(&declared, |_, artifact| {
            if artifact == "guava" {
                versions(&["33.0.0-jre"])
            } else {
                versions(&["1.0.0"]) // no newer release for "other"
            }
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].range, 10..20);
    }
}
