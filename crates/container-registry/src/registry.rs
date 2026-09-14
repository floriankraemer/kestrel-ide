//! The Docker Registry HTTP API V2 client (C7): the bearer-token
//! challenge/response dance, paginated `catalog`/`tags`, and
//! `test_connection` — plus the Docker Hub and GitLab project-registry
//! browsing paths a configured registry's [`RegistryKind`] dispatches to.
//! Blocking `reqwest`, the same TLS stack `hub`/`ai-chat-core` already
//! pull in; every call is meant to run on a worker thread, never the Qt
//! thread.

use std::time::Duration;

use serde::Deserialize;

pub use container_core::registry_ref::RegistryKind;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Why a registry call failed. Classified from `reqwest`'s own error
/// shape and the HTTP status the server sent — never from response body
/// wording, which varies wildly across V2 implementations (the registry
/// distribution reference implementation, GitLab, Harbor, Artifactory,
/// ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// A 401 with no usable `WWW-Authenticate` challenge, or a token
    /// request/retry that itself came back unauthorized — bad or missing
    /// credentials.
    Unauthorized,
    /// A 404 — the registry, repository, or tag does not exist (or, for a
    /// registry that hides its catalog from anonymous callers, is
    /// indistinguishable from one that does not exist).
    NotFound,
    /// Could not reach the server at all: DNS, connection refused, timed
    /// out, reset.
    Network(String),
    /// A TLS handshake/certificate failure — kept apart from `Network`
    /// since "wrong cert" and "wrong host" call for different next steps
    /// in the Settings page's error text.
    Tls(String),
    /// This [`RegistryKind`] does not support the call that was made
    /// (`Generic` is push-only by definition; a `Hub`/`GitLab` call this
    /// client has not implemented).
    Unsupported(RegistryKind),
    /// Anything else: an unexpected status code, a response body that did
    /// not parse as the JSON shape expected.
    Other(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::Unauthorized => write!(f, "the registry rejected these credentials"),
            RegistryError::NotFound => write!(f, "not found"),
            RegistryError::Network(message) => write!(f, "{message}"),
            RegistryError::Tls(message) => write!(f, "TLS error: {message}"),
            RegistryError::Unsupported(kind) => {
                write!(f, "'{}' registries do not support this", kind.id())
            }
            RegistryError::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl RegistryError {
    fn from_reqwest(error: reqwest::Error) -> Self {
        let message = error.to_string();
        let lower = message.to_lowercase();
        if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
            RegistryError::Tls(message)
        } else {
            RegistryError::Network(message)
        }
    }

    fn from_status(status: reqwest::StatusCode) -> Self {
        match status {
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                RegistryError::Unauthorized
            }
            reqwest::StatusCode::NOT_FOUND => RegistryError::NotFound,
            other => RegistryError::Other(format!("registry returned {other}")),
        }
    }
}

/// What `test_connection` reports on success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub address: String,
    pub kind: RegistryKind,
    /// The `Docker-Distribution-Api-Version` response header, when the
    /// server sent one — empty for Hub, which does not.
    pub api_version: String,
}

impl std::fmt::Display for ServerInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.api_version.is_empty() {
            write!(f, "Connected to {}", self.address)
        } else {
            write!(f, "Connected to {} ({})", self.address, self.api_version)
        }
    }
}

/// One page of a paginated listing (`catalog`/`tags`). `next` is the
/// cursor to pass back as `last` for the next page, `None` on the last
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub items: Vec<String>,
    pub next: Option<String>,
}

/// The `WWW-Authenticate: Bearer realm="...",service="...",scope="..."`
/// challenge a V2 registry answers an unauthenticated request with —
/// `docker/distribution`'s token-auth spec, `service`/`scope` optional.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Challenge {
    realm: String,
    service: String,
    scope: String,
}

