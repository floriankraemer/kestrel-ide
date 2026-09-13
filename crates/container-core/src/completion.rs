//! Image-name completion (C6): one ranking over what the local engine has
//! and what Docker Hub answered, for the editor's popup on a Dockerfile
//! `FROM` or a compose `image:`.
//!
//! Pure: the Hub answer arrives as already-fetched [`HubRepo`]s — the
//! client that fetches them is `container-registry`'s `hub` module, kept
//! out of this crate so no HTTP stack (and reqwest's private tokio
//! runtime) enters the tree beneath `run-core`.

use serde::Deserialize;

/// One Docker Hub search hit, reduced to what completion ranks and shows.
/// A plain DTO with Hub's own field names so `container-registry` parses
/// straight into it; nothing here performs I/O.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HubRepo {
    #[serde(rename = "repo_name")]
    pub name: String,
    #[serde(default)]
    pub is_official: bool,
    #[serde(default)]
    pub star_count: u64,
    #[serde(default, rename = "short_description")]
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImageCompletionKind {
    /// An image the connected engine already has.
    Local,
    /// A Docker Hub official image (`nginx`, `postgres`).
    Official,
    /// Any other Hub repository (`bitnami/redis`).
    Community,
    /// A tag of the repository typed before the `:`.
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCompletion {
    pub label: String,
    pub insert: String,
    pub detail: String,
    pub kind: ImageCompletionKind,
}

/// Rank: local images whose name starts with `prefix` first, then Hub
/// official images, then community images by stars, then the rest of the
/// local images (a substring match). `hub` is `None` while the network
/// answer is still pending — the popup shows local results at once and
/// is re-filled when Hub answers.
///
/// A `prefix` with a `:` is a tag request: `hub` is then the tag list of
/// that repository (each a [`HubRepo`] whose `name` is the tag), and only
/// local images of the same repository are offered next to it.
pub fn image_completions(
    prefix: &str,
    local: &[String],
    hub: Option<&[HubRepo]>,
) -> Vec<ImageCompletion> {
    if let Some((repository, tag_prefix)) = prefix.split_once(':') {
        return tag_completions(repository, tag_prefix, local, hub);
    }
    let prefix_lower = prefix.to_lowercase();
    let mut out: Vec<ImageCompletion> = local
        .iter()
        .filter(|name| name.to_lowercase().starts_with(&prefix_lower))
        .map(|name| local_item(name))
        .collect();

    let mut remote: Vec<&HubRepo> = hub
        .unwrap_or_default()
        .iter()
        .filter(|repo| !local.contains(&repo.name))
        .collect();
    remote.sort_by(|a, b| {
        b.is_official
            .cmp(&a.is_official)
            .then(b.star_count.cmp(&a.star_count))
            .then(a.name.cmp(&b.name))
    });
    out.extend(remote.into_iter().map(|repo| ImageCompletion {
        label: repo.name.clone(),
        insert: repo.name.clone(),
        detail: if repo.is_official {
            format!("official · {} ★", repo.star_count)
        } else {
            format!("{} ★ · {}", repo.star_count, repo.description)
        },
        kind: if repo.is_official {
            ImageCompletionKind::Official
        } else {
            ImageCompletionKind::Community
        },
    }));

    out.extend(
        local
            .iter()
            .filter(|name| {
                let lower = name.to_lowercase();
                !prefix_lower.is_empty()
                    && !lower.starts_with(&prefix_lower)
                    && lower.contains(&prefix_lower)
            })
            .map(|name| local_item(name)),
    );
    out
}

fn tag_completions(
    repository: &str,
    tag_prefix: &str,
    local: &[String],
    hub: Option<&[HubRepo]>,
) -> Vec<ImageCompletion> {
    let tag_prefix_lower = tag_prefix.to_lowercase();
    let mut out: Vec<ImageCompletion> = local
        .iter()
        .filter(|name| {
            name.strip_prefix(repository)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|tag| tag.to_lowercase().starts_with(&tag_prefix_lower))
        })
        .map(|name| local_item(name))
        .collect();
    out.extend(
        hub.unwrap_or_default()
            .iter()
            .filter(|tag| tag.name.to_lowercase().starts_with(&tag_prefix_lower))
            .map(|tag| format!("{repository}:{}", tag.name))
            .filter(|full| !local.contains(full))
            .map(|full| ImageCompletion {
                label: full.clone(),
                insert: full,
                detail: "Docker Hub tag".to_string(),
                kind: ImageCompletionKind::Tag,
            }),
    );
    out
}

fn local_item(name: &str) -> ImageCompletion {
    ImageCompletion {
        label: name.to_string(),
        insert: name.to_string(),
        detail: "local image".to_string(),
        kind: ImageCompletionKind::Local,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str, official: bool, stars: u64) -> HubRepo {
        HubRepo {
            name: name.into(),
            is_official: official,
            star_count: stars,
            description: format!("{name} desc"),
        }
    }

    fn labels(items: &[ImageCompletion]) -> Vec<(&str, ImageCompletionKind)> {
        items
            .iter()
            .map(|item| (item.label.as_str(), item.kind))
            .collect()
    }

    #[test]
    fn ranking_local_prefix_then_official_then_community_by_stars_then_local_substring() {
        let local = vec![
            "nginx:1.27".to_string(),
            "my-nginx-proxy:dev".to_string(),
            "postgres:16".to_string(),
        ];
        let hub = vec![
            repo("bitnami/nginx", false, 200),
            repo("nginx", true, 21378),
            repo("nginxinc/nginx-unprivileged", false, 300),
        ];
        let got = image_completions("ngi", &local, Some(&hub));
        use ImageCompletionKind::*;
        assert_eq!(
            labels(&got),
            vec![
                ("nginx:1.27", Local),
                ("nginx", Official),
                ("nginxinc/nginx-unprivileged", Community),
                ("bitnami/nginx", Community),
                ("my-nginx-proxy:dev", Local),
            ]
        );
        assert!(got[1].detail.starts_with("official"));
    }

    #[test]
    fn without_hub_only_local_results_and_hub_names_already_local_are_not_doubled() {
        let local = vec!["nginx".to_string()];
        assert_eq!(
            labels(&image_completions("n", &local, None)),
            vec![("nginx", ImageCompletionKind::Local)]
        );
        let hub = vec![repo("nginx", true, 1)];
        assert_eq!(image_completions("n", &local, Some(&hub)).len(), 1);
    }

    #[test]
    fn empty_prefix_lists_every_local_image_once() {
        let local = vec!["a:1".to_string(), "b:2".to_string()];
        assert_eq!(image_completions("", &local, None).len(), 2);
    }

    #[test]
    fn a_colon_switches_to_tags_of_that_repository() {
        let local = vec![
            "nginx:1.27".to_string(),
            "nginx:alpine".to_string(),
            "redis:7".to_string(),
        ];
        let hub = vec![
            repo("1.27", false, 0),
            repo("1.28", false, 0),
            repo("alpine", false, 0),
        ];
        let got = image_completions("nginx:1", &local, Some(&hub));
        use ImageCompletionKind::*;
        assert_eq!(
            labels(&got),
            vec![("nginx:1.27", Local), ("nginx:1.28", Tag)]
        );
    }
}
