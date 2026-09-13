//! Images (C4, ADR-0055): pull/tag/remove/prune argv builders, the
//! `history` layer parser (Docker's NDJSON `{{json .}}` vs. Podman's
//! `--format json` array), containers-using-an-image, copying an image to
//! another connection (`save`\|`load` through a temp file) and local
//! image-name completion.
//!
//! `history`'s two shapes are a real engine difference, not a wording one
//! (ADR-0055's "one argv" rule is for subcommands both CLIs share byte for
//! byte): Docker prints one Go-template JSON object per line and its
//! `Size` is a human string (`"22.3MB"`); Podman's `--format json` prints
//! the whole array at once with a numeric byte `size`. [`history_args`]
//! picks the argv per engine, [`parse_history`] accepts either shape
//! regardless of which one actually produced it.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::connection::{Engine, Invocation};
use crate::model::{self, Container, Image};
use crate::ops::{run_op, OpError, OpErrorCode};

/// `pull <reference>`.
pub fn pull_args(reference: &str) -> Vec<String> {
    vec!["pull".to_string(), reference.to_string()]
}

/// `rmi [-f] <id>`.
pub fn rmi_args(id: &str, force: bool) -> Vec<String> {
    let mut args = vec!["rmi".to_string()];
    if force {
        args.push("-f".to_string());
    }
    args.push(id.to_string());
    args
}

/// `image prune -f [-a]` — `-a` also removes tagged images with no
/// container using them, not just dangling ones.
pub fn image_prune_args(all: bool) -> Vec<String> {
    let mut args = vec!["image".to_string(), "prune".to_string(), "-f".to_string()];
    if all {
        args.push("-a".to_string());
    }
    args
}

/// `tag <src> <dst>`.
pub fn tag_args(src: &str, dst: &str) -> Vec<String> {
    vec!["tag".to_string(), src.to_string(), dst.to_string()]
}

/// `save -o <tar_path> <id>`.
fn save_args(id: &str, tar_path: &str) -> Vec<String> {
    vec![
        "save".to_string(),
        "-o".to_string(),
        tar_path.to_string(),
        id.to_string(),
    ]
}

/// `load -i <tar_path>`.
fn load_args(tar_path: &str) -> Vec<String> {
    vec!["load".to_string(), "-i".to_string(), tar_path.to_string()]
}

/// `history --no-trunc --format '{{json .}}' <id>` (Docker) or `history
/// --no-trunc --format json <id>` (Podman) — see the module doc comment
/// for why these differ.
pub fn history_args(engine: Engine, id: &str) -> Vec<String> {
    let mut args = vec!["history".to_string(), "--no-trunc".to_string()];
    args.push("--format".to_string());
    args.push(
        match engine {
            Engine::Docker => "{{json .}}",
            Engine::Podman => "json",
        }
        .to_string(),
    );
    args.push(id.to_string());
    args
}

/// One layer of `history`'s answer, engine-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    /// `sha256:...`, or `<missing>` for an intermediate layer Docker did
    /// not tag — kept verbatim rather than guessed at.
    pub id: String,
    /// RFC 3339 (Docker's `CreatedAt`/Podman's `created`) as printed;
    /// the view formats it with [`crate::model::parse_timestamp`]/[`crate::model::age`]
    /// the same way every other timestamp in this crate is shown.
    pub created: String,
    pub created_by: String,
    pub size: u64,
    pub comment: String,
}

#[derive(Debug, Deserialize)]
struct RawDockerLayer {
    #[serde(rename = "ID", default)]
    id: String,
    #[serde(rename = "CreatedAt", default)]
    created_at: String,
    #[serde(rename = "CreatedBy", default)]
    created_by: String,
    /// A human-formatted string (`"22.3MB"`, `"0B"`) — Docker's `history`
    /// table format, JSON included, never prints raw bytes.
    #[serde(rename = "Size", default)]
    size: String,
    #[serde(rename = "Comment", default)]
    comment: String,
}

