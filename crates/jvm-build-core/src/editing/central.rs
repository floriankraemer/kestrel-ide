//! The Maven Central client (D4): group/artifact search
//! (`search.maven.org`'s Solr endpoint) and a coordinate's known versions
//! (`repo1.maven.org`'s `maven-metadata.xml`), for D5's completion and
//! D6's version hints when the local index (D3) alone is not enough.
//!
//! * Network calls are blocking, on whatever thread the caller is already
//!   running on (D5/D6 already do their own work off the UI thread) — no
//!   tokio in this crate, the same rule every other network client below
//!   `ui-shell` follows.
//! * A disk cache under `<config_dir>/cache/maven-central/` with a 24h
//!   TTL, keyed by request URL, so a completion popup does not refetch on
//!   every keystroke and a stale entry ages out rather than growing
//!   forever. A failed or empty answer is cached too (review fix #10),
//!   with its own much shorter 1h TTL — a coordinate that genuinely has
//!   no Central listing (a corporate-internal group, a typo) would
//!   otherwise be retried over the network on every single save.
//! * `offline: true` skips the client entirely — no network attempt and
//!   no cache read either. A stale cached answer is still a *remote*
//!   answer; "offline" means "do not use the network for this", not
//!   "prefer a network answer that happens to be sitting on disk".
//! * One `reqwest::blocking::Client` per `CentralClient` (review fix
//!   #10), not one per request — building a client is not free (its own
//!   connection pool, TLS config), and this client already lives exactly
//!   as long as the caller's own sync/session does.
//!
//! The two response parsers ([`parse_search_response`],
//! [`parse_metadata_versions`]) take plain strings and are fixture-tested
//! with no network involved; [`CentralClient::search`]/
//! [`CentralClient::versions`] are the thin, closer-to-untested glue that
//! calls them.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};

const SEARCH_URL: &str = "https://search.maven.org/solrsearch/select";
const CENTRAL_REPO_URL: &str = "https://repo1.maven.org/maven2";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Review fix #10: how long a *failed or empty* answer is trusted not to
/// have changed — much shorter than a real answer's, since a transient
/// network blip should not keep looking like "this coordinate does not
/// exist" for a whole day.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug)]
pub struct CentralError(pub String);

impl std::fmt::Display for CentralError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Maven Central request failed: {}", self.0)
    }
}

/// One `search.maven.org` result row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CentralArtifact {
    pub group_id: String,
    pub artifact_id: String,
    /// Absent for a handful of ancient indexed artifacts Central itself
    /// reports with no `latestVersion` field.
    pub latest_version: Option<String>,
}

/// Talks to Maven Central, through a 24h disk cache, skipped entirely in
/// offline mode. One instance per project sync (D5/D6 keep it alongside
/// the synced model and D3's `RepoIndex`), not per request — the cache
/// directory and offline flag do not change mid-session.
pub struct CentralClient {
    cache_dir: PathBuf,
    offline: bool,
    client: reqwest::blocking::Client,
}

impl CentralClient {
    pub fn new(config_dir: &Path, offline: bool) -> Self {
        Self {
            cache_dir: config_dir.join("cache").join("maven-central"),
            offline,
            // `.expect`: the only ways `ClientBuilder::build` fails are a
            // TLS backend that failed to initialize or a malformed default
            // header — neither depends on anything this call site controls,
            // so a failure here means the process's TLS/network stack is
            // broken in a way nothing downstream could work around either.
            client: reqwest::blocking::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("reqwest client with only a timeout set never fails to build"),
        }
    }

    /// Group/artifact search. `solr_query` is a ready-made Solr `q` value
    /// (`g:com.google.guava`, `a:guava*`, …) — building it from a caret
    /// context is D5's job, not this thin client's.
    pub fn search(&self, solr_query: &str) -> Vec<CentralArtifact> {
        if self.offline {
            return Vec::new();
        }
        let url = format!(
            "{SEARCH_URL}?q={}&rows=20&wt=json",
            percent_encode(solr_query)
        );
        self.get_cached(&url)
            .and_then(|body| parse_search_response(&body).ok())
            .unwrap_or_default()
    }

    /// Every version `repo1.maven.org` has published for `group_id:artifact_id`,
    /// oldest-to-newest as Central's own `maven-metadata.xml` lists them —
    /// D6/D5 do their own semantic ordering, this returns them as reported.
    pub fn versions(&self, group_id: &str, artifact_id: &str) -> Vec<String> {
        if self.offline {
            return Vec::new();
        }
        let group_path = group_id.replace('.', "/");
        let url = format!("{CENTRAL_REPO_URL}/{group_path}/{artifact_id}/maven-metadata.xml");
        self.get_cached(&url)
            .and_then(|body| parse_metadata_versions(&body).ok())
            .unwrap_or_default()
    }

