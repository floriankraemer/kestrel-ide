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

    pub fn is_remote(&self) -> bool {
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

    /// Build the argv this host actually runs: unchanged for `Local`,
    /// wrapped in `wsl.exe -d <distro> --cd <linux-cwd> -e <program>
    /// <args...>` for `Wsl` — `-e`, not `--`, so a commit message or a
    /// `--message-format=json` argument is never reinterpreted by a shell.
    pub fn argv(&self, program: &str, args: &[&str], cwd: &Path) -> (String, Vec<String>) {
        match self {
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
    /// Windows-side working directory (`--cd` decides the *remote* cwd;
    /// this is what `wsl.exe` itself is launched from), `env` set on the
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
        let (resolved_program, resolved_args) = self.argv(program, args, cwd);
        let mut command = Command::new(&resolved_program);
        command.args(&resolved_args).current_dir(cwd);

        for (key, value) in env {
            command.env(key, value);
        }
        if self.is_remote() && !env.is_empty() {
            let inherited = std::env::var("WSLENV").unwrap_or_default();
            let mut names: Vec<String> = if inherited.is_empty() {
                Vec::new()
            } else {
                inherited.split(':').map(str::to_string).collect()
            };
            for (key, _) in env {
                names.push(format!("{key}/u"));
            }
            command.env("WSLENV", names.join(":"));
        }

        suppress_console_window(&mut command);
        command
    }
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
    let output = Command::new("wsl.exe").args(["--list", "--quiet"]).output();
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
    let ExecHost::Wsl(wsl) = host else {
        return Some(program.to_string());
    };

    let key = (wsl.distro.clone(), program.to_string());
    if let Some(cached) = resolve_cache().lock().unwrap().get(&key) {
        return cached.clone();
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
    resolved
}

fn probe_executable(distro: &str, remote_path: &str) -> bool {
    Command::new("wsl.exe")
        .args(["-d", distro, "-e", "test", "-x", remote_path])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn probe_command_v(distro: &str, program: &str) -> Option<String> {
    let script = format!("command -v {program}");
    let output = Command::new("wsl.exe")
        .args(["-d", distro, "-e", "/bin/sh", "-lc", &script])
        .output()
        .ok()?;
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
        assert!(host.is_remote());
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
}
