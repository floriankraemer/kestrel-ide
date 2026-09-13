//! Compose files (C6): which files are compose files, and where each
//! service is declared in one.
//!
//! [`is_compose_file`] is a file-name rule, not a language — compose is a
//! YAML *flavor* (ADR-0018: one detection table, and `yaml` already owns
//! `*.yml`), so `ui-shell` and `run-core` call this where they need to
//! know. [`services_with_lines`] walks the tree-sitter-yaml parse
//! (through `syntax-core`'s registry, the one grammar the editor already
//! highlights with) for the top-level `services` mapping: one entry per
//! service key, with its line, its `image:` and its published ports —
//! everything the gutter markers and the code lenses need, and nothing a
//! compose *run* needs (that is `compose config --services`,
//! `run_config::compose_services`, the ground truth once interpolation
//! and `extends` are involved).

use std::path::Path;

use tree_sitter::Node;

/// `docker-compose.yml`, `compose.yaml`, `container-compose.yml`,
/// `podman-compose.yaml`, `compose.override.yml`, `docker-compose.prod.yaml`…
///
/// `^(docker-|container-|podman-)?compose(\..+)?\.ya?ml$` on the file
/// name, spelled out rather than through `regex` so this crate does not
/// pull the crate in for one anchored pattern.
pub fn is_compose_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(stem) = name
        .strip_suffix(".yml")
        .or_else(|| name.strip_suffix(".yaml"))
    else {
        return false;
    };
    let stem = ["docker-", "container-", "podman-"]
        .iter()
        .find_map(|prefix| stem.strip_prefix(prefix))
        .unwrap_or(stem);
    stem == "compose" || stem.starts_with("compose.") && stem.len() > "compose.".len()
}

/// One service under a compose file's top-level `services:` mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeServiceSpan {
    pub name: String,
    /// 0-based line of the service's key.
    pub line: u32,
    /// `(host, container)` for every port the file publishes, short
    /// (`"8080:80"`, `"127.0.0.1:8080:80/tcp"`) or long (`published:`/
    /// `target:`) syntax. A short entry with no host part (`"80"`) is
    /// skipped: the engine picks an ephemeral port, the file cannot say.
    pub ports: Vec<(String, String)>,
    pub image: Option<String>,
}

/// The line of the top-level `services:` key and every service under it,
/// in file order. Empty when the file has no `services` mapping or does
/// not parse as YAML at all. Extension keys (`x-…`), merge keys (`<<`)
/// and anchors are skipped rather than reported; flow-style mappings
/// (`services: {web: …}`) are not walked.
pub fn services_with_lines(text: &str) -> Option<(u32, Vec<ComposeServiceSpan>)> {
    let tree = parse_yaml(text)?;
    let root_mapping = first_block_mapping(tree.root_node())?;
    let services_pair = mapping_pairs(root_mapping, text)
        .into_iter()
        .find(|(key, _)| key == "services")?;
    let line = services_pair.1.start_position().row as u32;
    let services = first_block_mapping(services_pair.1)
        .map(|mapping| {
            mapping_pairs(mapping, text)
                .into_iter()
                .filter(|(name, _)| !is_skipped_key(name))
                .map(|(name, pair)| ComposeServiceSpan {
                    name,
                    line: pair.start_position().row as u32,
                    ports: ports_of(pair, text),
                    image: child_scalar(pair, "image", text),
                })
                .collect()
        })
        .unwrap_or_default();
    Some((line, services))
}

fn parse_yaml(text: &str) -> Option<tree_sitter::Tree> {
    let language = syntax_core::language_by_id("yaml")?;
    let compiled = syntax_core::registry().compiled(language)?.ok()?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&compiled.grammar).ok()?;
    parser.parse(text, None)
}

fn is_skipped_key(key: &str) -> bool {
    key.starts_with("x-") || key == "<<"
}

/// The first `block_mapping` beneath `node` (through `document`/
/// `block_node`/`anchor` wrappers), or `None` for a scalar, a sequence or a
/// flow mapping.
fn first_block_mapping(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "block_mapping" {
        return Some(node);
    }
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
    children
        .into_iter()
        .filter(|child| {
            matches!(
                child.kind(),
                "stream" | "document" | "block_node" | "block_mapping"
            )
        })
        .find_map(first_block_mapping)
}

fn child_scalar(pair: Node<'_>, key: &str, text: &str) -> Option<String> {
    let value = pair.child_by_field_name("value")?;
    let mapping = first_block_mapping(value)?;
    mapping_pairs(mapping, text)
        .into_iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, pair)| pair.child_by_field_name("value"))
        .map(|value| scalar_text(value, text))
        .filter(|value| !value.is_empty())
}

fn ports_of(pair: Node<'_>, text: &str) -> Vec<(String, String)> {
    let Some(value) = pair.child_by_field_name("value") else {
        return Vec::new();
    };
    let Some(mapping) = first_block_mapping(value) else {
        return Vec::new();
    };
    let Some((_, ports_pair)) = mapping_pairs(mapping, text)
        .into_iter()
        .find(|(name, _)| name == "ports")
    else {
        return Vec::new();
    };
    let Some(ports_value) = ports_pair.child_by_field_name("value") else {
        return Vec::new();
    };
    sequence_items(ports_value)
        .into_iter()
        .filter_map(|item| port_of_item(item, text))
        .collect()
}