    fn get_cached(&self, url: &str) -> Option<String> {
        let cache_path = cache_path(&self.cache_dir, url);
        if let Some(entry) = read_cache(&cache_path) {
            return entry.body;
        }
        match self.fetch(url) {
            Ok(body) => {
                write_cache(&cache_path, Some(&body));
                Some(body)
            }
            Err(_) => {
                // Review fix #10: a failure is cached too, briefly — a
                // corporate-internal coordinate Central genuinely has no
                // listing for must not be retried on every save.
                write_cache(&cache_path, None);
                None
            }
        }
    }

    /// The one network call in this module — a blocking GET through this
    /// client's own (single, review fix #10) connection pool. Kept thin
    /// deliberately: everything worth unit-testing (parsing, cache
    /// freshness, the offline short-circuit) sits above or below it in
    /// functions that never open a socket.
    fn fetch(&self, url: &str) -> Result<String, CentralError> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|err| CentralError(err.to_string()))?
            .error_for_status()
            .map_err(|err| CentralError(err.to_string()))?;
        response.text().map_err(|err| CentralError(err.to_string()))
    }
}

/// A conservative percent-encoder for a Solr query string embedded in a
/// URL's query component — this module only ever sends coordinates
/// (letters, digits, `.`, `-`, `_`, `:`, `*`), so the safe set is small
/// and deliberately does not try to be a general-purpose URL encoder.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_' | b':' | b'*' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CacheEntry {
    fetched_at_epoch_secs: u64,
    /// `None` is a negative cache entry (review fix #10) — "we asked and
    /// got nothing", valid for [`NEGATIVE_CACHE_TTL`] rather than
    /// [`CACHE_TTL`].
    body: Option<String>,
}

fn cache_path(cache_dir: &Path, url: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    cache_dir.join(format!("{:016x}.json", hasher.finish()))
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache(path: &Path) -> Option<CacheEntry> {
    let raw = std::fs::read_to_string(path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&raw).ok()?;
    let ttl = if entry.body.is_some() {
        CACHE_TTL
    } else {
        NEGATIVE_CACHE_TTL
    };
    let age = now_epoch_secs().saturating_sub(entry.fetched_at_epoch_secs);
    (age <= ttl.as_secs()).then_some(entry)
}

fn write_cache(path: &Path, body: Option<&str>) {
    let entry = CacheEntry {
        fetched_at_epoch_secs: now_epoch_secs(),
        body: body.map(str::to_string),
    };
    let Ok(serialized) = serde_json::to_string(&entry) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, serialized);
}

/// Parses `search.maven.org`'s `solrsearch/select?...&wt=json` response
/// shape: `{"response": {"docs": [{"g": "...", "a": "...",
/// "latestVersion": "..."}, ...]}}`. A doc missing `g` or `a` is dropped
/// rather than failing the whole response — Central's schema has fields
/// this client does not use (`p`, `timestamp`, `versionCount`, …) and a
/// future field addition must not turn into a parse failure here.
pub fn parse_search_response(body: &str) -> Result<Vec<CentralArtifact>, CentralError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|err| CentralError(err.to_string()))?;
    let docs = value
        .get("response")
        .and_then(|r| r.get("docs"))
        .and_then(|d| d.as_array())
        .ok_or_else(|| CentralError("missing response.docs".to_string()))?;
    Ok(docs
        .iter()
        .filter_map(|doc| {
            let group_id = doc.get("g")?.as_str()?.to_string();
            let artifact_id = doc.get("a")?.as_str()?.to_string();
            let latest_version = doc
                .get("latestVersion")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            Some(CentralArtifact {
                group_id,
                artifact_id,
                latest_version,
            })
        })
        .collect())
}