#[derive(Debug, Deserialize)]
struct RawPodmanLayer {
    #[serde(default)]
    id: String,
    #[serde(default)]
    created: String,
    #[serde(default, rename = "createdBy")]
    created_by: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    comment: String,
}

/// Parse Docker's `size` column (`"0B"`, `"22.3MB"`, `"1.2GB"`) into bytes,
/// go-units' decimal (base-1000) scale. Anything unrecognised — including
/// an already-numeric string, which never appears from a real `history`
/// but keeps this total — falls back to `0`.
fn parse_docker_size(text: &str) -> u64 {
    let text = text.trim();
    let split_at = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split_at);
    let Ok(value) = number.parse::<f64>() else {
        return 0;
    };
    let multiplier: f64 = match unit.trim() {
        "B" | "" => 1.0,
        "kB" => 1_000.0,
        "MB" => 1_000_000.0,
        "GB" => 1_000_000_000.0,
        "TB" => 1_000_000_000_000.0,
        "PB" => 1_000_000_000_000_000.0,
        _ => 1.0,
    };
    (value * multiplier).round() as u64
}

/// Parse either shape `history` can print: a leading `[` means Podman's
/// whole-array `--format json`, anything else is Docker's NDJSON (one
/// `{{json .}}` object per line, blank lines skipped).
pub fn parse_history(output: &str) -> Result<Vec<Layer>, serde_json::Error> {
    let trimmed = output.trim_start();
    if trimmed.starts_with('[') {
        let raw: Vec<RawPodmanLayer> = serde_json::from_str(trimmed)?;
        Ok(raw
            .into_iter()
            .map(|layer| Layer {
                id: layer.id,
                created: layer.created,
                created_by: layer.created_by,
                size: layer.size,
                comment: layer.comment,
            })
            .collect())
    } else {
        output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<RawDockerLayer>(line).map(|layer| Layer {
                    id: layer.id,
                    created: layer.created_at,
                    created_by: layer.created_by,
                    size: parse_docker_size(&layer.size),
                    comment: layer.comment,
                })
            })
            .collect()
    }
}

/// Run `history` on `id` and parse its answer.
pub fn history(
    invocation: &Invocation,
    engine: Engine,
    id: &str,
    work_dir: &Path,
) -> Result<Vec<Layer>, OpError> {
    let output = run_op(invocation, &history_args(engine, id), work_dir)?;
    parse_history(&String::from_utf8_lossy(&output.stdout)).map_err(|error| OpError {
        code: OpErrorCode::Other,
        message: format!("history output was not valid JSON: {error}"),
    })
}

/// Containers whose `Config.Image`/image id matches `image_id` — the
/// Dashboard's "containers using it" list.
pub fn containers_using<'a>(image_id: &str, containers: &'a [Container]) -> Vec<&'a Container> {
    containers
        .iter()
        .filter(|container| container.image_id == image_id)
        .collect()
}

/// A temp tar path under [`std::env::temp_dir`], unique per process and
/// call so two concurrent copies never collide.
fn temp_tar_path(image_id: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "ide-container-image-{}-{}-{unique}.tar",
        std::process::id(),
        model::short_id(image_id)
    ))
}

/// Copy an image to another connection: `save -o <tmp> <id>` on `source`,
/// then `load -i <tmp>` on `dest`, through one temp file cleaned up on
/// every path (`process_exec::spawn` null-routes stdin — see ADR-0055 —
/// so this is two buffered `run` calls, not a pipe).
pub fn copy_image(
    source: &Invocation,
    dest: &Invocation,
    image_id: &str,
    work_dir: &Path,
) -> Result<(), OpError> {
    let tmp_path = temp_tar_path(image_id);
    let tmp_str = tmp_path.to_string_lossy().to_string();
    let cleanup = || {
        let _ = std::fs::remove_file(&tmp_path);
    };
    if let Err(err) = run_op(source, &save_args(image_id, &tmp_str), work_dir) {
        cleanup();
        return Err(err);
    }
    let result = run_op(dest, &load_args(&tmp_str), work_dir).map(|_| ());
    cleanup();
    result
}