/// Parse a `WWW-Authenticate` header value into its `Bearer` challenge
/// parameters. `None` for anything that is not a `Bearer` challenge (a
/// bare `Basic realm="..."`, which this client does not attempt) or that
/// has no `realm` at all — a challenge with no realm names nowhere to
/// fetch a token from.
fn parse_www_authenticate(header: &str) -> Option<Challenge> {
    let rest = header.trim().strip_prefix("Bearer")?.trim_start();
    let mut challenge = Challenge::default();
    for param in split_params(rest) {
        let Some((key, value)) = param.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "realm" => challenge.realm = value,
            "service" => challenge.service = value,
            "scope" => challenge.scope = value,
            _ => {}
        }
    }
    (!challenge.realm.is_empty()).then_some(challenge)
}

/// Split on commas that are not inside a quoted value — a bare
/// `.split(',')` breaks `scope="repository:a,b:pull"`, which some
/// registries write as one quoted, comma-joined value covering more than
/// one scope.
fn split_params(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut in_quotes = false;
    let mut start = 0;
    for (index, ch) in input.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                parts.push(input[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(input[start..].trim());
    parts.into_iter().filter(|part| !part.is_empty()).collect()
}

#[derive(Deserialize, Default)]
struct TokenResponse {
    token: Option<String>,
    access_token: Option<String>,
}

impl TokenResponse {
    fn into_value(self) -> Option<String> {
        self.token.or(self.access_token)
    }
}

#[derive(Deserialize, Default)]
struct CatalogResponse {
    #[serde(default)]
    repositories: Vec<String>,
}

#[derive(Deserialize, Default)]
struct TagsResponse {
    #[serde(default)]
    tags: Vec<String>,
}

/// Percent-decode a query-parameter value — only what a `Link` header's
/// `last=` cursor needs (a repository path can contain `/`, which some
/// servers percent-encode in the cursor they hand back). Stdlib only:
/// `reqwest` pulls in a URL parser transitively, but not as a dependency
/// this crate can use directly.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-encode a value for use in a query string — the inverse of
/// [`percent_decode`], covering the same "everything but the safe set"
/// rule, applied to `last=<cursor>` when it is sent back on the next page.
fn percent_encode(input: &str) -> String {
    input
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/' | b':')
            {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The `last` cursor from a `Link: <.../_catalog?n=20&last=foo>;
/// rel="next"` response header — `None` when there is no `rel="next"`
/// link (the last page).
fn next_cursor(link_header: &str) -> Option<String> {
    for part in link_header.split(',') {
        let part = part.trim();
        if !part.contains("rel=\"next\"") && !part.contains("rel=next") {
            continue;
        }
        let start = part.find('<')? + 1;
        let end = part[start..].find('>')? + start;
        let url = &part[start..end];
        let query = url.split_once('?').map(|(_, query)| query).unwrap_or("");
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("last=") {
                return Some(percent_decode(value));
            }
        }
    }
    None
}

/// A registry connection: address, kind, and the credentials to use if a
/// call comes back unauthorized. Built once per call from a configured
/// `app_config::RegistrySetting` plus its keychain secret — cheap (one
/// `reqwest::blocking::Client`), so nothing here is cached across calls.
pub struct RegistryClient {
    address: String,
    kind: RegistryKind,
    username: String,
    secret: String,
    gitlab_project: String,
    client: reqwest::blocking::Client,
}

impl RegistryClient {
    pub fn new(
        address: &str,
        kind: RegistryKind,
        username: &str,
        secret: &str,
        gitlab_project: &str,
    ) -> Result<Self, RegistryError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(TIMEOUT)
            .user_agent("ide-containers")
            .build()
            .map_err(|error| RegistryError::Other(error.to_string()))?;
        Ok(RegistryClient {
            address: address.trim_matches('/').to_string(),
            kind,
            username: username.to_string(),
            secret: secret.to_string(),
            gitlab_project: gitlab_project.to_string(),
            client,
        })
    }

    /// `address` as given when it already names a scheme (an internal V2
    /// registry served over plain HTTP, or a test server) — `https://`
    /// prepended otherwise, since every public registry this dials is TLS.
    fn base_url(&self) -> String {
        if self.address.starts_with("http://") || self.address.starts_with("https://") {
            self.address.clone()
        } else {
            format!("https://{}", self.address)
        }
    }

    fn checked(
        response: reqwest::blocking::Response,
    ) -> Result<reqwest::blocking::Response, RegistryError> {
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(RegistryError::from_status(response.status()))
        }
    }

    /// `GET realm?service=..&scope=..`, Basic auth when credentials are
    /// configured — the token half of the V2 dance.
    fn fetch_token(&self, challenge: &Challenge) -> Result<String, RegistryError> {
        let mut query = Vec::new();
        if !challenge.service.is_empty() {
            query.push(("service", challenge.service.as_str()));
        }
        if !challenge.scope.is_empty() {
            query.push(("scope", challenge.scope.as_str()));
        }
        let mut request = self.client.get(&challenge.realm).query(&query);
        if !self.username.is_empty() {
            request = request.basic_auth(&self.username, Some(&self.secret));
        }
        let response = Self::checked(request.send().map_err(RegistryError::from_reqwest)?)?;
        let body = response.text().map_err(RegistryError::from_reqwest)?;
        let parsed: TokenResponse = serde_json::from_str(&body).map_err(|error| {
            RegistryError::Other(format!("token response was not valid JSON: {error}"))
        })?;
        parsed.into_value().ok_or_else(|| {
            RegistryError::Other("token response had neither token nor access_token".to_string())
        })
    }

    /// `GET <base>/<path_and_query>`: on a 401 with a `Bearer` challenge,
    /// fetch a token ([`Self::fetch_token`]) and retry once with it; any
    /// other 401 (no challenge at all) is [`RegistryError::Unauthorized`]
    /// directly, no retry.
    fn v2_get(&self, path_and_query: &str) -> Result<reqwest::blocking::Response, RegistryError> {
        let url = format!("{}{path_and_query}", self.base_url());
        let response = self
            .client
            .get(&url)
            .send()
            .map_err(RegistryError::from_reqwest)?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Self::checked(response);
        }
        let challenge = response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_www_authenticate);
        let Some(challenge) = challenge else {
            return Err(RegistryError::Unauthorized);
        };
        let token = self.fetch_token(&challenge)?;
        let retried = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .map_err(RegistryError::from_reqwest)?;
        Self::checked(retried)
    }

    /// Confirm the registry is reachable (and, when credentials are
    /// configured, that they are accepted) without listing anything.
    pub fn test_connection(&self) -> Result<ServerInfo, RegistryError> {
        match self.kind {
            RegistryKind::Generic => Err(RegistryError::Unsupported(RegistryKind::Generic)),
            RegistryKind::Hub => self.hub_test_connection(),
            RegistryKind::GitLab | RegistryKind::DockerV2 => {
                let response = self.v2_get("/v2/")?;
                let api_version = response
                    .headers()
                    .get("Docker-Distribution-Api-Version")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                Ok(ServerInfo {
                    address: self.address.clone(),
                    kind: self.kind,
                    api_version,
                })
            }
        }
    }

    fn hub_test_connection(&self) -> Result<ServerInfo, RegistryError> {
        if !self.username.is_empty() {
            let body = serde_json::json!({ "username": self.username, "password": self.secret });
            let response = self
                .client
                .post("https://hub.docker.com/v2/users/login/")
                .json(&body)
                .send()
                .map_err(RegistryError::from_reqwest)?;
            Self::checked(response)?;
        }
        Ok(ServerInfo {
            address: "docker.io".to_string(),
            kind: RegistryKind::Hub,
            api_version: String::new(),
        })
    }

    /// List up to `n` repositories, from `last` (the previous page's
    /// [`Page::next`], `None` for the first page). `Generic` registries
    /// answer [`RegistryError::Unsupported`] — they are push-only by
    /// definition.
    pub fn catalog(&self, n: u32, last: Option<&str>) -> Result<Page, RegistryError> {
        match self.kind {
            RegistryKind::Generic => Err(RegistryError::Unsupported(RegistryKind::Generic)),
            RegistryKind::Hub => self.hub_catalog(n, last),
            RegistryKind::GitLab if !self.gitlab_project.is_empty() => self.gitlab_catalog(),
            RegistryKind::GitLab | RegistryKind::DockerV2 => self.v2_catalog(n, last),
        }
    }

    fn v2_catalog(&self, n: u32, last: Option<&str>) -> Result<Page, RegistryError> {
        let mut query = format!("/v2/_catalog?n={n}");
        if let Some(last) = last {
            query.push_str(&format!("&last={}", percent_encode(last)));
        }
        let response = self.v2_get(&query)?;
        let next = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|value| value.to_str().ok())
            .and_then(next_cursor);
        let body = response.text().map_err(RegistryError::from_reqwest)?;
        let parsed: CatalogResponse = serde_json::from_str(&body).map_err(|error| {
            RegistryError::Other(format!("catalog response was not valid JSON: {error}"))
        })?;
        Ok(Page {
            items: parsed.repositories,
            next,
        })
    }

    fn hub_catalog(&self, n: u32, last: Option<&str>) -> Result<Page, RegistryError> {
        let namespace = if self.username.is_empty() {
            "library"
        } else {
            &self.username
        };
        let page = last
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        let url = format!(
            "https://hub.docker.com/v2/repositories/{namespace}/?page_size={n}&page={page}"
        );
        let response = Self::checked(
            self.client
                .get(&url)
                .send()
                .map_err(RegistryError::from_reqwest)?,
        )?;
        let body = response.text().map_err(RegistryError::from_reqwest)?;
        #[derive(Deserialize, Default)]
        struct HubRepoRow {
            name: String,
        }
        #[derive(Deserialize, Default)]
        struct HubReposResponse {
            #[serde(default)]
            results: Vec<HubRepoRow>,
            next: Option<String>,
        }
        let parsed: HubReposResponse = serde_json::from_str(&body).map_err(|error| {
            RegistryError::Other(format!("Docker Hub returned an unexpected answer: {error}"))
        })?;
        Ok(Page {
            items: parsed
                .results
                .into_iter()
                .map(|row| format!("{namespace}/{}", row.name))
                .collect(),
            next: parsed.next.map(|_| (page + 1).to_string()),
        })
    }

    /// GitLab's own `GET /api/v4/projects/:id/registry/repositories` —
    /// used only when `gitlab_project` is configured; [`Self::catalog`]
    /// falls back to [`Self::v2_catalog`] otherwise, since most GitLab
    /// tokens cannot read the instance-wide V2 catalog at all.
    fn gitlab_catalog(&self) -> Result<Page, RegistryError> {
        let project = percent_encode(&self.gitlab_project);
        let host = self.address.split('/').next().unwrap_or(&self.address);
        let url = format!("https://{host}/api/v4/projects/{project}/registry/repositories");
        let mut request = self.client.get(&url);
        if !self.secret.is_empty() {
            request = request.header("PRIVATE-TOKEN", &self.secret);
        }
        let response = Self::checked(request.send().map_err(RegistryError::from_reqwest)?)?;
        let body = response.text().map_err(RegistryError::from_reqwest)?;
        #[derive(Deserialize, Default)]
        struct GitLabRepo {
            path: String,
        }
        let parsed: Vec<GitLabRepo> = serde_json::from_str(&body).map_err(|error| {
            RegistryError::Other(format!("GitLab returned an unexpected answer: {error}"))
        })?;
        Ok(Page {
            items: parsed.into_iter().map(|repo| repo.path).collect(),
            next: None,
        })
    }

    /// List up to `n` tags of `repository`, from `last` the same way
    /// [`Self::catalog`] pages.
    pub fn tags(
        &self,
        repository: &str,
        n: u32,
        last: Option<&str>,
    ) -> Result<Page, RegistryError> {
        match self.kind {
            RegistryKind::Generic => Err(RegistryError::Unsupported(RegistryKind::Generic)),
            RegistryKind::Hub => self.hub_tags(repository),
            RegistryKind::GitLab | RegistryKind::DockerV2 => self.v2_tags(repository, n, last),
        }
    }

    fn v2_tags(&self, repository: &str, n: u32, last: Option<&str>) -> Result<Page, RegistryError> {
        let mut query = format!("/v2/{repository}/tags/list?n={n}");
        if let Some(last) = last {
            query.push_str(&format!("&last={}", percent_encode(last)));
        }
        let response = self.v2_get(&query)?;
        let next = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|value| value.to_str().ok())
            .and_then(next_cursor);
        let body = response.text().map_err(RegistryError::from_reqwest)?;
        let parsed: TagsResponse = serde_json::from_str(&body).map_err(|error| {
            RegistryError::Other(format!("tags response was not valid JSON: {error}"))
        })?;
        Ok(Page {
            items: parsed.tags,
            next,
        })
    }

    fn hub_tags(&self, repository: &str) -> Result<Page, RegistryError> {
        let repository = if repository.contains('/') {
            repository.to_string()
        } else {
            format!("library/{repository}")
        };
        crate::hub::tags(&repository)
            .map(|tags| Page {
                items: tags,
                next: None,
            })
            .map_err(RegistryError::Other)
    }
}