/// Every item node of the `block_sequence` beneath `node`.
fn sequence_items(node: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "block_node" => out.extend(sequence_items(child)),
            "block_sequence" => {
                let mut inner = child.walk();
                for item in child.children(&mut inner) {
                    if item.kind() == "block_sequence_item" {
                        if let Some(content) = item.named_child(0) {
                            out.push(content);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `"8080:80"`, `"127.0.0.1:8080:80/udp"` (short) or `published: 8080` +
/// `target: 80` (long) → `(host, container)`.
fn port_of_item(item: Node<'_>, text: &str) -> Option<(String, String)> {
    if let Some(mapping) = first_block_mapping(item) {
        let pairs = mapping_pairs(mapping, text);
        let find = |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .and_then(|(_, pair)| pair.child_by_field_name("value"))
                .map(|value| scalar_text(value, text))
        };
        return Some((find("published")?, find("target")?));
    }
    let short = scalar_text(item, text);
    let spec = short.split('/').next().unwrap_or(&short);
    let mut parts: Vec<&str> = spec.split(':').collect();
    let container = parts.pop()?;
    let host = parts.pop()?;
    Some((host.to_string(), container.to_string()))
}

/// The scalar text of a value node, quotes stripped, or empty when the
/// node is not a scalar (a mapping, a sequence, an alias).
fn scalar_text(node: Node<'_>, text: &str) -> String {
    let raw = node.utf8_text(text.as_bytes()).unwrap_or_default().trim();
    let unquoted = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(raw);
    if unquoted.contains('\n') || unquoted.starts_with('*') || unquoted.starts_with('&') {
        return String::new();
    }
    unquoted.to_string()
}

fn mapping_pairs<'tree>(mapping: Node<'tree>, text: &str) -> Vec<(String, Node<'tree>)> {
    let mut cursor = mapping.walk();
    mapping
        .children(&mut cursor)
        .filter(|child| child.kind() == "block_mapping_pair")
        .filter_map(|pair| {
            let key = pair.child_by_field_name("key")?;
            Some((scalar_text(key, text), pair))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn services(text: &str) -> Vec<ComposeServiceSpan> {
        services_with_lines(text)
            .map(|(_, s)| s)
            .unwrap_or_default()
    }

    #[test]
    fn is_compose_file_matches_every_conventional_variant() {
        for name in [
            "docker-compose.yml",
            "docker-compose.yaml",
            "compose.yml",
            "compose.yaml",
            "container-compose.yml",
            "podman-compose.yaml",
            "compose.override.yml",
            "docker-compose.prod.yaml",
            "/a/b/compose.yaml",
        ] {
            assert!(is_compose_file(Path::new(name)), "{name}");
        }
        for name in [
            "Dockerfile",
            "random.yml",
            "compose.yml.bak",
            "mycompose.yml",
            "compose.",
            "compose",
            "compose..yml",
        ] {
            assert!(!is_compose_file(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn short_and_long_ports_syntax_and_image() {
        let text = "services:\n  web:\n    image: nginx:1.27\n    ports:\n      - \"8080:80\"\n      - 127.0.0.1:9090:90/udp\n      - \"80\"\n      - target: 443\n        published: 8443\n        protocol: tcp\n  db:\n    image: 'postgres'\n";
        let (line, found) = services_with_lines(text).unwrap();
        assert_eq!(line, 0);
        assert_eq!(
            found,
            vec![
                ComposeServiceSpan {
                    name: "web".into(),
                    line: 1,
                    ports: vec![
                        ("8080".into(), "80".into()),
                        ("9090".into(), "90".into()),
                        ("8443".into(), "443".into()),
                    ],
                    image: Some("nginx:1.27".into()),
                },
                ComposeServiceSpan {
                    name: "db".into(),
                    line: 10,
                    ports: vec![],
                    image: Some("postgres".into()),
                },
            ]
        );
    }

    #[test]
    fn anchors_merge_keys_quoted_keys_and_extension_keys() {
        let text = "x-common: &common\n  restart: always\nversion: \"3.9\"\nservices:\n  x-template: &tpl\n    image: busybox\n  \"quoted\": *tpl\n  api:\n    <<: *common\n    image: app:dev\n    build: .\n";
        let (line, found) = services_with_lines(text).unwrap();
        assert_eq!(line, 3);
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["quoted", "api"]);
        assert_eq!(found[0].line, 6);
        assert_eq!(
            found[0].image, None,
            "an alias is not a mapping we can read"
        );
        assert_eq!(found[1].image.as_deref(), Some("app:dev"));
    }

    #[test]
    fn no_services_key_is_none_and_garbage_is_empty() {
        assert_eq!(
            services_with_lines("version: '3'\nvolumes:\n  data:\n"),
            None
        );
        assert_eq!(services_with_lines(""), None);
        assert!(services("services: [not, a, mapping]\n").is_empty());
        assert_eq!(
            services_with_lines("services:\n").map(|(l, s)| (l, s.len())),
            Some((0, 0))
        );
    }
}
