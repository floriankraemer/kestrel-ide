//! Where a process runs: on this Windows machine, or inside a WSL distro
//! reached over `\\wsl$\<distro>\...` / `\\wsl.localhost\<distro>\...`.
//!
//! See `docs/architecture/remote-wsl-plan.md` for the full design. The
//! short version: [`ExecHost::for_path`] is a pure, stateless classifier —
//! [`crate::run`] and [`crate::spawn`] call it on their own `work_dir`, so
//! `vcs-core`, `analysis-core` and `test-core` get WSL execution without a
//! line changing in any of them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

use crate::suppress_console_window;

/// W7-2 (ADR-0052): the global `remote_wsl` off switch, default on.
///
/// `process-exec` is a leaf crate (ADR-0047 §4) and must not depend on
/// `app-config` to read the setting itself, so the switch lives here as a
/// process-wide flag instead: `ui-shell` reads the persisted setting once
/// at startup (and again whenever it changes) and calls
/// [`set_remote_wsl_enabled`]. Every classification funnels through
/// [`ExecHost::for_path`], so flipping this one flag is enough to make
/// every seam in this plan behave as if no project were ever remote —
/// no per-call-site plumbing, and nothing to keep in sync.
static REMOTE_WSL_ENABLED: AtomicBool = AtomicBool::new(true);

/// Turn remote WSL execution on or off for every [`ExecHost::for_path`]
/// call from here on, in this process. See [`REMOTE_WSL_ENABLED`].
pub fn set_remote_wsl_enabled(enabled: bool) {
    REMOTE_WSL_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Where a process should run.
///
/// A value, not a trait: there is one remote kind today, and a `match` on
/// this enum is what makes the next kind (SSH, say) a compile error at
/// every site that needs to think about it — the property a trait with one
/// implementation would trade away for nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecHost {
    Local,
    Wsl(WslHost),
    /// A container (ADR-0067). Never produced by [`ExecHost::for_path`]: a
    /// container is chosen by configuration (a PHP interpreter target), so
    /// git, cargo and npm stay on the host and only callers that go through
    /// `run_on`/`spawn_on` with such a host run inside it.
    Container(ContainerHost),
}

/// Where a project root lives on the host and where the container mounts
/// it, both directions.
///
/// Matching is an exact prefix match after normalising separators, never a
/// filesystem lookup, so this stays a pure, cheaply-testable mapping the way
/// [`ExecHost::to_remote`]/[`ExecHost::to_local`] is for WSL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMap {
    pub local_root: PathBuf,
    pub remote_root: String,
}

/// Where the project root mounts inside a container when nothing says
/// otherwise.
pub const DEFAULT_WORKDIR: &str = "/workspace";

/// Normalise a path string for prefix comparison: backslashes to forward
/// slashes, and (Windows-safe) a leading drive letter lower-cased — `C:\`
/// and `c:/` must match the same root.
fn normalize(path: &str) -> String {
    let out = path.replace('\\', "/");
    let mut chars = out.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => {
            format!("{}:{}", drive.to_ascii_lowercase(), &out[2..])
        }
        _ => out,
    }
}

impl PathMap {
    /// `local_root` joined with `remote_root` defaulted to
    /// [`DEFAULT_WORKDIR`] when `workdir` is empty.
    pub fn new(local_root: impl Into<PathBuf>, workdir: &str) -> Self {
        let remote_root = if workdir.is_empty() {
            DEFAULT_WORKDIR.to_string()
        } else {
            workdir.to_string()
        };
        PathMap {
            local_root: local_root.into(),
            remote_root,
        }
    }

    /// `path` under [`Self::local_root`] -> the same path under
    /// [`Self::remote_root`]. `None` when `path` is not under the root at
    /// all — the caller decides whether that is an error or simply "leave
    /// it alone" (an env value that happens not to be a project path).
    pub fn to_remote(&self, path: &Path) -> Option<String> {
        let root = normalize(&self.local_root.to_string_lossy());
        let candidate = normalize(&path.to_string_lossy());
        let tail = if candidate.eq_ignore_ascii_case(&root) {
            ""
        } else {
            let prefix = if root.ends_with('/') {
                root.clone()
            } else {
                format!("{root}/")
            };
            if candidate.len() >= prefix.len()
                && candidate[..prefix.len()].eq_ignore_ascii_case(&prefix)
            {
                &candidate[prefix.len()..]
            } else {
                return None;
            }
        };
        if tail.is_empty() {
            Some(self.remote_root.clone())
        } else {
            Some(format!("{}/{tail}", self.remote_root.trim_end_matches('/')))
        }
    }

    /// `arg` as the container should see it: a project path (or a
    /// `--flag=<project path>` value) is rebased onto the mount root,
    /// anything else is passed through. The tool is told paths in the
    /// IDE's own spelling (the file to analyse, the project root, the
    /// program itself); only the container knows they live elsewhere.
    pub fn rebase_arg(&self, arg: &str) -> String {
        if let Some(remote) = self.to_remote(Path::new(arg)) {
            return remote;
        }
        match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with('-') => {
                match self.to_remote(Path::new(value)) {
                    Some(remote) => format!("{flag}={remote}"),
                    None => arg.to_string(),
                }
            }
            _ => arg.to_string(),
        }
    }

    /// The inverse of [`Self::to_remote`]: `remote` under
    /// [`Self::remote_root`] -> the same path under [`Self::local_root`],
    /// played back with `local_root`'s own separator style. `None` when
    /// `remote` is not under [`Self::remote_root`].
    pub fn to_local(&self, remote: &str) -> Option<PathBuf> {
        let root = self.remote_root.trim_end_matches('/');
        let candidate = remote.trim_end_matches('/');
        let tail = if candidate == root {
            ""
        } else {
            let prefix = format!("{root}/");
            candidate.strip_prefix(prefix.as_str())?
        };
        let local = self.local_root.to_string_lossy();
        let sep = if local.contains('\\') { '\\' } else { '/' };
        if tail.is_empty() {
            Some(self.local_root.clone())
        } else {
            let tail = tail.replace('/', &sep.to_string());
            let joined = if local.ends_with(sep) {
                format!("{local}{tail}")
            } else {
                format!("{local}{sep}{tail}")
            };
            Some(PathBuf::from(joined))
        }
    }
}

