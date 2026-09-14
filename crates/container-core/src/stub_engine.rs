//! C10: the pure, testable half of `stub_engine` — a fake `docker`/`podman`
//! CLI so `e2e_containers` (`crates/app/tests/e2e_containers.rs`) needs no
//! real engine installed, the same role `lsp_core::bin::stub_server` and
//! `analysis_core::bin::stub_analyzer` play for their own E2E flows.
//!
//! Split from `crates/container-core/src/bin/stub_engine.rs` on review: a
//! `[[bin]]`'s own lines are invisible to `cargo llvm-cov`'s coverage
//! report (only the E2E flow ever runs it, and E2E is excluded from
//! coverage by design — `COVERAGE_EXCLUDES` in the `Makefile`), so every
//! rule of "what argv answers what" belongs here instead, unit-tested the
//! same way every other rule in this crate is. The bin becomes a thin
//! shell: read env vars, call [`StubData::load`] and [`respond`], print
//! the result, handle blocking or exit.
//!
//! [`respond`] takes no environment, does no I/O and never sleeps — every
//! canned answer already sits in the [`StubData`] the caller loaded once,
//! and the one genuinely time-based behaviour (`events`' optional delayed
//! line) is expressed as data (`StubResponse::defer`) for the caller to
//! act on, never as a `thread::sleep` in here.
//!
//! Two environment variables drive the binary that wraps this module:
//!
//! - `IDE_STUB_ENGINE_DATA`: the directory canned answers come from.
//!   Defaults to `container-core`'s own `testdata/` directory — the E2E
//!   fixture reuses the exact same `inspect/{docker,podman}` and
//!   `probe/*_version.json` files the unit tests already exercise, rather
//!   than a parallel copy, exactly as the C10 plan entry asks for.
//! - `IDE_STUB_ENGINE_LOG`: a file the binary appends one [`log_line`] to
//!   per invocation, so a test can assert exactly what `container-core`
//!   shelled out to.
//!
//! Called as `docker` or `podman` (a symlink/copy the test sets up on a
//! scratch `PATH` entry): `argv[0]`'s basename ([`engine_name`]) picks
//! which engine's canned subdirectory answers `ps -aq`/`inspect`/etc, so
//! one binary serves both of the fixture's connections.
//!
//! Known ceiling: every canned answer is static, keyed only by resource
//! kind and (for inspect) the ids the caller actually asked for — good
//! enough for the read-mostly E2E flows this backs (dock tree, Start,
//! Log tab, run-config preview, gutter/lens derivation), not a real
//! engine's state machine. `start`/`stop`/etc. don't mutate anything a
//! later `ps -aq` would reflect; a test that needs that would have to grow
//! this stub first.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

/// One engine's preloaded canned answers — read once by [`StubData::load`]
/// so [`respond`] itself touches no filesystem.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StubData {
    /// `"docker"` or `"podman"` — which subdirectory of `inspect/` and
    /// which `probe/*_version.json`/`history/*` file this data came from.
    pub engine: String,
    pub version_json: Option<String>,
    pub containers: Vec<Value>,
    pub images: Vec<Value>,
    pub volumes: Vec<Value>,
    pub networks: Vec<Value>,
    /// `pod ls --format json`'s raw answer — podman only.
    pub pods_json: Option<String>,
    /// The first line of `inspect/<engine>/events.jsonl`, if any — what
    /// `events` prints after its optional delay.
    pub events_first_line: Option<String>,
    pub top_text: Option<String>,
    pub logs_text: Option<String>,
    /// `exec <id> ls -la ...`'s canned answer (the Files tab).
    pub files_ls_text: Option<String>,
    /// `history`'s raw answer — already the right shape for `engine`
    /// (Docker's NDJSON vs. Podman's JSON array), so `respond` picks
    /// nothing here, only prints it.
    pub history_text: Option<String>,
    pub context_ls_text: Option<String>,
    /// `machine list --format json` — podman only.
    pub machine_list_text: Option<String>,
    pub compose_services_text: Option<String>,
    pub compose_config_text: Option<String>,
}