/// Parses `repo1.maven.org/maven2/<g>/<a>/maven-metadata.xml`'s
/// `<versioning><versions><version>…</version>...</versions></versioning>`
/// list. Deliberately a direct event scan rather than reusing
/// `maven::xml`'s tree builder — that module is private to `maven`, and a
/// flat list of `<version>` text nodes needs nothing a tree gives it.
pub fn parse_metadata_versions(body: &str) -> Result<Vec<String>, CentralError> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut versions = Vec::new();
    let mut in_version = false;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|err| CentralError(err.to_string()))?;
        match event {
            Event::Start(tag) if tag.name().as_ref() == b"version" => in_version = true,
            Event::End(tag) if tag.name().as_ref() == b"version" => in_version = false,
            Event::Text(text) if in_version => {
                if let Ok(raw) = text.decode() {
                    versions.push(raw.trim().to_string());
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(versions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_search_response_and_drops_docs_missing_group_or_artifact() {
        let body = r#"{
            "response": {
                "docs": [
                    {"g": "com.google.guava", "a": "guava", "latestVersion": "33.0.0-jre"},
                    {"g": "org.example", "a": "no-latest"},
                    {"a": "missing-group"}
                ]
            }
        }"#;
        let artifacts = parse_search_response(body).unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].group_id, "com.google.guava");
        assert_eq!(artifacts[0].artifact_id, "guava");
        assert_eq!(artifacts[0].latest_version.as_deref(), Some("33.0.0-jre"));
        assert_eq!(artifacts[1].artifact_id, "no-latest");
        assert_eq!(artifacts[1].latest_version, None);
    }

    #[test]
    fn a_malformed_search_response_is_a_typed_error_not_a_panic() {
        assert!(parse_search_response("not json").is_err());
        assert!(parse_search_response(r#"{"nope": true}"#).is_err());
    }

    #[test]
    fn parses_maven_metadata_versions_in_document_order() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>com.google.guava</groupId>
  <artifactId>guava</artifactId>
  <versioning>
    <latest>33.0.0-jre</latest>
    <release>33.0.0-jre</release>
    <versions>
      <version>32.0.0-jre</version>
      <version>32.1.3-jre</version>
      <version>33.0.0-jre</version>
    </versions>
    <lastUpdated>20231107000000</lastUpdated>
  </versioning>
</metadata>
"#;
        let versions = parse_metadata_versions(body).unwrap();
        assert_eq!(versions, vec!["32.0.0-jre", "32.1.3-jre", "33.0.0-jre"]);
    }

    #[test]
    fn malformed_metadata_xml_is_a_typed_error_not_a_panic() {
        // A closing tag that does not match its opening one — quick-xml
        // checks end-tag names by default and errors on the mismatch,
        // unlike a merely unclosed tag at EOF, which it tolerates.
        assert!(parse_metadata_versions("<metadata><versions></wat></metadata>").is_err());
    }

    #[test]
    fn offline_search_and_versions_never_touch_the_network_or_cache() {
        let dir = tempfile::tempdir().unwrap();
        let client = CentralClient::new(dir.path(), true);
        assert!(client.search("g:com.example").is_empty());
        assert!(client.versions("com.example", "lib").is_empty());
        // Nothing was written to the cache directory either.
        assert!(!dir.path().join("cache").exists());
    }

    #[test]
    fn cache_round_trips_within_the_ttl_and_expires_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path(), "https://example.test/x");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_cache(&path, Some("hello"));

        assert_eq!(
            read_cache(&path).and_then(|e| e.body),
            Some("hello".to_string())
        );

        // An entry written far enough in the past (the Unix epoch) is
        // expired under the normal (positive) TTL — written directly
        // rather than via `write_cache`, so the assertion does not
        // depend on real-clock timing at second resolution.
        let stale = CacheEntry {
            fetched_at_epoch_secs: 0,
            body: Some("old".to_string()),
        };
        std::fs::write(&path, serde_json::to_string(&stale).unwrap()).unwrap();
        assert_eq!(read_cache(&path), None);
    }

    /// Review fix #10: a negative entry (no body) round-trips within its
    /// own, much shorter TTL — and is still expired once even that has
    /// passed, the same as a positive entry.
    #[test]
    fn negative_cache_entry_round_trips_within_its_own_shorter_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path(), "https://example.test/nothing-here");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_cache(&path, None);

        let entry = read_cache(&path).expect("fresh negative entry still valid");
        assert_eq!(entry.body, None);

        // Older than the negative TTL (1h) but well inside the positive
        // one (24h) — must still read as expired, proving the negative
        // entry really is on its own shorter clock.
        let stale = CacheEntry {
            fetched_at_epoch_secs: now_epoch_secs().saturating_sub(2 * 60 * 60),
            body: None,
        };
        std::fs::write(&path, serde_json::to_string(&stale).unwrap()).unwrap();
        assert_eq!(read_cache(&path), None);
    }

    #[test]
    fn percent_encode_leaves_coordinate_characters_alone_and_escapes_the_rest() {
        assert_eq!(
            percent_encode("g:com.example-lib_1*"),
            "g:com.example-lib_1*"
        );
        assert_eq!(percent_encode("a b"), "a%20b");
    }
}