/// How to reach one container: the engine command line up to the point the
/// per-call `-w`/`-e` flags go, then the container reference.
///
/// Built by `container_core::target::exec_host`; this crate only knows the
/// shape `<program> <prefix_args> <verb_args> -w <cwd> -e K=V... <target>
/// <tool> <args...>`, so it stays a leaf (ADR-0047 §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerHost {
    /// The engine program, or `wsl.exe` when the engine is reached through
    /// a WSL distro.
    pub program: String,
    /// Connection flags and, for compose, the `compose -f ...` prefix.
    pub prefix_args: Vec<String>,
    /// The connection's own environment (`DOCKER_HOST`, ...), set on the
    /// engine process.
    pub engine_env: Vec<(String, String)>,
    /// True when `program` is `wsl.exe`, so `engine_env` needs `WSLENV`.
    pub via_wsl: bool,
    /// The verb and its flags, e.g. `exec -i`, `exec -T` or
    /// `run --rm -i -v src:dst`.
    pub verb_args: Vec<String>,
    /// The container, service or image reference.
    pub target: Vec<String>,
    pub path_map: PathMap,
}

/// A WSL distro this project's root lives under, and the exact UNC prefix
/// its path arrived with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslHost {
    /// "Ubuntu", as it appeared in the path — never re-cased or re-spelled.
    pub distro: String,
    /// Exactly as it arrived: `//wsl.localhost/Ubuntu` or
    /// `\\wsl$\Ubuntu`, separators and casing untouched. [`ExecHost::to_local`]
    /// plays this back verbatim, which is the "canonical to the root's own
    /// spelling" half of the translation rule.
    pub unc_prefix: String,
}

/// The two UNC server names WSL exposes a distro's filesystem under,
/// matched case-insensitively — Windows share names are case-insensitive.
const WSL_SERVER_NAMES: [&str; 2] = ["wsl$", "wsl.localhost"];

/// Split a path into `(unc_prefix, distro, remainder)` if it names a WSL
/// UNC path, tolerating any mix of `/`/`\` and any case in the server name.
/// `remainder` uses `/` separators and has no leading slash.
fn parse_unc(path: &Path) -> Option<(String, String, String)> {
    let original = path.to_string_lossy().into_owned();
    let normalized = original.replace('\\', "/");
    let after_leading = normalized.strip_prefix("//")?;

    let mut parts = after_leading.splitn(3, '/');
    let server = parts.next()?;
    let distro = parts.next().unwrap_or("");
    let remainder = parts.next().unwrap_or("");

    if distro.is_empty() || !WSL_SERVER_NAMES.contains(&server.to_ascii_lowercase().as_str()) {
        return None;
    }

    // `original` and `normalized` are the same length char-for-char (`\`
    // and `/` are both one byte/char), so this slice lines up with the
    // caller's own spelling of the server + distro segments.
    let prefix_len = 2 + server.len() + 1 + distro.len();
    let unc_prefix = original.chars().take(prefix_len).collect();

    Some((unc_prefix, distro.to_string(), remainder.to_string()))
}

impl ExecHost {
    /// Classify `path`: `Local` unless it names a WSL UNC path.
    ///
    /// Pure, total, deterministic on the path string — nothing here is
    /// stored, so no service can hold a stale answer once a caller has one.
    pub fn for_path(path: &Path) -> Self {
        if !REMOTE_WSL_ENABLED.load(Ordering::Relaxed) {
            return ExecHost::Local;
        }
        match parse_unc(path) {
            Some((unc_prefix, distro, _remainder)) => ExecHost::Wsl(WslHost { distro, unc_prefix }),
            None => ExecHost::Local,
        }
    }

    /// The tool runs somewhere other than this machine's own process
    /// namespace: in a WSL distro or a container. Decides spawn mechanics —
    /// no local working directory, exit-code mapping.
    pub fn runs_remotely(&self) -> bool {
        !matches!(self, ExecHost::Local)
    }

    /// The project's files are reached over a remote filesystem (WSL's
    /// UNC share), so local file I/O and the native watcher do not apply.
    /// A container keeps the project on the local disk (bind mount) and
    /// answers `false`.
    pub fn filesystem_is_remote(&self) -> bool {
        matches!(self, ExecHost::Wsl(_))
    }

    /// Windows UNC path -> Linux absolute path, e.g.
    /// `//wsl.localhost/Ubuntu/home/f/proj` -> `/home/f/proj`.
    ///
    /// Tolerant: `path` need not share `self`'s exact spelling (a
    /// `QFileDialog` pick and a `gix::discover` cwd can differ) — only the
    /// distro has to agree, and this reparses `path` itself rather than
    /// trusting `self`.
    pub fn to_remote(&self, path: &Path) -> String {
        if let ExecHost::Container(container) = self {
            return container
                .path_map
                .to_remote(path)
                .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"));
        }
        match parse_unc(path) {
            Some((_, _, remainder)) if remainder.is_empty() => "/".to_string(),
            Some((_, _, remainder)) => format!("/{remainder}"),
            None => path.to_string_lossy().replace('\\', "/"),
        }
    }

    /// Linux absolute path -> Windows UNC path, playing `self.unc_prefix`
    /// back verbatim — the "canonical to the root's own spelling" half of
    /// the rule. `to_remote` -> `to_local` is the identity on any path
    /// under the root: the tail is joined with whichever separator
    /// `unc_prefix` itself used, so a root that arrived all-backslash
    /// round-trips to an all-backslash path rather than a mix.
    pub fn to_local(&self, remote: &str) -> PathBuf {
        match self {
            ExecHost::Local => PathBuf::from(remote),
            ExecHost::Container(container) => container
                .path_map
                .to_local(remote)
                .unwrap_or_else(|| PathBuf::from(remote)),
            ExecHost::Wsl(wsl) => {
                let sep = if wsl.unc_prefix.contains('\\') {
                    '\\'
                } else {
                    '/'
                };
                let tail = remote.trim_start_matches('/');
                if tail.is_empty() {
                    PathBuf::from(&wsl.unc_prefix)
                } else {
                    let tail = tail.replace('/', &sep.to_string());
                    PathBuf::from(format!("{}{sep}{tail}", wsl.unc_prefix))
                }
            }
        }
    }

    /// A path a tool printed while running on this host, as a path this
    /// process can open: a Linux absolute path under WSL is translated with
    /// [`Self::to_local`]; anything else (a local path, or a relative one)
    /// is returned as printed.
    pub fn path_from_tool(&self, printed: &str) -> PathBuf {
        match self {
            ExecHost::Wsl(_) | ExecHost::Container(_) if printed.starts_with('/') => {
                self.to_local(printed)
            }
            _ => PathBuf::from(printed),
        }
    }