#[cfg(test)]
mod stub_server {
    //! A tiny in-process HTTP/1.1 server for exercising [`super::
    //! RegistryClient`] against real `reqwest` requests without a real
    //! registry — `std::net::TcpListener` only, no new dev-dependency
    //! (this task's own instruction).

    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// One canned response, matched by request path (query string
    /// ignored: `reqwest`'s own percent-encoding of query values is not
    /// worth pinning down exactly in a route table).
    pub struct Route {
        pub path: &'static str,
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: String,
    }

    pub struct StubServer {
        pub port: u16,
        pub requests: Arc<Mutex<Vec<String>>>,
        stop: mpsc::Sender<()>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl Drop for StubServer {
        fn drop(&mut self) {
            let _ = self.stop.send(());
            // Unblock the accept loop's next `.next()` so it can see the
            // stop signal and return.
            let _ = TcpStream::connect(("127.0.0.1", self.port));
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    /// Bind an ephemeral port without serving it yet — so a caller can
    /// read the port (to bake a self-referential `WWW-Authenticate` realm
    /// into a route's body) before [`serve`] hands the listener off to the
    /// server thread.
    pub fn bind() -> TcpListener {
        TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port")
    }

    pub fn spawn(routes: Vec<Route>) -> StubServer {
        serve(bind(), routes)
    }

    pub fn serve(listener: TcpListener, routes: Vec<Route>) -> StubServer {
        let port = listener.local_addr().expect("local addr").port();
        let (stop_tx, stop_rx) = mpsc::channel();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_thread = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_rx.try_recv().is_ok() {
                    return;
                }
                let Ok(mut stream) = stream else { continue };
                handle_one(&mut stream, &routes, &requests_for_thread);
            }
        });
        StubServer {
            port,
            requests,
            stop: stop_tx,
            handle: Some(handle),
        }
    }

