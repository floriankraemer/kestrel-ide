//! A pragmatic version comparator (D5's newest-first completion ranking,
//! D6's "latest stable" hint) — deliberately not a strict semver parser.
//! Maven and Gradle coordinates routinely use shapes semver rejects
//! outright (`5.10.0.RELEASE`, `32.1.3-jre`, `1.0.0.Final`), so this is a
//! Debian/Gradle-style segment comparator instead: split on `.`/`-`/`_`/
//! `+`, compare numeric segments numerically and text segments lexically,
//! and sort a stable release ahead of a pre-release with the same prefix.

use std::cmp::Ordering;

/// The plan's own list of pre-release markers, checked whole-segment
/// (case-insensitive) so `1.0.0-createuser` — a real internal artifact
/// might use that as a qualifier — does not false-positive on `create`
/// merely containing no marker substring; only an exact segment match
/// (`rc`/`m`/`alpha`/… optionally followed by digits) counts.
fn is_marker_segment(segment: &str) -> bool {
    let lower = segment.to_lowercase();
    let (letters, digits) = split_trailing_digits(&lower);
    if !digits.is_empty() && letters.is_empty() {
        return false;
    }
    match letters {
        "alpha" | "beta" | "snapshot" | "preview" | "ea" => true,
        "rc" | "m" => !digits.is_empty() || letters == "rc",
        _ => false,
    }
}

/// Splits `s` into its trailing run of ASCII digits and everything before
/// it, e.g. `"rc1"` -> `("rc", "1")`, `"alpha"` -> `("alpha", "")`.
fn split_trailing_digits(s: &str) -> (&str, &str) {
    let split_at = s
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(0);
    s.split_at(split_at)
}

fn segments(version: &str) -> Vec<&str> {
    version
        .split(['.', '-', '_', '+'])
        .filter(|s| !s.is_empty())
        .collect()
}

/// Does `version` look like a pre-release/qualifier build rather than a
/// stable release?
pub fn is_pre_release(version: &str) -> bool {
    segments(version).iter().any(|s| is_marker_segment(s))
}

/// A segment, tokenized once: `Text` sorts below `Num` at the same
/// position (declaration order drives the derived `Ord`), so `"1.0.rc"`
/// sorts below `"1.0.1"` even though `Ord` never compares a `Text` to a
/// `Num`'s value directly.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Token {
    Text(String),
    Num(u64),
}

fn tokens(version: &str) -> Vec<Token> {
    segments(version)
        .into_iter()
        .map(|s| match s.parse::<u64>() {
            Ok(n) => Token::Num(n),
            Err(_) => Token::Text(s.to_lowercase()),
        })
        .collect()
}

/// `(stable-before-prerelease, tokens)` — deriving `Ord` on the tuple
/// gives "stable beats pre-release" priority over the token comparison
/// for free, and `Vec<Token>`'s derived `Ord` already implements
/// "a longer version with a matching prefix sorts higher" (`1.0.1` above
/// `1.0`) the same way slice comparison always has.
fn key(version: &str) -> (bool, Vec<Token>) {
    (!is_pre_release(version), tokens(version))
}

/// Orders `a` against `b` newest-first — usable directly as
/// `slice::sort_by(|a, b| compare_descending(a, b))`, where the entry
/// with the higher [`key`] comes first.
///
/// ponytail: this is a heuristic comparator, not a registry-verified
/// semver order — two coordinates with genuinely incomparable versioning
/// schemes (`2023.1` vs `9.0`, a calendar-versioned artifact next to a
/// semver one) will not always agree with the artifact's own release
/// history. Upgrade path if that ever matters: a per-artifact scheme
/// hint, not a smarter general comparator.
pub fn compare_descending(a: &str, b: &str) -> Ordering {
    key(b).cmp(&key(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_dotted_versions_compare_numerically_not_lexically() {
        // Lexical comparison would put "9" ahead of "10".
        let mut versions = vec!["1.9.0", "1.10.0", "1.2.0"];
        versions.sort_by(|a, b| compare_descending(a, b));
        assert_eq!(versions, vec!["1.10.0", "1.9.0", "1.2.0"]);
    }

    #[test]
    fn a_stable_release_sorts_ahead_of_a_pre_release_with_the_same_prefix() {
        let mut versions = ["2.0.0-rc1", "2.0.0", "2.0.0-alpha"];
        versions.sort_by(|a, b| compare_descending(a, b));
        assert_eq!(versions[0], "2.0.0");
    }

    #[test]
    fn a_longer_version_with_a_matching_prefix_sorts_above_the_shorter_one() {
        let mut versions = vec!["1.0", "1.0.1"];
        versions.sort_by(|a, b| compare_descending(a, b));
        assert_eq!(versions, vec!["1.0.1", "1.0"]);
    }

    #[test]
    fn snapshot_milestone_and_release_candidate_markers_are_recognised() {
        for marker in ["SNAPSHOT", "M1", "M2", "rc1", "RC", "alpha", "beta", "ea"] {
            assert!(
                is_pre_release(&format!("1.0.0-{marker}")),
                "{marker} should be a pre-release marker"
            );
        }
    }

    #[test]
    fn ordinary_qualifiers_are_not_pre_release_markers() {
        // A real Spring Boot / Maven convention: neither is a pre-release.
        assert!(!is_pre_release("5.10.0.RELEASE"));
        assert!(!is_pre_release("32.1.3-jre"));
        assert!(!is_pre_release("1.0.0.Final"));
    }

    #[test]
    fn a_version_ending_in_bare_digits_is_not_mistaken_for_a_milestone() {
        // "m2" the marker vs a coincidental trailing "2" after some other
        // letter run — only an exact "m<digits>" or "rc[<digits>]" segment
        // counts.
        assert!(!is_pre_release("1.0.custom2"));
    }
}