/// Rank a candidate image reference against `prefix`: `0` when its repo
/// part (before the last `:`) starts with `prefix`, `1` when the whole
/// reference does (a tag match), `2` when `prefix` merely occurs
/// somewhere in it, `3` (excluded once `prefix` is non-empty) otherwise.
/// Matching is case-insensitive; ties break alphabetically so the result
/// is stable across calls.
fn rank(candidate: &str, prefix_lower: &str) -> u8 {
    let lower = candidate.to_lowercase();
    let repo = lower
        .rsplit_once(':')
        .map_or(lower.as_str(), |(repo, _)| repo);
    if repo.starts_with(prefix_lower) {
        0
    } else if lower.starts_with(prefix_lower) {
        1
    } else if lower.contains(prefix_lower) {
        2
    } else {
        3
    }
}

/// Local image-name completion (the images console's `QCompleter` feed) —
/// a `CompletionSource` seam: Hub search is C6/C7's, this ranks only what
/// the snapshot already knows about.
pub fn complete_images(images: &[Image], prefix: &str) -> Vec<String> {
    let prefix_lower = prefix.to_lowercase();
    let mut candidates: Vec<String> = images
        .iter()
        .flat_map(|image| image.repo_tags.iter().cloned())
        .collect();
    candidates.sort();
    candidates.dedup();
    let mut ranked: Vec<(u8, String)> = candidates
        .into_iter()
        .map(|candidate| (rank(&candidate, &prefix_lower), candidate))
        .filter(|(rank, _)| prefix_lower.is_empty() || *rank < 3)
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    ranked.into_iter().map(|(_, candidate)| candidate).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_rmi_tag_argv() {
        assert_eq!(pull_args("nginx:1.27"), vec!["pull", "nginx:1.27"]);
        assert_eq!(rmi_args("abc", false), vec!["rmi", "abc"]);
        assert_eq!(rmi_args("abc", true), vec!["rmi", "-f", "abc"]);
        assert_eq!(
            tag_args("nginx:1.27", "myrepo/nginx:prod"),
            vec!["tag", "nginx:1.27", "myrepo/nginx:prod"]
        );
    }

    #[test]
    fn image_prune_argv_all_flag() {
        assert_eq!(image_prune_args(false), vec!["image", "prune", "-f"]);
        assert_eq!(image_prune_args(true), vec!["image", "prune", "-f", "-a"]);
    }

    #[test]
    fn history_argv_differs_by_engine() {
        assert_eq!(
            history_args(Engine::Docker, "abc"),
            vec!["history", "--no-trunc", "--format", "{{json .}}", "abc"]
        );
        assert_eq!(
            history_args(Engine::Podman, "abc"),
            vec!["history", "--no-trunc", "--format", "json", "abc"]
        );
    }

    #[test]
    fn save_and_load_argv() {
        assert_eq!(
            save_args("abc", "/tmp/x.tar"),
            vec!["save", "-o", "/tmp/x.tar", "abc"]
        );
        assert_eq!(load_args("/tmp/x.tar"), vec!["load", "-i", "/tmp/x.tar"]);
    }

    #[test]
    fn docker_size_strings_parse_to_bytes() {
        assert_eq!(parse_docker_size("0B"), 0);
        assert_eq!(parse_docker_size("22.3MB"), 22_300_000);
        assert_eq!(parse_docker_size("1.2GB"), 1_200_000_000);
        assert_eq!(parse_docker_size("512kB"), 512_000);
        assert_eq!(parse_docker_size("garbage"), 0);
    }

    #[test]
    fn parses_docker_ndjson_history() {
        let sample = "\
{\"Comment\":\"buildkit.dockerfile.v0\",\"CreatedAt\":\"2026-09-10T01:35:34+02:00\",\"CreatedBy\":\"RUN date\",\"ID\":\"sha256:789c201d245af5f0\",\"Size\":\"0B\"}
{\"Comment\":\"\",\"CreatedAt\":\"2026-09-08T13:10:14+02:00\",\"CreatedBy\":\"RUN curl\",\"ID\":\"<missing>\",\"Size\":\"22.3MB\"}
";
        let layers = parse_history(sample).unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].id, "sha256:789c201d245af5f0");
        assert_eq!(layers[0].size, 0);
        assert_eq!(layers[1].id, "<missing>");
        assert_eq!(layers[1].size, 22_300_000);
        assert_eq!(layers[1].created_by, "RUN curl");
    }

    #[test]
    fn parses_podman_json_array_history() {
        let sample = r#"[
            {"id":"sha256:abc123","created":"2026-06-01T00:00:00Z","createdBy":"/bin/sh -c #(nop) CMD [\"nginx\"]","size":0,"comment":""},
            {"id":"<missing>","created":"2026-06-01T00:00:00Z","createdBy":"/bin/sh -c apt-get install","size":52428800,"comment":"layer"}
        ]"#;
        let layers = parse_history(sample).unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[1].size, 52_428_800);
        assert_eq!(layers[1].comment, "layer");
    }

    #[test]
    fn history_rejects_invalid_json() {
        assert!(parse_history("not json at all").is_err());
    }

    #[test]
    fn containers_using_matches_on_image_id() {
        let raw = serde_json::json!({"Id":"c1","Image":"sha256:aaa"});
        let container = Container::from_value(raw).unwrap();
        let other = serde_json::json!({"Id":"c2","Image":"sha256:bbb"});
        let other = Container::from_value(other).unwrap();
        let containers = vec![container, other];
        let using = containers_using("sha256:aaa", &containers);
        assert_eq!(using.len(), 1);
        assert_eq!(using[0].id, "c1");
    }

    #[test]
    fn completion_ranks_exact_repo_prefix_first_then_tag_matches_stably() {
        let make = |tags: &[&str]| Image {
            id: "sha256:x".to_string(),
            repo_tags: tags.iter().map(|tag| tag.to_string()).collect(),
            repo_digests: vec![],
            created: String::new(),
            size: 0,
            architecture: String::new(),
            os: String::new(),
            labels: Default::default(),
            raw: serde_json::Value::Null,
        };
        let images = vec![
            make(&["myrepo/redis:7"]),
            make(&["redis:7.2-alpine", "redis:latest"]),
            make(&["postgres:16"]),
        ];
        let ranked = complete_images(&images, "redis");
        assert_eq!(
            ranked,
            vec!["redis:7.2-alpine", "redis:latest", "myrepo/redis:7"],
            "repo-prefix matches before a tag-only match, alphabetical within a rank"
        );

        let empty_prefix = complete_images(&images, "");
        assert_eq!(empty_prefix.len(), 4, "no prefix lists everything, deduped");

        let no_match = complete_images(&images, "zzz");
        assert!(no_match.is_empty());
    }

    #[test]
    fn copy_image_cleans_up_its_temp_file_even_when_save_fails() {
        // `save` against a nonexistent program fails fast (NotFound), and
        // the temp path must not be left behind either way.
        let broken = Invocation {
            program: "definitely-not-a-real-engine-binary".to_string(),
            prefix_args: Vec::new(),
            env: Vec::new(),
            host: process_exec::host::ExecHost::Local,
        };
        let result = copy_image(&broken, &broken, "sha256:deadbeef", Path::new("."));
        assert!(result.is_err());
        let leftovers: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("ide-container-image-")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "copy_image must not leave a temp tar behind on failure"
        );
    }
}
