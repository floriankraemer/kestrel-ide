//! Image references in editor text (C6): parsing one
//! (`registry/namespace/repo:tag@digest`), finding the one under the caret
//! in a Dockerfile `FROM` line or a compose `image:` line, and deciding
//! whether the caret sits where an image name is being typed.
//!
//! Pure text rules — no engine call, no parse tree. A `FROM` line and an
//! `image:` line are regular enough that a tokenizer is the honest tool,
//! and both the intention ("Pull image …") and the completion trigger
//! need an answer per keystroke.

use std::fmt;
use std::ops::Range;

/// A parsed image reference. `registry` is the first path component when
/// it contains a `.` or `:` or is `localhost` — the distribution/reference
/// grammar's own rule, which is how `docker` itself tells `myregistry:5000/
/// app` from `library/app`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageRef {
    pub registry: Option<String>,
    /// Everything between the registry and the tag/digest: `nginx`,
    /// `library/nginx`, `org/team/app`.
    pub repository: String,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

impl ImageRef {
    /// `None` for an empty string or one that is not a reference at all
    /// (whitespace inside, a bare tag, a `$VARIABLE`).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty()
            || text.contains(char::is_whitespace)
            || text.contains('$')
            || text.starts_with(':')
            || text.starts_with('@')
        {
            return None;
        }
        let (rest, digest) = match text.split_once('@') {
            Some((rest, digest)) => (rest, Some(digest.to_string())),
            None => (text, None),
        };
        let (mut registry, mut path) = (None, rest);
        if let Some((first, remainder)) = rest.split_once('/') {
            if first == "localhost" || first.contains('.') || first.contains(':') {
                registry = Some(first.to_string());
                path = remainder;
            }
        }
        // A `:` after the last `/` is the tag separator; before it, it is a
        // registry port.
        let (repository, tag) = match path.rsplit_once(':') {
            Some((repository, tag)) if !tag.contains('/') => {
                (repository.to_string(), Some(tag.to_string()))
            }
            _ => (path.to_string(), None),
        };
        if repository.is_empty() {
            return None;
        }
        Some(ImageRef {
            registry,
            repository,
            tag,
            digest,
        })
    }

    /// The Docker Hub namespace/repository pair for this reference, when
    /// it lives on Docker Hub at all: `nginx` → `library/nginx`,
    /// `bitnami/redis` → `bitnami/redis`. `None` for another registry or a
    /// path deeper than two components (Hub has none).
    pub fn hub_repository(&self) -> Option<String> {
        if self
            .registry
            .as_deref()
            .is_some_and(|registry| registry != "docker.io")
        {
            return None;
        }
        match self.repository.split('/').collect::<Vec<_>>().as_slice() {
            [name] => Some(format!("library/{name}")),
            [namespace, name] => Some(format!("{namespace}/{name}")),
            _ => None,
        }
    }
}

impl fmt::Display for ImageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(registry) = &self.registry {
            write!(f, "{registry}/")?;
        }
        write!(f, "{}", self.repository)?;
        if let Some(tag) = &self.tag {
            write!(f, ":{tag}")?;
        }
        if let Some(digest) = &self.digest {
            write!(f, "@{digest}")?;
        }
        Ok(())
    }
}

/// The reference the caret (`col`, a char index into `line_text`) is on,
/// in a Dockerfile `FROM [--platform=…] <ref> [AS name]` line or a compose
/// `image: <ref>` line, with the char range it occupies. `scratch` is not
/// an image and is never returned.
pub fn image_ref_at(line_text: &str, col: usize) -> Option<(ImageRef, Range<usize>)> {
    let range = reference_range(line_text)?;
    if col < range.start || col > range.end {
        return None;
    }
    let text: String = line_text
        .chars()
        .skip(range.start)
        .take(range.end - range.start)
        .collect();
    if text == "scratch" {
        return None;
    }
    ImageRef::parse(&text).map(|reference| (reference, range))
}