impl StubData {
    /// Read every canned file `respond` might need for `engine` out of
    /// `data_dir` — a missing file just leaves its field `None`/empty
    /// rather than failing the load, the same "answer generically instead
    /// of refusing" rule the old per-request reads followed.
    pub fn load(data_dir: &Path, engine: &str) -> StubData {
        let inspect_dir = data_dir.join("inspect").join(engine);
        StubData {
            engine: engine.to_string(),
            version_json: read_to_string(
                &data_dir
                    .join("probe")
                    .join(format!("{engine}_version.json")),
            ),
            containers: read_json_array(&inspect_dir.join("containers.json")),
            images: read_json_array(&inspect_dir.join("images.json")),
            volumes: read_json_array(&inspect_dir.join("volumes.json")),
            networks: read_json_array(&inspect_dir.join("networks.json")),
            pods_json: (engine == "podman")
                .then(|| read_to_string(&inspect_dir.join("pods.json")))
                .flatten(),
            events_first_line: read_to_string(&inspect_dir.join("events.jsonl"))
                .and_then(|text| text.lines().next().map(str::to_string)),
            top_text: read_to_string(&data_dir.join("top.txt")),
            logs_text: read_to_string(&data_dir.join("logs/lines.txt")),
            files_ls_text: read_to_string(&data_dir.join("files/ls.txt")),
            history_text: read_to_string(&data_dir.join(if engine == "podman" {
                "history/podman.json"
            } else {
                "history/docker.jsonl"
            })),
            context_ls_text: read_to_string(&data_dir.join("discovery/docker_context_ls.jsonl")),
            machine_list_text: (engine == "podman")
                .then(|| read_to_string(&data_dir.join("machine/list.json")))
                .flatten(),
            compose_services_text: read_to_string(&data_dir.join("compose/services.txt")),
            compose_config_text: read_to_string(&data_dir.join("compose/config.json")),
        }
    }
}

fn read_json_array(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(items)) => items,
        Ok(Value::Null) => Vec::new(),
        Ok(other) => vec![other],
        Err(_) => Vec::new(),
    }
}

fn read_to_string(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// What [`respond`] decided: what to print, what to exit with, and whether
/// the caller must then block (a real `logs -f`/`events`/`attach`/`compose
/// up` never returns on its own).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StubResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    /// The caller sleeps/waits for a kill signal instead of exiting.
    pub blocks: bool,
    /// `events` only: print this line after this delay, then keep
    /// blocking — data, not an action, so `respond` still sleeps nothing
    /// itself; the caller (which owns real time) acts on it.
    pub defer: Option<(Duration, String)>,
}

impl StubResponse {
    fn ok(stdout: impl Into<String>) -> StubResponse {
        StubResponse {
            stdout: stdout.into(),
            exit_code: 0,
            ..Default::default()
        }
    }

    fn blocking(stdout: impl Into<String>) -> StubResponse {
        StubResponse {
            stdout: stdout.into(),
            exit_code: 0,
            blocks: true,
            ..Default::default()
        }
    }
}

/// `argv[0]`'s basename, `.exe` stripped, lowercased — `"podman"` if that's
/// what it contains, `"docker"` for anything else (including a plain
/// `stub_engine` run with no engine-shaped name, so `cargo run --bin
/// stub_engine -- version --format json` works for a quick manual check).
pub fn engine_name(program: &str) -> String {
    let base = Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("docker")
        .to_lowercase();
    if base.contains("podman") {
        "podman".to_string()
    } else {
        "docker".to_string()
    }
}

/// One `IDE_STUB_ENGINE_LOG` line: `{"argv":[...]}`, the *whole* argv
/// (program name included) so a test can tell `docker` and `podman`
/// invocations apart without re-deriving [`engine_name`] itself.
pub fn log_line(argv: &[String]) -> String {
    serde_json::json!({ "argv": argv }).to_string()
}

/// Docker/Podman's own short id: 12 hex characters, `sha256:` stripped —
/// what `ps -aq`/`image ls -q` print, and what a real caller's `inspect`
/// argv then carries back in here.
fn short_id(id: &str) -> String {
    id.strip_prefix("sha256:")
        .unwrap_or(id)
        .chars()
        .take(12)
        .collect()
}

fn str_field<'a>(entry: &'a Value, field: &str) -> &'a str {
    entry.get(field).and_then(Value::as_str).unwrap_or("")
}