    /// Build the argv this host actually runs: unchanged for `Local`,
    /// wrapped in `wsl.exe -d <distro> --cd <linux-cwd> -e <program>
    /// <args...>` for `Wsl` — `-e`, not `--`, so a commit message or a
    /// `--message-format=json` argument is never reinterpreted by a shell.
    pub fn argv(&self, program: &str, args: &[&str], cwd: &Path) -> (String, Vec<String>) {
        self.argv_with_env(program, args, cwd, &[])
    }

    /// [`Self::argv`] with per-call `env`. Only a container needs it in the
    /// argv (`-e K=V` before the container reference); `Local` and `Wsl`
    /// carry env on the spawned process instead.
    fn argv_with_env(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
        env: &[(&str, &str)],
    ) -> (String, Vec<String>) {
        match self {
            ExecHost::Container(container) => {
                let remote_cwd = container
                    .path_map
                    .to_remote(cwd)
                    .unwrap_or_else(|| container.path_map.remote_root.clone());
                let mut full = container.prefix_args.clone();
                full.extend(container.verb_args.iter().cloned());
                full.push("-w".to_string());
                full.push(remote_cwd);
                for (key, value) in env {
                    full.push("-e".to_string());
                    full.push(format!("{key}={value}"));
                }
                full.extend(container.target.iter().cloned());
                full.push(container.path_map.rebase_arg(program));
                full.extend(args.iter().map(|a| container.path_map.rebase_arg(a)));
                (container.program.clone(), full)
            }
            ExecHost::Local => (
                program.to_string(),
                args.iter().map(|a| a.to_string()).collect(),
            ),
            ExecHost::Wsl(wsl) => {
                let linux_cwd = self.to_remote(cwd);
                let mut full = vec![
                    "-d".to_string(),
                    wsl.distro.clone(),
                    "--cd".to_string(),
                    linux_cwd,
                    "-e".to_string(),
                    program.to_string(),
                ];
                full.extend(args.iter().map(|a| a.to_string()));
                ("wsl.exe".to_string(), full)
            }
        }
    }

    /// A ready-to-spawn [`Command`]: [`Self::argv`]'s program/args, the
    /// working directory for a local host only (a remote one takes its cwd
    /// from `--cd`, inside the distro, and `wsl.exe` itself is launched from
    /// wherever this process already is), `env` set on the
    /// Windows-side process and, for a remote host, merged onto `WSLENV`
    /// with the `/u` (translate-as-UTF-8-string) flag so `wsl.exe` actually
    /// passes each variable through — without a name in `WSLENV`, `wsl.exe`
    /// drops it silently. `CREATE_NO_WINDOW` applies to whichever process
    /// this spawns directly: `wsl.exe` itself for a remote host, so it -
    /// not a child inside the distro, which Windows never sees - is the one
    /// thing that could flash a console.
    pub fn command(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
        env: &[(&str, &str)],
    ) -> Command {
        let (resolved_program, resolved_args) = self.argv_with_env(program, args, cwd, env);
        let mut command = Command::new(&resolved_program);
        command.args(&resolved_args);
        // Only a local host is launched *from* the working directory. For a
        // remote one `--cd` already decides where the command runs inside
        // the distro, so `wsl.exe`'s own Windows-side directory buys
        // nothing — and pinning it to the `\\wsl.localhost\...` path is a
        // way to fail: the spawn needs that UNC path to resolve, which it
        // does not when the distro is stopped or the share is unavailable.
        // A container's engine is a local process, and a relative compose
        // file resolves against its cwd — unless the engine itself sits
        // behind `wsl.exe`.
        let local_cwd = match self {
            ExecHost::Local => true,
            ExecHost::Wsl(_) => false,
            ExecHost::Container(container) => !container.via_wsl,
        };
        if local_cwd {
            command.current_dir(cwd);
        }

        match self {
            ExecHost::Container(container) => {
                let engine_env: Vec<(&str, &str)> = container
                    .engine_env
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                for (key, value) in &engine_env {
                    command.env(key, value);
                }
                if container.via_wsl && !engine_env.is_empty() {
                    command.env("WSLENV", wslenv_with(&engine_env));
                }
            }
            _ => {
                for (key, value) in env {
                    command.env(key, value);
                }
                if self.runs_remotely() && !env.is_empty() {
                    command.env("WSLENV", wslenv_with(env));
                }
            }
        }

        suppress_console_window(&mut command);
        command
    }
}

/// The `WSLENV` value that makes `wsl.exe` pass each of `env`'s variables
/// through to the distro: the inherited list plus every key with the `/u`
/// (translate-as-UTF-8-string) flag. Without a name in `WSLENV`, `wsl.exe`
/// drops the variable silently. Shared with callers that build a
/// `wsl.exe` argv themselves (a container connection through a WSL
/// distro) and therefore cannot go through [`ExecHost::command`].
pub fn wslenv_with(env: &[(&str, &str)]) -> String {
    let inherited = std::env::var("WSLENV").unwrap_or_default();
    let mut names: Vec<String> = if inherited.is_empty() {
        Vec::new()
    } else {
        inherited.split(':').map(str::to_string).collect()
    };
    for (key, _) in env {
        names.push(format!("{key}/u"));
    }
    names.join(":")
}

// --------------------------------------------------------- discovery -----

/// `wsl.exe` writes UTF-16LE, so its output is mostly NUL bytes to anything
/// expecting UTF-8 — decoding it as such yields one distro name per *two*
/// bytes of garbage, which is why this is spelled out rather than left to
/// `String::from_utf8_lossy`. Moved here from `pty_core::shells` (W1-4):
/// [`resolve_program`] needs the same decode for `wsl.exe`'s own output,
/// and a second copy is what `process-exec` as the mechanism crate exists
/// to prevent.
fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        // A byte-order mark leads the stream on some Windows builds.
        .filter(|unit| *unit != 0xFEFF)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Distro names out of `wsl.exe --list --quiet`'s decoded text.
