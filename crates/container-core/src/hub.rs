//! Docker Hub's public API (C6): repository search for image-name
//! completion, and a repository's tags once the prefix has a `:`.
//!
//! Only what the editor's completion needs — registry browsing, tokens
//! and private registries are C7's `registry.rs`. Blocking `reqwest`, the
//! same TLS stack `ai-chat-core` already pulls in; callers put it on a
//! worker thread. Short timeout: a popup that waits three seconds for a
//! network answer is worse than one without Hub results.

use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;

const SEARCH_URL: &str = "https://hub.docker.com/v2/search/repositories/";
const TAGS_URL: &str = "https://hub.docker.com/v2/repositories/";
const TIMEOUT: Duration = Duration::from_secs(3);
const SEARCH_PAGE_SIZE: u32 = 25;
const TAGS_PAGE_SIZE: u32 = 50;

/// One Hub search hit, reduced to what completion ranks and shows.
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

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<HubRepo>,
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    results: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

/// Search Hub repositories matching `query` — official images come back
/// flagged, ranking is [`crate::completion::image_completions`]'s.
pub fn search(query: &str) -> Result<Vec<HubRepo>, String> {
    let response = client()?
        .get(SEARCH_URL)
        .query(&[
            ("query", query),
            ("page_size", &SEARCH_PAGE_SIZE.to_string()),
        ])
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let body = response.text().map_err(|error| error.to_string())?;
    parse_search(&body)
}

/// The most recent tags of a Hub repository (`library/nginx`,
/// `bitnami/redis` — [`crate::image_ref::ImageRef::hub_repository`]'s shape).
pub fn tags(repository: &str) -> Result<Vec<String>, String> {
    let response = client()?
        .get(format!("{TAGS_URL}{repository}/tags/"))
        .query(&[("page_size", TAGS_PAGE_SIZE.to_string())])
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let body = response.text().map_err(|error| error.to_string())?;
    parse_tags(&body)
}

pub fn parse_search(body: &str) -> Result<Vec<HubRepo>, String> {
    serde_json::from_str::<SearchResponse>(body)
        .map(|response| response.results)
        .map_err(|error| format!("Docker Hub returned an unexpected answer: {error}"))
}

pub fn parse_tags(body: &str) -> Result<Vec<String>, String> {
    serde_json::from_str::<TagsResponse>(body)
        .map(|response| response.results.into_iter().map(|tag| tag.name).collect())
        .map_err(|error| format!("Docker Hub returned an unexpected answer: {error}"))
}

fn client() -> Result<&'static reqwest::blocking::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(TIMEOUT)
                .user_agent("ide-containers")
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_response_parses_the_captured_fixture() {
        let body = include_str!("../testdata/hub/search-nginx.json");
        let repos = parse_search(body).unwrap();
        assert_eq!(repos.len(), 25);
        assert_eq!(
            repos[0],
            HubRepo {
                name: "nginx".into(),
                is_official: true,
                star_count: 21378,
                description: "Official build of Nginx.".into(),
            }
        );
        assert_eq!(repos[1].name, "nginx/nginx-ingress");
        assert!(!repos[1].is_official);
    }

    #[test]
    fn tags_response_parses_the_captured_fixture() {
        let body = include_str!("../testdata/hub/tags-library-nginx.json");
        let tags = parse_tags(body).unwrap();
        assert_eq!(tags.len(), 5);
        assert_eq!(tags[0], "stable-alpine3.24-perl");
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(parse_search("<html>").is_err());
        assert!(parse_tags("{}").unwrap().is_empty());
    }
}