    fn handle_one(stream: &mut TcpStream, routes: &[Route], requests: &Arc<Mutex<Vec<String>>>) {
        let mut buf = [0u8; 8192];
        let read = match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let request = String::from_utf8_lossy(&buf[..read]).into_owned();
        let head = request
            .split("\r\n\r\n")
            .next()
            .unwrap_or(&request)
            .to_string();
        requests.lock().unwrap().push(head.clone());

        let Some(first_line) = request.lines().next() else {
            return;
        };
        let raw_path = first_line.split_whitespace().nth(1).unwrap_or("/");
        let path = raw_path.split('?').next().unwrap_or(raw_path);
        let route = routes.iter().find(|route| route.path == path);

        let (status, headers, body): (u16, Vec<(String, String)>, String) = match route {
            Some(route) => (route.status, route.headers.clone(), route.body.clone()),
            None => (404, Vec::new(), String::new()),
        };
        let mut response = format!(
            "HTTP/1.1 {status} {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            reason_phrase(status),
            body.len()
        );
        for (key, value) in &headers {
            response.push_str(&format!("{key}: {value}\r\n"));
        }
        response.push_str("\r\n");
        response.push_str(&body);
        let _ = stream.write_all(response.as_bytes());
    }

    fn reason_phrase(status: u16) -> &'static str {
        match status {
            200 => "OK",
            401 => "Unauthorized",
            404 => "Not Found",
            _ => "Error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stub_server::{spawn, Route};
    use super::*;

    #[test]
    fn www_authenticate_parses_realm_service_and_scope() {
        let challenge = parse_www_authenticate(
            r#"Bearer realm="https://auth.example.com/token",service="registry.example.com",scope="repository:acme/app:pull""#,
        )
        .unwrap();
        assert_eq!(challenge.realm, "https://auth.example.com/token");
        assert_eq!(challenge.service, "registry.example.com");
        assert_eq!(challenge.scope, "repository:acme/app:pull");
    }

    #[test]
    fn www_authenticate_handles_a_comma_inside_a_quoted_scope() {
        let challenge = parse_www_authenticate(
            r#"Bearer realm="https://auth.example.com/token",service="r",scope="repository:a:pull,repository:b:pull""#,
        )
        .unwrap();
        assert_eq!(challenge.scope, "repository:a:pull,repository:b:pull");
    }

    #[test]
    fn www_authenticate_allows_missing_service_and_scope() {
        let challenge =
            parse_www_authenticate(r#"Bearer realm="https://auth.example.com/token""#).unwrap();
        assert_eq!(challenge.realm, "https://auth.example.com/token");
        assert_eq!(challenge.service, "");
        assert_eq!(challenge.scope, "");
    }

    #[test]
    fn www_authenticate_rejects_non_bearer_and_realm_less_challenges() {
        assert!(parse_www_authenticate(r#"Basic realm="x""#).is_none());
        assert!(parse_www_authenticate("Bearer service=\"x\"").is_none());
        assert!(parse_www_authenticate("garbage").is_none());
    }

    #[test]
    fn next_cursor_reads_the_last_query_parameter_off_a_rel_next_link() {
        assert_eq!(
            next_cursor(r#"</v2/_catalog?n=2&last=b>; rel="next""#),
            Some("b".to_string())
        );
        assert_eq!(next_cursor(r#"</v2/_catalog?n=2>; rel="prev""#), None);
        assert_eq!(next_cursor(""), None);
    }

    #[test]
    fn next_cursor_percent_decodes_the_cursor() {
        assert_eq!(
            next_cursor(r#"</v2/_catalog?n=2&last=acme%2Fapp>; rel="next""#),
            Some("acme/app".to_string())
        );
    }

    #[test]
    fn from_status_classifies_401_403_404_and_falls_back_to_other() {
        assert_eq!(
            RegistryError::from_status(reqwest::StatusCode::UNAUTHORIZED),
            RegistryError::Unauthorized
        );
        assert_eq!(
            RegistryError::from_status(reqwest::StatusCode::FORBIDDEN),
            RegistryError::Unauthorized
        );
        assert_eq!(
            RegistryError::from_status(reqwest::StatusCode::NOT_FOUND),
            RegistryError::NotFound
        );
        assert!(matches!(
            RegistryError::from_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            RegistryError::Other(_)
        ));
    }

    #[test]
    fn generic_registries_refuse_every_browse_call() {
        let client = RegistryClient::new("example.com", RegistryKind::Generic, "", "", "").unwrap();
        assert_eq!(
            client.test_connection(),
            Err(RegistryError::Unsupported(RegistryKind::Generic))
        );
        assert_eq!(
            client.catalog(20, None),
            Err(RegistryError::Unsupported(RegistryKind::Generic))
        );
        assert_eq!(
            client.tags("acme/app", 20, None),
            Err(RegistryError::Unsupported(RegistryKind::Generic))
        );
    }

    #[test]
    fn tags_pagination_reads_the_link_header_cursor() {
        let server = spawn(vec![Route {
            path: "/v2/acme/app/tags/list",
            status: 200,
            headers: vec![(
                "Link".to_string(),
                r#"</v2/acme/app/tags/list?n=1&last=v1>; rel="next""#.to_string(),
            )],
            body: r#"{"tags":["v1"]}"#.to_string(),
        }]);
        let client = RegistryClient::new(
            &format!("http://127.0.0.1:{}", server.port),
            RegistryKind::DockerV2,
            "",
            "",
            "",
        )
        .unwrap();
        let page = client.tags("acme/app", 1, None).unwrap();
        assert_eq!(page.items, vec!["v1".to_string()]);
        assert_eq!(page.next, Some("v1".to_string()));
    }

    #[test]
    fn catalog_without_any_challenge_is_a_plain_200() {
        let server = spawn(vec![Route {
            path: "/v2/_catalog",
            status: 200,
            headers: Vec::new(),
            body: r#"{"repositories":["acme/app","acme/web"]}"#.to_string(),
        }]);
        let client = RegistryClient::new(
            &format!("http://127.0.0.1:{}", server.port),
            RegistryKind::DockerV2,
            "",
            "",
            "",
        )
        .unwrap();
        let page = client.catalog(20, None).unwrap();
        assert_eq!(
            page.items,
            vec!["acme/app".to_string(), "acme/web".to_string()]
        );
        assert_eq!(page.next, None);
    }

    #[test]
    fn a_401_with_no_challenge_header_is_unauthorized_without_retrying() {
        let server = spawn(vec![Route {
            path: "/v2/_catalog",
            status: 401,
            headers: Vec::new(),
            body: String::new(),
        }]);
        let client = RegistryClient::new(
            &format!("http://127.0.0.1:{}", server.port),
            RegistryKind::DockerV2,
            "",
            "",
            "",
        )
        .unwrap();
        assert_eq!(client.catalog(20, None), Err(RegistryError::Unauthorized));
    }

    #[test]
    fn a_full_401_challenge_token_200_round_trip_reaches_the_catalog() {
        // Bind first so the server's own port can be baked into the
        // `WWW-Authenticate` realm it will serve — the challenge points
        // back at this same stub server's `/token` path.
        let listener = super::stub_server::bind();
        let port = listener.local_addr().unwrap().port();
        let realm = format!("http://127.0.0.1:{port}/token");
        let server = super::stub_server::serve(
            listener,
            vec![
                Route {
                    path: "/v2/_catalog",
                    status: 401,
                    headers: vec![(
                        "WWW-Authenticate".to_string(),
                        format!(
                            r#"Bearer realm="{realm}",service="registry.example",scope="registry:catalog:*""#
                        ),
                    )],
                    body: String::new(),
                },
                Route {
                    path: "/token",
                    status: 200,
                    headers: Vec::new(),
                    body: r#"{"token":"abc123"}"#.to_string(),
                },
            ],
        );
        let client = RegistryClient::new(
            &format!("http://127.0.0.1:{port}"),
            RegistryKind::DockerV2,
            "alice",
            "s3cr3t",
            "",
        )
        .unwrap();
        // The `/v2/_catalog` route always answers 401 (there is no
        // authenticated route for it in this fixture), so the call still
        // fails — what this test asserts is that the retry *happened*
        // with the right token, not that it then succeeded.
        assert_eq!(client.catalog(20, None), Err(RegistryError::Unauthorized));

        let requests = server.requests.lock().unwrap();
        let token_request = requests
            .iter()
            .find(|request| request.starts_with("GET /token"))
            .expect("a token request was made");
        assert!(token_request.contains("service=registry.example"));
        assert!(token_request.contains("scope="), "{token_request}");
        assert!(token_request.contains("catalog"), "{token_request}");
        assert!(token_request
            .to_lowercase()
            .contains("authorization: basic"));

        let catalog_requests = requests
            .iter()
            .filter(|request| request.starts_with("GET /v2/_catalog"))
            .count();
        assert_eq!(
            catalog_requests, 2,
            "the 401 first, then the bearer-authenticated retry"
        );
    }
}