///
/// `--quiet` drops the header, but a default distro is still marked with a
/// trailing ` (Default)` in some Windows builds, and every line carries the
/// `\r` of a CRLF stream.
fn parse_distro_list(decoded: &str) -> Vec<String> {
    decoded
        .lines()
        .map(|line| line.trim().trim_end_matches("(Default)").trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Every WSL distro this machine has installed, most-preferred (the
/// default) first. Empty wherever `wsl.exe` is not on `PATH` — every other
/// platform, and a Windows machine without WSL.
pub fn distros() -> Vec<String> {
    let mut command = Command::new("wsl.exe");
    command.args(["--list", "--quiet"]);
    suppress_console_window(&mut command);
    let output = crate::retry_text_busy(|| command.output());
    match output {
        Ok(output) => parse_distro_list(&decode_utf16le(&output.stdout)),
        Err(_) => Vec::new(),
    }
}

// ------------------------------------------------------ tool discovery ---

type ResolveCache = Mutex<HashMap<(String, String), Option<String>>>;

fn resolve_cache() -> &'static ResolveCache {
    static CACHE: OnceLock<ResolveCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve `program` to an absolute path *inside the distro* `host` names,
/// memoised per `(distro, program)` for the process's lifetime — one probe
/// per tool per distro per session, which is what makes [`ExecHost::argv`]'s
/// exact-argv `-e` guarantee affordable (see the plan's "Tool discovery"
/// section).
///
/// `Local` never probes: `Some(program.to_string())` unchanged, today's
/// behaviour.
///
/// A candidate containing a path separator (`./gradlew`,
/// `vendor/bin/phpstan`) is joined onto `cwd`'s remote path and confirmed
/// executable with `test -x`; the probe is about executability *in the
/// distro*, not existence on the 9P share. A bare name (`rust-analyzer`,
/// `git`) is resolved with one login-shell `command -v`, since only a login
/// shell sources the profile that puts `~/.cargo/bin` on `PATH`.
///
/// `None` means "not found in the distro" — callers map that onto their own
/// vocabulary (`VcsError::GitNotInstalled`, `AnalyzerStatus::NotDetected`),
/// the same split every other `process-exec` failure draws.
pub fn resolve_program(host: &ExecHost, program: &str, cwd: &Path) -> Option<String> {
    resolve_program_or_reason(host, program, cwd).ok()
}

/// [`resolve_program`] that also says *why* a container lookup missed:
/// `Err(Some(reason))` when the container itself is unavailable (stopped,
/// no such service, engine down — [`container_unavailable_reason`]), else
/// `Err(None)`.
pub fn resolve_program_or_reason(
    host: &ExecHost,
    program: &str,
    cwd: &Path,
) -> Result<String, Option<&'static str>> {
    let wsl = match host {
        ExecHost::Wsl(wsl) => wsl,
        ExecHost::Container(container) => {
            return resolve_in_container(host, container, program, cwd)
        }
        ExecHost::Local => return Ok(program.to_string()),
    };

    let key = (wsl.distro.clone(), program.to_string());
    if let Some(cached) = resolve_cache().lock().unwrap().get(&key) {
        return cached.clone().ok_or(None);
    }

    let resolved = if program.contains('/') || program.contains('\\') {
        let candidate = host.to_remote(&cwd.join(program));
        probe_executable(&wsl.distro, &candidate).then_some(candidate)
    } else {
        probe_command_v(&wsl.distro, program)
    };

    resolve_cache()
        .lock()
        .unwrap()
        .insert(key, resolved.clone());
    resolved.ok_or(None)
}

/// [`resolve_program`] inside a container: `sh -c 'command -v "$1"'` for a
/// bare name, `test -x` for a path. Only a hit is memoised — a miss may just
/// mean the container is not up yet, and caching that would keep reporting
/// it missing after the user starts it.
fn resolve_in_container(
    host: &ExecHost,
    container: &ContainerHost,
    program: &str,
    cwd: &Path,
) -> Result<String, Option<&'static str>> {
    let key = (
        format!(
            "container:{} {:?} {:?}",
            container.program, container.prefix_args, container.target
        ),
        program.to_string(),
    );
    if let Some(Some(hit)) = resolve_cache().lock().unwrap().get(&key) {
        return Ok(hit.clone());
    }

    let is_path = program.contains('/') || program.contains('\\');
    let candidate = if is_path {
        host.to_remote(&cwd.join(program))
    } else {
        program.to_string()
    };
    let script = if is_path {
        r#"test -x "$1" && echo "$1""#
    } else {
        r#"command -v "$1""#
    };
    let mut command = host.command("sh", &["-c", script, "sh", &candidate], cwd, &[]);
    suppress_console_window(&mut command);
    let output = crate::retry_text_busy(|| command.output()).map_err(|_| None)?;
    if !output.status.success() {
        return Err(container_unavailable_reason(&output.stderr));
    }
    let resolved = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if resolved.is_empty() {
        return Err(None);
    }
    resolve_cache()
        .lock()
        .unwrap()
        .insert(key, Some(resolved.clone()));
    Ok(resolved)
}

/// The first of `candidates` that [`resolve_program`] would find on `host`,
/// in order. A container answers the whole list in one probe: with a
/// `compose run` target each probe starts a container (about a second
/// each), so three candidates per analyzer would mean three containers.
/// A hit is memoised per list, a miss is not (the container may be down).
pub fn resolve_first(host: &ExecHost, candidates: &[String], cwd: &Path) -> Option<String> {
    let ExecHost::Container(container) = host else {
        return candidates
            .iter()
            .find_map(|candidate| resolve_program(host, candidate, cwd));
    };
    let key = (
        format!(
            "container-first:{} {:?} {:?}",
            container.program, container.prefix_args, container.target
        ),
        candidates.join("\0"),
    );
    if let Some(Some(hit)) = resolve_cache().lock().unwrap().get(&key) {
        return Some(hit.clone());
    }
    // A path candidate is tested by its remote path, a bare name looked up
    // on the container's PATH; a remote path always has a `/`, a bare name
    // never does, so the script tells them apart the same way.
    let remote: Vec<String> = candidates
        .iter()
        .map(|candidate| {
            if candidate.contains('/') || candidate.contains('\\') {
                host.to_remote(&cwd.join(candidate))
            } else {
                candidate.clone()
            }
        })
        .collect();
    let script = r#"for c in "$@"; do case "$c" in */*) [ -f "$c" ] && [ -x "$c" ] && { echo "$c"; exit 0; } ;; *) command -v "$c" && exit 0 ;; esac; done; exit 0"#;
    let mut argv = vec!["-c", script, "sh"];
    argv.extend(remote.iter().map(String::as_str));
    let mut command = host.command("sh", &argv, cwd, &[]);
    suppress_console_window(&mut command);
    let output = crate::retry_text_busy(|| command.output()).ok()?;
    let resolved = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if !output.status.success() || resolved.is_empty() {
        return None;
    }
    resolve_cache()
        .lock()
        .unwrap()
        .insert(key, Some(resolved.clone()));
    Some(resolved)
}

fn probe_executable(distro: &str, remote_path: &str) -> bool {
    let mut command = Command::new("wsl.exe");
    command.args(["-d", distro, "-e", "test", "-x", remote_path]);
    suppress_console_window(&mut command);
    crate::retry_text_busy(|| command.output())
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn probe_command_v(distro: &str, program: &str) -> Option<String> {
    let script = format!("command -v {program}");
    let mut command = Command::new("wsl.exe");
    command.args(["-d", distro, "-e", "/bin/sh", "-lc", &script]);
    suppress_console_window(&mut command);
    let output = crate::retry_text_busy(|| command.output()).ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

// ------------------------------------------------------- exit mapping ----

/// The two ways `wsl.exe` itself, not the command it was told to run,
/// reports failure — matched case-insensitively against stderr.
const WSL_ITSELF_FAILED_MARKERS: [&str; 2] = [
    "no distribution with the supplied name",
    "no installed distributions",
];

/// What a container engine says on stderr when the container, not the tool
/// inside it, is the problem, paired with the sentence to show the user.
/// Matched case-insensitively. Docker, Podman and Compose phrase it
/// differently: `No such container`, `no container with name or ID`,
/// `service "x" is not running`, `no such service`, and a missing binary as
/// `executable file not found`.
const CONTAINER_UNAVAILABLE_MARKERS: [(&str, &str); 6] = [
    (
        "no such container",
        "The container does not exist. Start it, or check the interpreter's container target.",
    ),
    (
        "no container with name or id",
        "The container does not exist. Start it, or check the interpreter's container target.",
    ),
    (
        "is not running",
        "The container or compose service is not running. Start it (for example `docker compose up -d`).",
    ),
    (
        "no such service",
        "The compose file has no such service. Check the interpreter's container target.",
    ),
    (
        "executable file not found",
        "The program is not installed in the container.",
    ),
    (
        "cannot connect to the docker daemon",
        "The container engine is not reachable. Start Docker or Podman.",
    ),
];

/// A clear sentence when `stderr` shows the container (not the tool in it)
/// is unavailable; `None` for an ordinary failure of the tool itself.
pub fn container_unavailable_reason(stderr: &[u8]) -> Option<&'static str> {
    let lower = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    CONTAINER_UNAVAILABLE_MARKERS
        .iter()
        .find(|(marker, _)| lower.contains(marker))
        .map(|(_, reason)| *reason)
}

/// Whether a finished remote-host run should be reported as
/// [`crate::Failure::NotFound`] rather than a normal (possibly failing)
/// [`crate::Output`].
///
/// `wsl.exe` returns the *Linux command's* exit code, so `Output::status`
/// keeps its meaning for every ordinary failure. What breaks is a missing
/// Linux binary: `wsl.exe` itself spawned fine, so it never surfaces as
/// `io::ErrorKind::NotFound` the way a missing local binary does. A shell
/// reports "command not found" as exit 127, and `wsl.exe` reports its own
/// setup failures (wrong distro name, no WSL installed) as text on stderr
/// with a nonzero exit — both get remapped here.
pub fn is_missing_program(exit_code: Option<i32>, stderr: &[u8]) -> bool {
    if exit_code == Some(127) {
        return true;
    }
    let stderr_lower = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    WSL_ITSELF_FAILED_MARKERS
        .iter()
        .any(|marker| stderr_lower.contains(marker))
        || container_unavailable_reason(stderr).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wsl(path: &str) -> ExecHost {
        ExecHost::for_path(Path::new(path))
    }

    // W7-2: the off switch is a process-wide static, so this test resets it
    // on every exit path rather than trusting nextest's one-process-per-test
    // isolation alone — a plain `cargo test` run shares a process across
    // this whole file's tests.
    #[test]
    fn the_off_switch_forces_local_even_for_a_wsl_shaped_path() {
        set_remote_wsl_enabled(false);
        let result = std::panic::catch_unwind(|| {
            assert_eq!(wsl(r"\\wsl.localhost\Ubuntu\home\f\proj"), ExecHost::Local);
        });
        set_remote_wsl_enabled(true);
        result.unwrap();
    }

    #[test]
    fn wsl_dollar_backslash_classifies_as_remote() {
        let host = wsl(r"\\wsl$\Ubuntu\home\f\proj");
        assert_eq!(
            host,
            ExecHost::Wsl(WslHost {
                distro: "Ubuntu".into(),
                unc_prefix: r"\\wsl$\Ubuntu".into(),
            })
        );
    }

    #[test]
    fn wsl_localhost_backslash_classifies_as_remote() {
        let host = wsl(r"\\wsl.localhost\Ubuntu\home\f\proj");
        assert_eq!(
            host,
            ExecHost::Wsl(WslHost {
                distro: "Ubuntu".into(),
                unc_prefix: r"\\wsl.localhost\Ubuntu".into(),
            })
        );
    }

    #[test]
    fn wsl_dollar_forward_slash_classifies_as_remote() {
        let host = wsl("//wsl$/Ubuntu/home/f/proj");
        assert_eq!(
            host,
            ExecHost::Wsl(WslHost {
                distro: "Ubuntu".into(),
                unc_prefix: "//wsl$/Ubuntu".into(),
            })
        );
    }

    #[test]
    fn wsl_localhost_forward_slash_classifies_as_remote() {
        let host = wsl("//wsl.localhost/Ubuntu/home/f/proj");
        assert_eq!(
            host,
            ExecHost::Wsl(WslHost {
                distro: "Ubuntu".into(),
                unc_prefix: "//wsl.localhost/Ubuntu".into(),
            })
        );
    }

    #[test]
    fn mixed_separators_and_case_still_classify() {
        let host = wsl(r"\\WSL.LOCALHOST/Ubuntu\home/f");
        assert!(host.filesystem_is_remote());
        let ExecHost::Wsl(wsl) = host else {
            unreachable!()
        };
        assert_eq!(wsl.distro, "Ubuntu");
    }

    #[test]
    fn a_distro_name_with_a_hyphen_and_dot_is_kept_verbatim() {
        let host = wsl(r"\\wsl.localhost\Ubuntu-22.04\home\f");
        let ExecHost::Wsl(wsl) = host else {
            panic!("expected remote")
        };
        assert_eq!(wsl.distro, "Ubuntu-22.04");
    }

    #[test]
    fn a_path_merely_containing_wsl_is_local() {
        assert_eq!(wsl("C:/wsl/notaunc"), ExecHost::Local);
        assert_eq!(wsl(r"C:\wsl\notaunc"), ExecHost::Local);
    }

    #[test]
    fn a_plain_local_path_is_local() {
        assert_eq!(wsl(r"C:\Users\f\proj"), ExecHost::Local);
        assert_eq!(wsl("/home/f/proj"), ExecHost::Local);
    }

    #[test]
    fn to_remote_strips_the_unc_prefix() {
        let host = wsl(r"\\wsl.localhost\Ubuntu\home\f\proj");
        assert_eq!(
            host.to_remote(Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj\src\main.rs")),
            "/home/f/proj/src/main.rs"
        );
    }

    #[test]
    fn to_remote_tolerates_a_different_spelling_than_the_stored_host() {
        // The Qt-vs-gix divergence the plan calls out: both spellings name
        // the same distro and must translate to the same Linux path.
        let host = wsl(r"\\wsl.localhost\Ubuntu\home\f\proj");
        assert_eq!(
            host.to_remote(Path::new(r"\\wsl$\Ubuntu\home\f\proj")),
            "/home/f/proj"
        );
    }

    #[test]
    fn a_linux_path_a_tool_printed_under_wsl_becomes_a_unc_path() {
        let host = wsl(r"\\wsl.localhost\Ubuntu\home\f\proj");
        assert_eq!(
            host.path_from_tool("/home/f/proj/src/a.php"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\f\proj\src\a.php")
        );
    }

    #[test]
    fn a_relative_or_local_tool_path_is_left_alone() {
        let host = wsl(r"\\wsl.localhost\Ubuntu\home\f\proj");
        assert_eq!(host.path_from_tool("src/a.php"), PathBuf::from("src/a.php"));
        assert_eq!(
            ExecHost::Local.path_from_tool("/p/a.php"),
            PathBuf::from("/p/a.php")
        );
    }

    #[test]
    fn to_remote_to_local_round_trips_to_identity() {
        let host = wsl(r"\\wsl.localhost\Ubuntu\home\f\proj");
        let original = Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj\src\main.rs");
        let remote = host.to_remote(original);
        assert_eq!(host.to_local(&remote), PathBuf::from(original));
    }

    #[test]
    fn to_remote_to_local_round_trips_at_the_root_itself() {
        let host = wsl(r"\\wsl.localhost\Ubuntu\home\f\proj");
        let root = Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj");
        let remote = host.to_remote(root);
        assert_eq!(host.to_local(&remote), PathBuf::from(root));
    }

    #[test]
    fn local_host_to_remote_to_local_is_identity_too() {
        let host = ExecHost::Local;
        let original = Path::new("/home/f/proj/src/main.rs");
        let remote = host.to_remote(original);
        assert_eq!(host.to_local(&remote), PathBuf::from(original));
    }

    #[test]
    fn local_argv_is_unchanged() {
        let host = ExecHost::Local;
        let (program, args) = host.argv("git", &["status"], Path::new("/home/f/proj"));
        assert_eq!(program, "git");
        assert_eq!(args, vec!["status".to_string()]);
    }

    #[test]
    fn wsl_argv_wraps_with_dash_e_and_dash_dash_cd() {
        let host = wsl(r"\\wsl.localhost\Ubuntu\home\f\proj");
        let (program, args) = host.argv(
            "git",
            &["log", "--grep=a b $x"],
            Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj"),
        );
        assert_eq!(program, "wsl.exe");
        assert_eq!(
            args,
            vec![
                "-d".to_string(),
                "Ubuntu".to_string(),
                "--cd".to_string(),
                "/home/f/proj".to_string(),
                "-e".to_string(),
                "git".to_string(),
                "log".to_string(),
                "--grep=a b $x".to_string(),
            ]
        );
    }

    #[test]
    fn wsl_command_merges_env_names_onto_wslenv_without_dropping_the_inherited_value() {
        // SAFETY: test-only env mutation, no threads touch WSLENV concurrently.
        unsafe {
            std::env::set_var("WSLENV", "EXISTING/u");
        }
        let host = wsl(r"\\wsl.localhost\Ubuntu\home\f\proj");
        let command = host.command(
            "git",
            &["status"],
            Path::new(r"\\wsl.localhost\Ubuntu\home\f\proj"),
            &[("GIT_TERMINAL_PROMPT", "0")],
        );
        let wslenv = command
            .get_envs()
            .find(|(k, _)| *k == "WSLENV")
            .and_then(|(_, v)| v)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(wslenv, "EXISTING/u:GIT_TERMINAL_PROMPT/u");
        // SAFETY: same test-local cleanup.
        unsafe {
            std::env::remove_var("WSLENV");
        }
    }

    #[test]
    fn distro_list_drops_the_default_marker_and_trailing_cr() {
        assert_eq!(
            parse_distro_list("Ubuntu (Default)\r\ndebian\r\n"),
            vec!["Ubuntu".to_string(), "debian".to_string()]
        );
    }

    #[test]
    fn wsl_output_is_decoded_as_utf16le_not_utf8() {
        let mut bytes = vec![0xFF, 0xFE]; // BOM
        for unit in "Ubuntu\r\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_utf16le(&bytes), "Ubuntu\r\n");
    }

    #[test]
    fn local_host_never_probes_and_returns_the_program_unchanged() {
        let resolved = resolve_program(&ExecHost::Local, "anything-at-all", Path::new("/tmp"));
        assert_eq!(resolved, Some("anything-at-all".to_string()));
    }

    #[test]
    fn exit_127_is_a_missing_program() {
        assert!(is_missing_program(Some(127), b""));
    }

    #[test]
    fn wsl_no_distribution_stderr_is_a_missing_program() {
        assert!(is_missing_program(
            Some(1),
            b"Wsl/Service/CreateInstance/CreateVm/... There is no distribution with the supplied name."
        ));
    }

    #[test]
    fn wsl_no_installed_distributions_stderr_is_a_missing_program() {
        assert!(is_missing_program(
            Some(1),
            b"Windows Subsystem for Linux has no installed distributions."
        ));
    }

    #[test]
    fn an_ordinary_nonzero_exit_is_not_a_missing_program() {
        assert!(!is_missing_program(Some(1), b"fatal: not a git repository"));
    }

    // The three tests below cover `distros`, `probe_executable` and
    // `probe_command_v` — the spawn sites this branch adds
    // `suppress_console_window` to (they used to build their own bare
    // `Command::new("wsl.exe")`, bypassing `ExecHost::command`'s call to it).
    // All three serialize on `PATH_LOCK` since they mutate the process-wide
    // `PATH` to put a fake `wsl.exe` in front of the real one, the same
    // technique `run_core::supervisor`'s `launch_wraps_a_remote_cwd_...` test
    // uses.
    static PATH_LOCK: Mutex<()> = Mutex::new(());

    /// Prepends `dir` to `PATH` and returns the previous value to restore.
    fn prepend_to_path(dir: &Path) -> String {
        let original = std::env::var("PATH").unwrap_or_default();
        // SAFETY: serialized by `PATH_LOCK`, held by every caller.
        unsafe {
            std::env::set_var("PATH", format!("{}:{original}", dir.display()));
        }
        original
    }

    fn restore_path(original: String) {
        // SAFETY: serialized by `PATH_LOCK`, held by every caller.
        unsafe {
            std::env::set_var("PATH", original);
        }
    }

    fn write_executable_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        write!(file, "#!/bin/sh\n{body}").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[test]
    fn distros_lists_every_distro_a_fake_wsl_exe_reports() {
        let _guard = PATH_LOCK.lock().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();

        // `wsl.exe --list --quiet` really writes UTF-16LE with a leading
        // BOM (see `wsl_output_is_decoded_as_utf16le_not_utf8` above) — the
        // fake binary here just `cat`s a file holding those exact bytes,
        // built with the same encoding this module's own decoder expects.
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "Ubuntu (Default)\r\ndebian\r\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let output_path = bin_dir.path().join("distros_output.bin");
        std::fs::write(&output_path, &bytes).unwrap();

        write_executable_script(
            bin_dir.path(),
            "wsl.exe",
            &format!("cat \"{}\"\n", output_path.display()),
        );

        let original_path = prepend_to_path(bin_dir.path());
        let result = distros();
        restore_path(original_path);

        assert_eq!(result, vec!["Ubuntu".to_string(), "debian".to_string()]);
    }

    #[test]
    fn distros_is_empty_when_wsl_exe_is_not_on_path() {
        let _guard = PATH_LOCK.lock().unwrap();
        let original_path = prepend_to_path(Path::new("/does/not/exist"));
        let result = distros();
        restore_path(original_path);
        assert!(result.is_empty());
    }

    #[test]
    fn probe_executable_reports_true_when_the_fake_wsl_exe_confirms_it() {
        let _guard = PATH_LOCK.lock().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();
        write_executable_script(
            bin_dir.path(),
            "wsl.exe",
            "case \"$*\" in\n  *present*) exit 0;;\n  *) exit 1;;\nesac\n",
        );

        let original_path = prepend_to_path(bin_dir.path());
        let found = probe_executable("Ubuntu", "/opt/present-tool");
        let missing = probe_executable("Ubuntu", "/opt/absent-tool");
        restore_path(original_path);

        assert!(found);
        assert!(!missing);
    }

    #[test]
    fn probe_command_v_returns_the_resolved_path_on_success() {
        let _guard = PATH_LOCK.lock().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();
        write_executable_script(
            bin_dir.path(),
            "wsl.exe",
            "echo /usr/bin/rust-analyzer\nexit 0\n",
        );

        let original_path = prepend_to_path(bin_dir.path());
        let resolved = probe_command_v("Ubuntu", "rust-analyzer");
        restore_path(original_path);

        assert_eq!(resolved, Some("/usr/bin/rust-analyzer".to_string()));
    }

    #[test]
    fn probe_command_v_returns_none_when_the_fake_wsl_exe_fails() {
        let _guard = PATH_LOCK.lock().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();
        write_executable_script(bin_dir.path(), "wsl.exe", "exit 1\n");

        let original_path = prepend_to_path(bin_dir.path());
        let resolved = probe_command_v("Ubuntu", "does-not-exist");
        restore_path(original_path);

        assert_eq!(resolved, None);
    }

    // ----------------------------------------------- container (X1) -----

    fn container() -> ExecHost {
        ExecHost::Container(ContainerHost {
            program: "docker".into(),
            prefix_args: vec!["compose".into(), "-f".into(), "dc.yml".into()],
            engine_env: vec![],
            via_wsl: false,
            verb_args: vec!["exec".into(), "-T".into()],
            target: vec!["php".into()],
            path_map: PathMap::new("/home/f/proj", "/var/www"),
        })
    }

    #[test]
    fn a_container_argv_is_engine_verb_cwd_env_target_then_the_tool() {
        let host = container();
        let (program, args) = host.argv(
            "vendor/bin/phpstan",
            &["analyse"],
            Path::new("/home/f/proj/app"),
        );
        assert_eq!(program, "docker");
        assert_eq!(
            args,
            [
                "compose",
                "-f",
                "dc.yml",
                "exec",
                "-T",
                "-w",
                "/var/www/app",
                "php",
                "vendor/bin/phpstan",
                "analyse"
            ]
        );
    }

    #[test]
    fn a_container_command_passes_per_call_env_as_dash_e() {
        let host = container();
        let command = host.command("php", &["-v"], Path::new("/home/f/proj"), &[("A", "b")]);
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
        assert!(args.windows(2).any(|w| w == ["-e", "A=b"]), "{args:?}");
        assert_eq!(command.get_current_dir(), Some(Path::new("/home/f/proj")));
    }

    #[test]
    fn project_paths_in_the_program_and_args_are_rebased_onto_the_mount() {
        let (_, args) = container().argv(
            "/home/f/proj/vendor/bin/phpstan",
            &[
                "analyse",
                "/home/f/proj/src/A.php",
                "--configuration=/home/f/proj/phpstan.neon",
                "--level=5",
                "src/relative.php",
                "/etc/hosts",
            ],
            Path::new("/home/f/proj"),
        );
        assert_eq!(
            args[args.len() - 7..],
            [
                "/var/www/vendor/bin/phpstan",
                "analyse",
                "/var/www/src/A.php",
                "--configuration=/var/www/phpstan.neon",
                "--level=5",
                "src/relative.php",
                "/etc/hosts"
            ]
        );
    }

    #[test]
    fn a_cwd_outside_the_mount_runs_from_the_mount_root() {
        let (_, args) = container().argv("php", &[], Path::new("/tmp"));
        assert!(args.windows(2).any(|w| w == ["-w", "/var/www"]), "{args:?}");
    }

    #[test]
    fn a_container_translates_paths_through_its_path_map() {
        let host = container();
        assert_eq!(
            host.to_remote(Path::new("/home/f/proj/src/A.php")),
            "/var/www/src/A.php"
        );
        assert_eq!(
            host.to_local("/var/www/src/A.php"),
            PathBuf::from("/home/f/proj/src/A.php")
        );
        assert_eq!(
            host.path_from_tool("/var/www/src/A.php"),
            PathBuf::from("/home/f/proj/src/A.php")
        );
        // Outside the mount (e.g. a vendor path baked into the image).
        assert_eq!(
            host.path_from_tool("/usr/share/php/X.php"),
            PathBuf::from("/usr/share/php/X.php")
        );
        assert_eq!(host.path_from_tool("rel/A.php"), PathBuf::from("rel/A.php"));
    }

    #[test]
    fn a_container_runs_remotely_but_keeps_a_local_filesystem() {
        let host = container();
        assert!(host.runs_remotely());
        assert!(!host.filesystem_is_remote());
        assert!(wsl(r"\\wsl$\Ubuntu\home").filesystem_is_remote());
        assert!(!ExecHost::Local.runs_remotely());
    }

    #[test]
    fn for_path_never_classifies_a_container() {
        for path in ["/home/f/proj", "/workspace", r"C:\proj", "//wsl$/Ubuntu/x"] {
            assert!(!matches!(wsl(path), ExecHost::Container(_)), "{path}");
        }
    }

    // ------------------------------------------- container (X2) -----

    #[test]
    fn a_stopped_container_or_service_is_a_missing_program() {
        for stderr in [
            "Error response from daemon: No such container: web",
            "service \"php\" is not running",
            "Error: no container with name or ID \"web\" found",
            "no such service: php",
        ] {
            assert!(is_missing_program(Some(1), stderr.as_bytes()), "{stderr}");
            assert!(container_unavailable_reason(stderr.as_bytes()).is_some());
        }
        assert!(!is_missing_program(Some(1), b"PHP Parse error"));
        assert_eq!(container_unavailable_reason(b"PHP Parse error"), None);
    }

    /// A "container engine" that is `echo` (or `sh -c`), so no test writes an
    /// executable and races another test's fork (ETXTBSY).
    fn fake_container(program: &str, prefix: &[&str], root: &Path) -> ExecHost {
        ExecHost::Container(ContainerHost {
            program: program.into(),
            prefix_args: prefix.iter().map(|a| a.to_string()).collect(),
            engine_env: vec![],
            via_wsl: false,
            verb_args: vec!["exec".into(), "-i".into()],
            target: vec!["web".into()],
            // The engine is spawned with the project as its cwd, so it must exist.
            path_map: PathMap::new(root, "/workspace"),
        })
    }

    #[test]
    fn a_bare_name_resolves_with_command_v_inside_the_container() {
        let dir = tempfile::tempdir().unwrap();
        // `echo` prints the argv it was given, which is what the probe reads back.
        let host = fake_container("echo", &[], dir.path());
        let resolved = resolve_program(&host, "phpcs-x", dir.path());
        assert_eq!(
            resolved.as_deref(),
            Some(r#"exec -i -w /workspace web sh -c command -v "$1" sh phpcs-x"#)
        );
    }

    #[test]
    fn a_path_candidate_is_tested_inside_the_container_by_its_remote_path() {
        let dir = tempfile::tempdir().unwrap();
        let host = fake_container("echo", &[], dir.path());
        let resolved = resolve_program(&host, "vendor/bin/phpstan", dir.path());
        assert!(
            resolved
                .as_deref()
                .is_some_and(|r| r.ends_with("sh /workspace/vendor/bin/phpstan")),
            "{resolved:?}"
        );
    }

    /// An "engine" that drops `exec -i -w <cwd> web` and runs the rest
    /// locally, counting its starts; the project maps onto itself.
    fn counting_container(root: &Path, count: &Path) -> ExecHost {
        let script = format!("echo x >> {}; shift 5; exec \"$@\"", count.display());
        ExecHost::Container(ContainerHost {
            program: "sh".into(),
            prefix_args: vec!["-c".into(), script, "sh".into()],
            engine_env: vec![],
            via_wsl: false,
            verb_args: vec!["exec".into(), "-i".into()],
            target: vec!["web".into()],
            path_map: PathMap::new(root, &root.to_string_lossy()),
        })
    }

    #[test]
    fn a_container_resolves_the_whole_candidate_list_in_one_probe() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("starts");
        let host = counting_container(dir.path(), &count);
        let candidates: Vec<String> = ["vendor/bin/nope", "nope-x.phar", "sh"]
            .map(String::from)
            .to_vec();
        let found = resolve_first(&host, &candidates, dir.path());
        assert!(
            found.as_deref().is_some_and(|p| p.ends_with("/sh")),
            "{found:?}"
        );
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
        // Memoised: a second lookup starts nothing.
        assert_eq!(resolve_first(&host, &candidates, dir.path()), found);
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
    }

    #[test]
    fn a_container_list_with_no_hit_is_none_and_is_asked_again() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("starts");
        let host = counting_container(dir.path(), &count);
        let candidates = vec!["vendor/bin/nope".to_string(), "nope-y".to_string()];
        assert_eq!(resolve_first(&host, &candidates, dir.path()), None);
        assert_eq!(resolve_first(&host, &candidates, dir.path()), None);
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    }

    #[test]
    fn a_path_candidate_wins_over_a_later_bare_name() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("starts");
        let host = counting_container(dir.path(), &count);
        // `/bin/sh` is a path candidate outside the project: kept as is.
        let candidates = vec!["/bin/sh".to_string(), "sh".to_string()];
        assert_eq!(
            resolve_first(&host, &candidates, dir.path()).as_deref(),
            Some("/bin/sh")
        );
    }

    #[test]
    fn a_failing_probe_is_none_and_is_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("up");
        let script = format!("[ -e {} ] && echo /usr/bin/nope-x", flag.display());
        let host = fake_container("sh", &["-c", &script, "sh"], dir.path());
        assert_eq!(resolve_program(&host, "nope-x", dir.path()), None);
        // The container comes up: the next lookup must reach it again.
        std::fs::write(&flag, "").unwrap();
        assert_eq!(
            resolve_program(&host, "nope-x", dir.path()),
            Some("/usr/bin/nope-x".to_string())
        );
    }
}