/// The char range of the image reference token in a `FROM`/`image:`
/// line, or `None` when the line is neither.
fn reference_range(line_text: &str) -> Option<Range<usize>> {
    let start = image_token_start(line_text)?;
    let end = start
        + line_text
            .chars()
            .skip(start)
            .take_while(|c| !c.is_whitespace() && *c != '#')
            .count();
    let token: String = line_text.chars().skip(start).take(end - start).collect();
    let token = token.trim_matches(|c| c == '"' || c == '\'').to_string();
    if token.is_empty() {
        return None;
    }
    let quote_offset = line_text
        .chars()
        .skip(start)
        .take_while(|c| *c == '"' || *c == '\'')
        .count();
    Some(start + quote_offset..start + quote_offset + token.chars().count())
}

/// Where the image token begins (char index) in a `FROM` or `image:` line.
fn image_token_start(line_text: &str) -> Option<usize> {
    let chars: Vec<char> = line_text.chars().collect();
    let mut index = chars.iter().take_while(|c| c.is_whitespace()).count();
    let word = |from: usize| -> String {
        chars[from..]
            .iter()
            .take_while(|c| !c.is_whitespace())
            .collect()
    };
    let skip_spaces = |mut from: usize| {
        while from < chars.len() && chars[from].is_whitespace() {
            from += 1;
        }
        from
    };
    let head = word(index);
    // `FROM` alone, with nothing after it, is not yet a place to complete.
    let after_head = index + head.chars().count();
    if !chars.get(after_head).is_some_and(|c| c.is_whitespace()) {
        return None;
    }
    index = skip_spaces(after_head);
    if head.eq_ignore_ascii_case("from") {
        // `--platform=linux/amd64` and any other flag before the reference.
        while index < chars.len() && chars[index] == '-' {
            let flag = word(index);
            index = skip_spaces(index + flag.chars().count());
        }
        return Some(index);
    }
    // YAML needs a space after the key, so `image:nginx` is a scalar, not
    // the key — only a bare `image:` counts.
    (head == "image:").then_some(index)
}

/// Where completion is being asked for: the text typed so far of an image
/// reference (may be empty right after `FROM ` / `image: `).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionContext {
    pub prefix: String,
}

