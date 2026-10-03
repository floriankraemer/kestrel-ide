//! The PHP language level: which `major.minor` the servers and analyzers
//! assume. Precedence: an explicit setting, then the lower bound of
//! `composer.json`'s `require.php`, then the probed interpreter version.

use std::fmt;
use std::str::FromStr;

/// `major.minor`, ordered by version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanguageLevel {
    pub major: u32,
    pub minor: u32,
}

impl fmt::Display for LanguageLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FromStr for LanguageLevel {
    type Err = ();

    /// Accepts `8`, `8.3` and `8.3.6` (the patch is dropped); nothing else.
    fn from_str(s: &str) -> Result<Self, ()> {
        let mut parts = s.trim().splitn(3, '.');
        let number = |p: Option<&str>| p.ok_or(())?.parse::<u32>().map_err(|_| ());
        let major = number(parts.next())?;
        let minor = match parts.next() {
            Some(p) => number(Some(p))?,
            None => 0,
        };
        if parts.next().is_some_and(|p| p.parse::<u32>().is_err()) {
            return Err(());
        }
        Ok(Self { major, minor })
    }
}

/// The level in force: `explicit`, else `require.php`'s lower bound, else
/// the `probed` interpreter version (`PHP 8.3.6`-style output is not
/// accepted here; pass the bare version).
pub fn resolve(
    explicit: Option<&str>,
    require_php: Option<&str>,
    probed: Option<&str>,
) -> Option<LanguageLevel> {
    explicit
        .and_then(|s| s.parse().ok())
        .or_else(|| require_php.and_then(constraint_lower_bound))
        .or_else(|| probed.and_then(|s| s.parse().ok()))
}

/// The lowest version a Composer constraint admits, as `major.minor`.
///
/// `||` alternatives yield the lowest of their bounds; within one
/// alternative the highest lower bound wins (`>=7.4 >=8.0` admits 8.0+).
/// Upper bounds (`<`, `<=`, `!=`) are ignored, and an alternative with no
/// lower bound (`*`, `<9`) contributes nothing. Stability flags and
/// `@dev`-style suffixes are not interpreted.
pub fn constraint_lower_bound(constraint: &str) -> Option<LanguageLevel> {
    constraint
        .split("||")
        .flat_map(|alt| alt.split(" | "))
        .filter_map(alternative_lower_bound)
        .min()
}

fn alternative_lower_bound(alternative: &str) -> Option<LanguageLevel> {
    alternative
        .replace(',', " ")
        .split_whitespace()
        .filter_map(token_lower_bound)
        .max()
}

fn token_lower_bound(token: &str) -> Option<LanguageLevel> {
    if token.starts_with('<') || token.starts_with("!=") {
        return None;
    }
    let version = token.trim_start_matches(['^', '~', '>', '=', 'v']);
    // `8.2.*` and `8.*`: the wildcard part is not a number, drop it.
    let version = version.split(".*").next().unwrap_or(version);
    version.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(s: &str) -> Option<String> {
        constraint_lower_bound(s).map(|l| l.to_string())
    }

    #[test]
    fn caret_tilde_and_ranges() {
        assert_eq!(level("^8.1").as_deref(), Some("8.1"));
        assert_eq!(level("~7.4.0").as_deref(), Some("7.4"));
        assert_eq!(level(">=8.0 <8.4").as_deref(), Some("8.0"));
        assert_eq!(level(">=8.0,<8.4").as_deref(), Some("8.0"));
        assert_eq!(level("8.2.*").as_deref(), Some("8.2"));
        assert_eq!(level("8").as_deref(), Some("8.0"));
    }

    #[test]
    fn alternatives_take_the_lowest_bound() {
        assert_eq!(level("^7.4 || ^8.0").as_deref(), Some("7.4"));
        assert_eq!(level("^8.0 | ^7.2").as_deref(), Some("7.2"));
    }

    #[test]
    fn unbounded_constraints_have_no_level() {
        assert_eq!(level("*"), None);
        assert_eq!(level("<9"), None);
        assert_eq!(level("garbage"), None);
    }

    #[test]
    fn precedence_is_explicit_then_composer_then_probe() {
        assert_eq!(
            resolve(Some("8.3"), Some("^7.4"), Some("8.1.2")).map(|l| l.to_string()),
            Some("8.3".into())
        );
        assert_eq!(
            resolve(None, Some("^7.4"), Some("8.1.2")).map(|l| l.to_string()),
            Some("7.4".into())
        );
        assert_eq!(
            resolve(None, None, Some("8.1.2")).map(|l| l.to_string()),
            Some("8.1".into())
        );
        assert_eq!(resolve(None, Some("*"), None), None);
    }

    #[test]
    fn a_malformed_explicit_level_falls_through() {
        assert_eq!(
            resolve(Some("latest"), Some("^8.2"), None).map(|l| l.to_string()),
            Some("8.2".into())
        );
    }
}