/// Every non-flag token in `args` — the id list for `start`/`stop`/`rm`/
/// `inspect`/etc, whose only flags are boolean (`-f`, `-it`) or, for
/// `exec`'s `-u 0`, a flag-then-value pair this also skips.
fn ids_only(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "-u" {
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn ids_stdout(entries: &[Value], field: &str) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut out = String::new();
    for entry in entries {
        let id = short_id(str_field(entry, field));
        if id.is_empty() || seen.contains(&id) {
            continue;
        }
        out.push_str(&id);
        out.push('\n');
        seen.push(id);
    }
    out
}

/// `inspect`'s answer: every entry whose `field` (prefix-matched, the way
/// a short id matches a full one) is one of `requested`. Unmatched
/// requested ids are silently dropped rather than erroring — good enough
/// for a stub nothing here asks to fail this way.
fn inspect_stdout(entries: &[Value], field: &str, requested: &[String]) -> String {
    let matches: Vec<&Value> = entries
        .iter()
        .filter(|entry| {
            let value = str_field(entry, field);
            requested
                .iter()
                .any(|id| value.starts_with(id.as_str()) || id.starts_with(&short_id(value)))
        })
        .collect();
    format!("{}\n", serde_json::to_string(&matches).unwrap_or_default())
}

fn engine_error(engine: &str, subcommand: &str) -> StubResponse {
    StubResponse {
        stderr: format!(
            "{engine}: '{subcommand}' is not a {engine} command.\nSee '{engine} --help'.\n"
        ),
        exit_code: 1,
        ..Default::default()
    }
}

/// The whole decision: given `data` (already loaded for the right engine)
/// and the argv the caller was invoked with (program name excluded —
/// [`engine_name`] already used that to pick `data`), what to print, exit
/// with, or block on. `event_after_ms` is `events`' own delay, read from
/// `IDE_STUB_ENGINE_EVENT_AFTER_MS` by the caller and handed in as a plain
/// value — this function never reads the environment itself.
pub fn respond(args: &[String], data: &StubData, event_after_ms: Option<Duration>) -> StubResponse {
    let engine = data.engine.as_str();
    let first = args.first().map(String::as_str).unwrap_or("");
    let second = args.get(1).map(String::as_str).unwrap_or("");

    match (first, second) {
        ("version", _) if args.iter().any(|a| a == "--format") => match &data.version_json {
            Some(text) => StubResponse::ok(text.clone()),
            None => engine_error(engine, "version"),
        },
        ("ps", "-aq") => StubResponse::ok(ids_stdout(&data.containers, "Id")),
        ("container", "inspect") => StubResponse::ok(inspect_stdout(
            &data.containers,
            "Id",
            &ids_only(&args[2..]),
        )),
        ("image", "ls") => StubResponse::ok(ids_stdout(&data.images, "Id")),
        ("image", "inspect") => {
            StubResponse::ok(inspect_stdout(&data.images, "Id", &ids_only(&args[2..])))
        }
        ("volume", "ls") => StubResponse::ok(ids_stdout(&data.volumes, "Name")),
        ("volume", "inspect") => {
            StubResponse::ok(inspect_stdout(&data.volumes, "Name", &ids_only(&args[2..])))
        }
        ("network", "ls") => StubResponse::ok(ids_stdout(&data.networks, "Id")),
        ("network", "inspect") => {
            StubResponse::ok(inspect_stdout(&data.networks, "Id", &ids_only(&args[2..])))
        }
        ("pod", "ls") if engine == "podman" => StubResponse::ok(format!(
            "{}\n",
            data.pods_json.clone().unwrap_or_else(|| "[]".to_string())
        )),
        ("events", _) => StubResponse {
            blocks: true,
            defer: event_after_ms.and_then(|delay| {
                data.events_first_line
                    .clone()
                    .map(|line| (delay, format!("{line}\n")))
            }),
            ..Default::default()
        },
        ("start", _) | ("stop", _) | ("restart", _) | ("pause", _) | ("unpause", _) => {
            StubResponse::ok(ids_lines(&ids_only(&args[1..])))
        }
        ("rm", _) => StubResponse::ok(ids_lines(&ids_only(&args[1..]))),
        ("container", "prune")
        | ("image", "prune")
        | ("network", "prune")
        | ("volume", "prune") => StubResponse::ok("Total reclaimed space: 0B\n"),
        ("top", _) => StubResponse::ok(data.top_text.clone().unwrap_or_else(|| {
            "PID   USER   TIME   COMMAND\n1     root   0:00   sh\n".to_string()
        })),
        ("logs", _) => StubResponse::blocking(data.logs_text.clone().unwrap_or_default()),
        ("attach", _) => StubResponse::blocking(""),
        ("exec", _) => respond_exec(data, &args[1..]),
        ("pull", _) => respond_pull(args.get(1).map(String::as_str).unwrap_or_default()),
        ("tag", _) | ("rmi", _) | ("save", _) | ("load", _) => StubResponse::ok(""),
        ("history", _) => StubResponse::ok(data.history_text.clone().unwrap_or_default()),
        ("context", "ls") => StubResponse::ok(data.context_ls_text.clone().unwrap_or_default()),
        ("machine", "list") if engine == "podman" => StubResponse::ok(format!(
            "{}\n",
            data.machine_list_text
                .clone()
                .unwrap_or_else(|| "[]".to_string())
        )),
        ("machine", "start") | ("machine", "stop") if engine == "podman" => StubResponse::ok(""),
        ("compose", _) => respond_compose(data, args),
        _ => engine_error(engine, first),
    }
}

fn ids_lines(ids: &[String]) -> String {
    let mut out = String::new();
    for id in ids {
        out.push_str(id);
        out.push('\n');
    }
    out
}

/// `-it`/`-u 0` are `exec`'s own flags, ahead of the id; the command that
/// follows keeps its own flags (`-la`) untouched — [`ids_only`] would strip
/// those too, so this walks `rest` itself rather than reusing it.
fn respond_exec(data: &StubData, rest: &[String]) -> StubResponse {
    let mut iter = rest.iter();
    let mut id = None;
    while let Some(token) = iter.next() {
        if token == "-u" {
            iter.next();
            continue;
        }
        if token.starts_with('-') {
            continue;
        }
        id = Some(token);
        break;
    }
    let command: Vec<&str> = iter.map(String::as_str).collect();
    // `ls -la ...` (the Files tab) gets a canned listing; anything else
    // (the Terminal/Exec shell probe) just succeeds with no output, since
    // nothing in this suite drives an actual interactive shell through the
    // stub.
    if id.is_some() && command.first() == Some(&"ls") {
        StubResponse::ok(
            data.files_ls_text
                .clone()
                .unwrap_or_else(|| "total 0\n".to_string()),
        )
    } else {
        StubResponse::ok("")
    }
}

fn respond_pull(reference: &str) -> StubResponse {
    StubResponse::ok(format!(
        "Using default tag: latest\n\
         latest: Pulling from {reference}\n\
         Digest: sha256:0000000000000000000000000000000000000000000000000000000000000000\n\
         Status: Downloaded newer image for {reference}\n"
    ))
}

fn respond_compose(data: &StubData, args: &[String]) -> StubResponse {
    if args.iter().any(|a| a == "--services") {
        return StubResponse::ok(
            data.compose_services_text
                .clone()
                .unwrap_or_else(|| "web\ndb\n".to_string()),
        );
    }
    if args.iter().any(|a| a == "config") {
        return StubResponse::ok(format!(
            "{}\n",
            data.compose_config_text
                .clone()
                .unwrap_or_else(|| "{}".to_string())
        ));
    }
    if args.iter().any(|a| a == "up") {
        return StubResponse::blocking("Creating network...\nContainer started\n");
    }
    // down/stop/scale: nothing further needed by this suite.
    StubResponse::ok("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(engine: &str) -> StubData {
        StubData {
            engine: engine.to_string(),
            version_json: Some(r#"{"Client":{"Version":"1.0"}}"#.to_string()),
            containers: vec![serde_json::json!({"Id": "abc123def456extra"})],
            images: vec![serde_json::json!({"Id": "sha256:11112222333344445555"})],
            volumes: vec![serde_json::json!({"Name": "my-volume"})],
            networks: vec![serde_json::json!({"Id": "net123456789extra"})],
            pods_json: Some(r#"[{"Cgroup":"user.slice"}]"#.to_string()),
            events_first_line: Some(r#"{"status":"start"}"#.to_string()),
            top_text: Some("PID USER TIME COMMAND\n1 root 0:00 sh\n".to_string()),
            logs_text: Some("line one\nline two\n".to_string()),
            files_ls_text: Some("total 4\ndrwxr-xr-x 1 root root 0 Jan 1 00:00 .\n".to_string()),
            history_text: Some("{\"ID\":\"abc\"}\n".to_string()),
            context_ls_text: Some(r#"{"Name":"default","Current":true}"#.to_string()),
            machine_list_text: Some(r#"[{"Name":"podman-machine-default"}]"#.to_string()),
            compose_services_text: Some("web\ndb\n".to_string()),
            compose_config_text: Some(r#"{"services":{}}"#.to_string()),
        }
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn engine_name_recognises_podman_by_substring_and_defaults_to_docker() {
        assert_eq!(engine_name("/usr/local/bin/podman"), "podman");
        assert_eq!(engine_name("podman-remote.exe"), "podman");
        assert_eq!(engine_name("/usr/bin/docker"), "docker");
        assert_eq!(engine_name("stub_engine"), "docker");
        assert_eq!(engine_name(""), "docker");
    }

    #[test]
    fn log_line_carries_the_whole_argv_as_json() {
        let line = log_line(&args(&["docker", "ps", "-aq"]));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["argv"], serde_json::json!(["docker", "ps", "-aq"]));
    }

    #[test]
    fn version_prints_the_canned_json_for_the_loaded_engine() {
        let response = respond(
            &args(&["version", "--format", "json"]),
            &data("docker"),
            None,
        );
        assert_eq!(response.stdout, r#"{"Client":{"Version":"1.0"}}"#);
        assert_eq!(response.exit_code, 0);
        assert!(!response.blocks);
    }

    #[test]
    fn version_without_canned_data_is_the_unknown_command_error() {
        let mut d = data("docker");
        d.version_json = None;
        let response = respond(&args(&["version", "--format", "json"]), &d, None);
        assert_eq!(response.exit_code, 1);
        assert!(response.stderr.contains("not a docker command"));
    }

    #[test]
    fn ps_aq_prints_short_ids_deduplicated() {
        let mut d = data("docker");
        d.containers
            .push(serde_json::json!({"Id": "abc123def456extra"})); // duplicate short id
        let response = respond(&args(&["ps", "-aq"]), &d, None);
        assert_eq!(response.stdout, "abc123def456\n");
    }

    #[test]
    fn container_inspect_matches_by_short_id_prefix() {
        let response = respond(
            &args(&["container", "inspect", "abc123def456"]),
            &data("docker"),
            None,
        );
        let parsed: Value = serde_json::from_str(response.stdout.trim()).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 1);
        assert_eq!(parsed[0]["Id"], "abc123def456extra");
    }

    #[test]
    fn container_inspect_with_no_match_returns_an_empty_array() {
        let response = respond(
            &args(&["container", "inspect", "nonexistent"]),
            &data("docker"),
            None,
        );
        assert_eq!(response.stdout.trim(), "[]");
    }

    #[test]
    fn image_ls_and_inspect_use_the_id_field() {
        let list = respond(
            &args(&["image", "ls", "-q", "--no-trunc"]),
            &data("docker"),
            None,
        );
        assert_eq!(list.stdout, "111122223333\n");
        let inspect = respond(
            &args(&["image", "inspect", "111122223333"]),
            &data("docker"),
            None,
        );
        assert!(inspect.stdout.contains("11112222333344445555"));
    }

    #[test]
    fn volume_ls_and_inspect_use_the_name_field_not_a_short_id() {
        let list = respond(&args(&["volume", "ls"]), &data("docker"), None);
        assert_eq!(list.stdout, "my-volume\n");
        let inspect = respond(
            &args(&["volume", "inspect", "my-volume"]),
            &data("docker"),
            None,
        );
        assert!(inspect.stdout.contains("my-volume"));
    }

    #[test]
    fn network_ls_and_inspect_use_the_id_field() {
        let list = respond(&args(&["network", "ls"]), &data("docker"), None);
        assert_eq!(list.stdout, "net123456789\n");
        let inspect = respond(
            &args(&["network", "inspect", "net123456789"]),
            &data("docker"),
            None,
        );
        assert!(inspect.stdout.contains("net123456789extra"));
    }

    #[test]
    fn pod_ls_answers_only_for_podman() {
        let podman = respond(
            &args(&["pod", "ls", "--format", "json"]),
            &data("podman"),
            None,
        );
        assert!(podman.stdout.contains("user.slice"));
        let docker = respond(
            &args(&["pod", "ls", "--format", "json"]),
            &data("docker"),
            None,
        );
        assert_eq!(docker.exit_code, 1, "docker has no `pod` subcommand");
    }

    #[test]
    fn events_blocks_with_no_deferred_line_when_no_delay_was_given() {
        let response = respond(
            &args(&["events", "--format", "{{json .}}"]),
            &data("docker"),
            None,
        );
        assert!(response.blocks);
        assert!(response.defer.is_none());
        assert!(response.stdout.is_empty());
    }

    #[test]
    fn events_defers_the_canned_line_when_a_delay_is_given() {
        let response = respond(
            &args(&["events", "--format", "{{json .}}"]),
            &data("docker"),
            Some(Duration::from_millis(50)),
        );
        assert!(response.blocks);
        let (delay, line) = response.defer.expect("a delay was given");
        assert_eq!(delay, Duration::from_millis(50));
        assert_eq!(line, "{\"status\":\"start\"}\n");
    }

    #[test]
    fn events_with_a_delay_but_no_canned_line_still_blocks_with_no_defer() {
        let mut d = data("docker");
        d.events_first_line = None;
        let response = respond(&args(&["events"]), &d, Some(Duration::from_millis(10)));
        assert!(response.blocks);
        assert!(response.defer.is_none());
    }

    #[test]
    fn lifecycle_ops_echo_every_id_and_never_block() {
        for op in ["start", "stop", "restart", "pause", "unpause"] {
            let response = respond(&args(&[op, "c1", "c2"]), &data("docker"), None);
            assert_eq!(response.stdout, "c1\nc2\n", "op = {op}");
            assert!(!response.blocks);
            assert_eq!(response.exit_code, 0);
        }
    }

    #[test]
    fn rm_supports_the_force_flag_ahead_of_ids() {
        let response = respond(&args(&["rm", "-f", "c1"]), &data("docker"), None);
        assert_eq!(response.stdout, "c1\n");
    }

    #[test]
    fn every_prune_kind_answers_the_same_way() {
        for kind in ["container", "image", "network", "volume"] {
            let response = respond(&args(&[kind, "prune", "-f"]), &data("docker"), None);
            assert_eq!(response.stdout, "Total reclaimed space: 0B\n");
        }
    }

    #[test]
    fn top_prints_the_canned_table() {
        let response = respond(&args(&["top", "c1"]), &data("docker"), None);
        assert!(response.stdout.contains("PID"));
    }

    #[test]
    fn top_falls_back_to_a_minimal_table_with_no_canned_data() {
        let mut d = data("docker");
        d.top_text = None;
        let response = respond(&args(&["top", "c1"]), &d, None);
        assert!(response.stdout.contains("PID"));
    }

    #[test]
    fn logs_prints_the_canned_lines_and_blocks() {
        let response = respond(
            &args(&["logs", "-f", "--timestamps", "--tail", "500", "c1"]),
            &data("docker"),
            None,
        );
        assert_eq!(response.stdout, "line one\nline two\n");
        assert!(response.blocks);
    }

    #[test]
    fn attach_prints_nothing_and_blocks() {
        let response = respond(
            &args(&["attach", "--sig-proxy=false", "c1"]),
            &data("docker"),
            None,
        );
        assert_eq!(response.stdout, "");
        assert!(response.blocks);
    }

    #[test]
    fn exec_ls_gets_the_canned_files_listing() {
        let response = respond(
            &args(&["exec", "-it", "c1", "ls", "-la", "/etc"]),
            &data("docker"),
            None,
        );
        assert!(response.stdout.contains("drwxr-xr-x"));
        assert!(!response.blocks);
    }

    #[test]
    fn exec_root_flag_is_skipped_to_find_the_id_and_command() {
        let response = respond(
            &args(&["exec", "-u", "0", "c1", "ls", "-la"]),
            &data("docker"),
            None,
        );
        assert!(response.stdout.contains("drwxr-xr-x"));
    }

    #[test]
    fn exec_of_a_non_ls_command_prints_nothing() {
        let response = respond(
            &args(&["exec", "-it", "c1", "sh", "-c", "true"]),
            &data("docker"),
            None,
        );
        assert_eq!(response.stdout, "");
        assert_eq!(response.exit_code, 0);
    }

    #[test]
    fn pull_prints_a_progress_transcript_for_the_reference() {
        let response = respond(&args(&["pull", "nginx:1.27"]), &data("docker"), None);
        assert!(response.stdout.contains("Pulling from nginx:1.27"));
        assert!(response
            .stdout
            .contains("Status: Downloaded newer image for nginx:1.27"));
    }

    #[test]
    fn tag_rmi_save_load_all_succeed_with_no_output() {
        for op in ["tag", "rmi", "save", "load"] {
            let response = respond(&args(&[op, "x", "y"]), &data("docker"), None);
            assert_eq!(response.stdout, "");
            assert_eq!(response.exit_code, 0);
        }
    }

    #[test]
    fn history_prints_the_engine_specific_canned_text() {
        let response = respond(
            &args(&["history", "--no-trunc", "--format", "json", "c1"]),
            &data("docker"),
            None,
        );
        assert_eq!(response.stdout, "{\"ID\":\"abc\"}\n");
    }

    #[test]
    fn context_ls_prints_the_canned_ndjson() {
        let response = respond(
            &args(&["context", "ls", "--format", "json"]),
            &data("docker"),
            None,
        );
        assert!(response.stdout.contains("\"Current\":true"));
    }

    #[test]
    fn machine_list_answers_only_for_podman() {
        let podman = respond(
            &args(&["machine", "list", "--format", "json"]),
            &data("podman"),
            None,
        );
        assert!(podman.stdout.contains("podman-machine-default"));
        let docker = respond(
            &args(&["machine", "list", "--format", "json"]),
            &data("docker"),
            None,
        );
        assert_eq!(docker.exit_code, 1);
    }

    #[test]
    fn machine_start_and_stop_succeed_only_for_podman() {
        for sub in ["start", "stop"] {
            let podman = respond(&args(&["machine", sub]), &data("podman"), None);
            assert_eq!(podman.exit_code, 0);
            let docker = respond(&args(&["machine", sub]), &data("docker"), None);
            assert_eq!(docker.exit_code, 1);
        }
    }

    #[test]
    fn compose_services_prints_the_canned_service_list() {
        let response = respond(
            &args(&["compose", "-f", "compose.yml", "config", "--services"]),
            &data("docker"),
            None,
        );
        assert_eq!(response.stdout, "web\ndb\n");
    }

    #[test]
    fn compose_config_without_services_prints_the_canned_config_json() {
        let response = respond(
            &args(&["compose", "-f", "compose.yml", "config", "--format", "json"]),
            &data("docker"),
            None,
        );
        assert!(response.stdout.contains("\"services\""));
    }

    #[test]
    fn compose_up_blocks_and_prints_a_starting_transcript() {
        let response = respond(
            &args(&["compose", "-f", "compose.yml", "up", "-d"]),
            &data("docker"),
            None,
        );
        assert!(response.blocks);
        assert!(response.stdout.contains("Container started"));
    }

    #[test]
    fn compose_down_and_other_subcommands_just_succeed() {
        let response = respond(
            &args(&["compose", "-f", "compose.yml", "down"]),
            &data("docker"),
            None,
        );
        assert_eq!(response.stdout, "");
        assert!(!response.blocks);
    }

    #[test]
    fn an_unknown_subcommand_is_a_docker_shaped_error() {
        let response = respond(&args(&["frobnicate"]), &data("docker"), None);
        assert_eq!(response.exit_code, 1);
        assert_eq!(
            response.stderr,
            "docker: 'frobnicate' is not a docker command.\nSee 'docker --help'.\n"
        );
        let podman_response = respond(&args(&["frobnicate"]), &data("podman"), None);
        assert!(podman_response.stderr.starts_with("podman:"));
    }

    #[test]
    fn stub_data_load_fills_in_from_the_shared_testdata_tree() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"));
        let loaded = StubData::load(dir, "docker");
        assert!(loaded.version_json.is_some());
        assert!(!loaded.containers.is_empty());
        assert!(!loaded.images.is_empty());
        assert!(loaded.pods_json.is_none(), "pods are podman-only");

        let podman = StubData::load(dir, "podman");
        assert!(podman.pods_json.is_some());
        assert!(podman.machine_list_text.is_some());
        // No `top.txt`/`files/ls.txt` fixture is committed — `respond`'s
        // own fallback text covers those, exercised above against a
        // hand-built `StubData` rather than this real tree.
        assert!(loaded.top_text.is_none());
        assert!(loaded.files_ls_text.is_none());
    }

    #[test]
    fn stub_data_load_never_panics_on_a_missing_directory() {
        let loaded = StubData::load(Path::new("/does/not/exist"), "docker");
        assert_eq!(
            loaded,
            StubData {
                engine: "docker".to_string(),
                ..Default::default()
            }
        );
    }
}