/// Whether `line_before_cursor` ends inside an image reference — after
/// `FROM ` (and its flags) in a Dockerfile, after `image: ` in a compose
/// file — and if so, what has been typed of it. `None` anywhere else:
/// after `AS`, in a comment, on a `RUN` line.
pub fn completion_context(
    line_before_cursor: &str,
    is_dockerfile: bool,
) -> Option<CompletionContext> {
    let start = image_token_start(line_before_cursor)?;
    let head: String = line_before_cursor
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| !c.is_whitespace())
        .collect();
    if is_dockerfile != head.eq_ignore_ascii_case("from") {
        return None;
    }
    let prefix: String = line_before_cursor.chars().skip(start).collect();
    if prefix.contains(char::is_whitespace) || prefix.contains('#') {
        return None;
    }
    Some(CompletionContext {
        prefix: prefix.trim_matches(|c| c == '"' || c == '\'').to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> ImageRef {
        ImageRef::parse(text).unwrap()
    }

    #[test]
    fn parse_table() {
        // (text, registry, repository, tag, digest)
        type Case = (
            &'static str,
            Option<&'static str>,
            &'static str,
            Option<&'static str>,
            Option<&'static str>,
        );
        let cases: Vec<Case> = vec![
            ("nginx", None, "nginx", None, None),
            ("nginx:1.27", None, "nginx", Some("1.27"), None),
            ("bitnami/redis:7", None, "bitnami/redis", Some("7"), None),
            (
                "ghcr.io/org/app:v1",
                Some("ghcr.io"),
                "org/app",
                Some("v1"),
                None,
            ),
            ("localhost/app", Some("localhost"), "app", None, None),
            (
                "myregistry:5000/app",
                Some("myregistry:5000"),
                "app",
                None,
                None,
            ),
            (
                "myregistry:5000/app:2",
                Some("myregistry:5000"),
                "app",
                Some("2"),
                None,
            ),
            ("nginx@sha256:abc", None, "nginx", None, Some("sha256:abc")),
            (
                "docker.io/library/nginx:1@sha256:abc",
                Some("docker.io"),
                "library/nginx",
                Some("1"),
                Some("sha256:abc"),
            ),
        ];
        for (text, registry, repository, tag, digest) in cases {
            let reference = parsed(text);
            assert_eq!(reference.registry.as_deref(), registry, "{text}");
            assert_eq!(reference.repository, repository, "{text}");
            assert_eq!(reference.tag.as_deref(), tag, "{text}");
            assert_eq!(reference.digest.as_deref(), digest, "{text}");
            assert_eq!(reference.to_string(), text, "round trip");
        }
        for bad in ["", "  ", ":tag", "@sha256:x", "a b", "$BASE", "ghcr.io/"] {
            assert_eq!(ImageRef::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn hub_repository_only_for_hub_shaped_references() {
        assert_eq!(
            parsed("nginx").hub_repository().as_deref(),
            Some("library/nginx")
        );
        assert_eq!(
            parsed("bitnami/redis").hub_repository().as_deref(),
            Some("bitnami/redis")
        );
        assert_eq!(
            parsed("docker.io/library/nginx")
                .hub_repository()
                .as_deref(),
            Some("library/nginx")
        );
        assert_eq!(parsed("ghcr.io/org/app").hub_repository(), None);
        assert_eq!(parsed("a/b/c").hub_repository(), None);
    }

    #[test]
    fn image_ref_at_dockerfile_from_lines() {
        let (reference, range) = image_ref_at("FROM nginx:1.27 AS base", 7).unwrap();
        assert_eq!(reference.to_string(), "nginx:1.27");
        assert_eq!(range, 5..15);
        assert_eq!(image_ref_at("FROM nginx:1.27 AS base", 18), None, "on AS");
        let (reference, range) =
            image_ref_at("from --platform=linux/amd64 ghcr.io/org/app:v1", 30).unwrap();
        assert_eq!(reference.registry.as_deref(), Some("ghcr.io"));
        assert_eq!(range, 28..46);
        assert_eq!(image_ref_at("FROM scratch", 6), None);
        assert_eq!(image_ref_at("RUN apt-get install nginx", 10), None);
        assert_eq!(image_ref_at("FROM $BASE", 6), None);
        assert_eq!(image_ref_at("FROM ", 5), None);
        assert!(
            image_ref_at("FROM nginx", 5).is_some(),
            "range start inclusive"
        );
        assert!(
            image_ref_at("FROM nginx", 10).is_some(),
            "range end inclusive"
        );
    }

    #[test]
    fn image_ref_at_compose_image_lines() {
        let (reference, range) = image_ref_at("    image: \"postgres:16\"", 15).unwrap();
        assert_eq!(reference.to_string(), "postgres:16");
        assert_eq!(range, 12..23);
        assert!(image_ref_at("  image: redis # cache", 10).is_some());
        assert_eq!(image_ref_at("  image: redis # cache", 18), None);
        assert_eq!(image_ref_at("  images: redis", 10), None);
        assert_eq!(image_ref_at("  image:redis", 10), None);
    }

    #[test]
    fn completion_context_positions() {
        let ctx =
            |line: &str, dockerfile: bool| completion_context(line, dockerfile).map(|c| c.prefix);
        assert_eq!(ctx("FROM ", true).as_deref(), Some(""));
        assert_eq!(ctx("FROM ngi", true).as_deref(), Some("ngi"));
        assert_eq!(
            ctx("FROM --platform=linux/arm64 alp", true).as_deref(),
            Some("alp")
        );
        assert_eq!(ctx("FROM nginx:1.", true).as_deref(), Some("nginx:1."));
        assert_eq!(ctx("FROM nginx AS ", true), None);
        assert_eq!(ctx("FROM", true), None);
        assert_eq!(ctx("RUN ngi", true), None);
        assert_eq!(ctx("    image: post", false).as_deref(), Some("post"));
        assert_eq!(ctx("    image: \"post", false).as_deref(), Some("post"));
        assert_eq!(ctx("    image: ", false).as_deref(), Some(""));
        assert_eq!(ctx("    image: postgres # ", false), None);
        assert_eq!(ctx("    build: .", false), None);
        assert_eq!(
            ctx("FROM ngi", false),
            None,
            "a FROM line in a compose file"
        );
        assert_eq!(
            ctx("image: ngi", true),
            None,
            "an image: line in a Dockerfile"
        );
    }
}
