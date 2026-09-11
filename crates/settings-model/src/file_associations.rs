//! What a file-association rule means, and which handler applies to a given
//! path (issue #258).
//!
//! `app-config` stores `[[file_associations.rule]]` as bare
//! `{pattern, handler}` strings and does not interpret either (ADR-0017).
//! This module owns the vocabulary: what `handler` names are valid
//! ([`HandlerKind`]), what a `pattern` matches ([`matches_pattern`]), and the
//! precedence a path resolves through — the *effective* settings' own
//! rules first (project overrides, wholesale, the same area-override rule
//! `editing`'s section follows), then this build's shipped defaults.
//!
//! [`HandlerKind`] is deliberately not the same type as `app_core::TabKind`:
//! this is "how the user asked a file to be opened", `TabKind` is "which
//! widget the view builds", and the two happen to agree today only because
//! there is no second way to view an image yet. `app-core` may not depend
//! on this crate (it sits below the support layer), so `ui-shell` — which
//! depends on both — maps one onto the other at the seam and decides
//! nothing else, the same split `FileOp`/`ResourceOp` already use (ADR-0029).

use std::path::Path;

use app_config::Settings;

/// What a resolved file-association rule opens a file as.
///
/// Deliberately not a `bool` ("is this an image") even though only `Image`
/// is user-facing today: a settings string round-trips as a name, and a
/// future handler (e.g. an `ImageEditor` that opens the same file for
/// editing rather than viewing) is a new match arm here, not a reshaped
/// caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerKind {
    /// An editable text document — the same tab a file with no rule at all
    /// gets when it doesn't sniff as binary.
    Text,
    /// A read-only image view (issue #258).
    Image,
    /// A read-only hex view — the same tab a file with no rule at all gets
    /// when it does sniff as binary.
    Binary,
}

impl HandlerKind {
    /// The name a `[[file_associations.rule]]` entry's `handler` field
    /// stores. Stable — it is what a user (or a shipped default) writes.
    fn name(self) -> &'static str {
        match self {
            HandlerKind::Text => "text",
            HandlerKind::Image => "image",
            HandlerKind::Binary => "binary",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "text" => Some(HandlerKind::Text),
            "image" => Some(HandlerKind::Image),
            "binary" => Some(HandlerKind::Binary),
            _ => None,
        }
    }
}

/// Shipped defaults, checked when neither the project nor the global layer
/// names a rule matching the path. Image formats only, per issue #258 —
/// widening this list is additive and does not touch the resolver.
const BUILTIN_RULES: &[(&str, HandlerKind)] = &[
    ("*.png", HandlerKind::Image),
    ("*.jpg", HandlerKind::Image),
    ("*.jpeg", HandlerKind::Image),
    ("*.gif", HandlerKind::Image),
    ("*.bmp", HandlerKind::Image),
    ("*.webp", HandlerKind::Image),
    ("*.svg", HandlerKind::Image),
];

/// The handler `path` resolves to, or `None` when no user rule and no
/// built-in default matches — the caller's cue to fall back to its own
/// content-sniffing heuristic (`app_core::AppSession::open_file`'s binary
/// sniff), exactly the behaviour a file with no association had before this
/// resolver existed.
///
/// `settings` is expected to be the *effective* settings
/// ([`crate::scope::resolve`]'s output): a project's rules already replace
/// the global ones wholesale when the project has any, so this function
/// only ever reads one rule set, in order, before falling through to the
/// built-in defaults.
pub fn resolve_handler(settings: &Settings, path: &Path) -> Option<HandlerKind> {
    let file_name = path.file_name()?.to_str()?;

    for rule in &settings.file_associations.rules {
        if matches_pattern(&rule.pattern, file_name) {
            if let Some(kind) = HandlerKind::parse(&rule.handler) {
                return Some(kind);
            }
        }
    }
    for (pattern, kind) in BUILTIN_RULES {
        if matches_pattern(pattern, file_name) {
            return Some(*kind);
        }
    }
    None
}

/// A minimal glob: `*` matches any run of characters (including none),
/// every other character matches itself, case-insensitively — enough for
/// the `"*.ext"` and exact-filename patterns every built-in default and
/// every association the settings page writes actually needs.
///
/// ponytail: no `?`, no character classes, no `**`. `globset` (already a
/// dependency two crates over, in `index-core`) is the upgrade if a user
/// ever needs a richer pattern than "by extension" or "by exact name".
fn matches_pattern(pattern: &str, file_name: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let file_name = file_name.to_ascii_lowercase();
    glob_match(pattern.as_bytes(), file_name.as_bytes())
}

fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => {
            // Try consuming zero, then one, then two, ... characters of
            // `text` for the `*` before matching the rest of the pattern.
            (0..=text.len()).any(|skip| glob_match(rest, &text[skip..]))
        }
        Some((&p, rest)) => match text.split_first() {
            Some((&t, tail)) if t == p => glob_match(rest, tail),
            _ => false,
        },
    }
}

/// The handler name a rule stores for a given kind, for the page that lets a
/// user add/edit a rule to write back.
pub fn handler_name(kind: HandlerKind) -> &'static str {
    kind.name()
}

/// Every handler a user may pick on the settings page, in display order.
pub const ALL_HANDLERS: [HandlerKind; 3] =
    [HandlerKind::Text, HandlerKind::Image, HandlerKind::Binary];

#[cfg(test)]
mod tests {
    use super::*;
    use app_config::project_settings::ProjectSettings;
    use app_config::{FileAssociationRule, FileAssociationSettings};

    fn settings_with_rules(rules: Vec<(&str, &str)>) -> Settings {
        Settings {
            file_associations: FileAssociationSettings {
                rules: rules
                    .into_iter()
                    .map(|(pattern, handler)| FileAssociationRule {
                        pattern: pattern.to_string(),
                        handler: handler.to_string(),
                    })
                    .collect(),
            },
            ..Settings::default()
        }
    }

    #[test]
    fn an_unrecognised_extension_falls_through_to_no_rule() {
        assert_eq!(
            resolve_handler(&Settings::default(), Path::new("notes.xyz")),
            None
        );
    }

    #[test]
    fn a_builtin_default_resolves_image_extensions() {
        for name in ["a.png", "a.JPG", "a.jpeg", "a.gif", "a.bmp", "a.webp"] {
            assert_eq!(
                resolve_handler(&Settings::default(), Path::new(name)),
                Some(HandlerKind::Image),
                "{name}"
            );
        }
    }

    #[test]
    fn svg_resolves_to_image_not_binary() {
        // SVG is textual XML and would misclassify under the binary sniff —
        // the whole reason this resolver exists rather than extending that
        // heuristic.
        assert_eq!(
            resolve_handler(&Settings::default(), Path::new("logo.svg")),
            Some(HandlerKind::Image)
        );
    }

    #[test]
    fn a_user_rule_wins_over_the_builtin_default() {
        let settings = settings_with_rules(vec![("*.svg", "text")]);
        assert_eq!(
            resolve_handler(&settings, Path::new("logo.svg")),
            Some(HandlerKind::Text)
        );
    }

    #[test]
    fn user_rules_are_checked_in_order_first_match_wins() {
        let settings = settings_with_rules(vec![("special.png", "text"), ("*.png", "image")]);
        assert_eq!(
            resolve_handler(&settings, Path::new("special.png")),
            Some(HandlerKind::Text)
        );
        assert_eq!(
            resolve_handler(&settings, Path::new("other.png")),
            Some(HandlerKind::Image)
        );
    }

    #[test]
    fn an_unreadable_handler_name_falls_through_to_the_next_rule() {
        // No builtin default exists for `*.xyz`, so an unparsable handler
        // name on the only matching rule leaves nothing to resolve to.
        let settings = settings_with_rules(vec![("*.xyz", "not-a-real-handler")]);
        assert_eq!(resolve_handler(&settings, Path::new("a.xyz")), None);
    }

    #[test]
    fn project_rules_replace_the_global_layer_wholesale_via_scope_resolve() {
        // This module only ever reads one rule set; `scope::resolve` is what
        // decides which one — proved here the same way `editing`'s tests
        // prove it for the `[editing]` section.
        let global = settings_with_rules(vec![("*.svg", "image")]);
        let project = ProjectSettings {
            file_associations: Some(FileAssociationSettings {
                rules: vec![FileAssociationRule {
                    pattern: "*.svg".into(),
                    handler: "text".into(),
                }],
            }),
            ..ProjectSettings::default()
        };
        let resolved = crate::scope::resolve(&global, &project);
        assert_eq!(
            resolve_handler(&resolved, Path::new("logo.svg")),
            Some(HandlerKind::Text)
        );
    }
}
