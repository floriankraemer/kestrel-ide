//! Which shells this machine actually offers, as a catalogue the UI can put
//! in a menu and the settings file can name by id.
//!
//! [`crate::ShellSpec`] deliberately promises that "no OS probing happens in
//! constructors" so a caller can inject an explicit shell and the choice
//! stays testable on any single platform.
//! That promise is intact: probing lives here, in a function a caller asks
//! for by name, and never in a constructor.
//!
//! The same testability rule applies inside this module.
//! Each platform's list is built by a pure function taking the machine's
//! answers as arguments — `$SHELL`, `/etc/shells`, `wsl.exe --list`, and a
//! "can this be launched?" predicate — so the Windows catalogue is covered
//! by tests on Linux CI, which is the property `WindowsShellKind` was
//! introduced for (ADR-0007).

use std::path::Path;

use crate::ShellSpec;

/// One shell the user can pick, as offered by this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCandidate {
    /// Stable across releases and across machines, because this is what
    /// gets written into `settings.toml`: `system`, `bash`, `zsh`, `pwsh`,
    /// `cmd`, `wsl:Ubuntu`, …
    pub id: String,
    /// What the menu shows. Never parsed.
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

impl ShellCandidate {
    fn new(id: &str, label: &str, program: &str, args: &[&str]) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            program: program.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
        }
    }

    /// The spawnable form. Working directory and environment are the
    /// caller's to add — a candidate says *which* shell, never *where*.
    pub fn to_spec(&self) -> ShellSpec {
        ShellSpec::new(self.program.clone(), self.args.clone())
    }
}

/// Every shell this machine offers, most-preferred first.
///
/// The first entry, when there is one, is the platform default — the same
/// shell [`ShellSpec::unix_default`] resolves to on Unix.
pub fn detect() -> Vec<ShellCandidate> {
    // `cfg!`, not `#[cfg]`: both branches then compile — and are checked,
    // and are reachable from tests — on every platform, which is the whole
    // reason the per-platform lists take their inputs as arguments.
    if cfg!(windows) {
        detect_windows()
    } else {
        detect_unix()
    }
}

/// The candidate with this id, or `None` if the machine no longer offers it
/// — a settings file naming a shell that has since been uninstalled is a
/// normal thing to find, not an error.
pub fn find(id: &str) -> Option<ShellCandidate> {
    detect().into_iter().find(|candidate| candidate.id == id)
}

/// Whether `program` can be launched: an absolute path is checked directly,
/// a bare name is looked up on `PATH` the way the OS itself would.
fn launchable(program: &str) -> bool {
    let path = Path::new(program);
    if path.is_absolute() {
        return path.is_file();
    }
    let Some(search) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&search).any(|dir| dir.join(program).is_file())
}

// ---------------------------------------------------------------- Unix ----

fn detect_unix() -> Vec<ShellCandidate> {
    let shell_env = std::env::var("SHELL").ok();
    let etc_shells = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    unix_candidates(shell_env.as_deref(), &etc_shells, launchable)
}

/// Shells to look for when `/etc/shells` is missing or says nothing useful,
/// which is the normal state inside a slim container image.
const UNIX_FALLBACKS: &[&str] = &[
    "/bin/bash",
    "/bin/zsh",
    "/usr/bin/zsh",
    "/bin/fish",
    "/usr/bin/fish",
    "/bin/sh",
];

/// Accounts are disabled by pointing their shell at one of these, so they
/// appear in `/etc/shells` on some distributions without being usable.
const NOT_A_SHELL: &[&str] = &["nologin", "false", "sync"];

/// Build the Unix/macOS catalogue from this machine's answers.
///
/// `$SHELL` leads, as the shell the user already chose outside this IDE.
/// Everything after it is deduplicated by id — two paths to the same shell
/// (`/bin/zsh` and `/usr/bin/zsh`) are one entry, and the shell that is
/// already the default is not offered twice under its own name.
fn unix_candidates(
    shell_env: Option<&str>,
    etc_shells: &str,
    launchable: impl Fn(&str) -> bool,
) -> Vec<ShellCandidate> {
    let mut candidates = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    if let Some(program) = shell_env.filter(|program| !program.is_empty()) {
        if launchable(program) {
            let name = shell_name(program);
            candidates.push(ShellCandidate::new(
                "system",
                &format!("Default ({name})"),
                program,
                &[],
            ));
            seen.push(name);
        }
    }

    let listed = etc_shells.lines().map(str::trim).filter(|line| {
        !line.is_empty()
            && !line.starts_with('#')
            && !NOT_A_SHELL.contains(&shell_name(line).as_str())
    });
    for program in listed.chain(UNIX_FALLBACKS.iter().copied()) {
        let id = shell_name(program);
        if seen.contains(&id) || !launchable(program) {
            continue;
        }
        candidates.push(ShellCandidate::new(&id, &id, program, &[]));
        seen.push(id);
    }

    candidates
}

/// The last path segment: `/usr/bin/zsh` → `zsh`. Both the id and the label
/// of a Unix candidate, which is why it is one function.
fn shell_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}

// ------------------------------------------------------------- Windows ----

fn detect_windows() -> Vec<ShellCandidate> {
    let distros = std::process::Command::new("wsl.exe")
        .args(["--list", "--quiet"])
        .output()
        .map(|output| decode_utf16le(&output.stdout))
        .unwrap_or_default();

    let git_bash: Vec<String> = ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .map(|dir| format!("{dir}\\Git\\bin\\bash.exe"))
        .collect();
    let git_bash: Vec<&str> = git_bash.iter().map(String::as_str).collect();

    windows_candidates(&distros, &git_bash, launchable)
}

/// Build the Windows catalogue from this machine's answers.
///
/// PowerShell 7 leads where it exists, matching what a Windows user who
/// installed it expects, and Windows PowerShell — which every machine has —
/// is the floor.
fn windows_candidates(
    wsl_list: &str,
    git_bash_paths: &[&str],
    launchable: impl Fn(&str) -> bool,
) -> Vec<ShellCandidate> {
    let mut candidates = Vec::new();

    for (id, label, program) in [
        ("pwsh", "PowerShell", "pwsh.exe"),
        ("powershell", "Windows PowerShell", "powershell.exe"),
        ("cmd", "Command Prompt", "cmd.exe"),
    ] {
        if launchable(program) {
            candidates.push(ShellCandidate::new(id, label, program, &[]));
        }
    }

    if let Some(path) = git_bash_paths.iter().find(|path| launchable(path)) {
        candidates.push(ShellCandidate::new("git-bash", "Git Bash", path, &[]));
    }

    for distro in wsl_distros(wsl_list) {
        candidates.push(ShellCandidate::new(
            &format!("wsl:{distro}"),
            &format!("{distro} (WSL)"),
            "wsl.exe",
            &["-d", &distro],
        ));
    }

    candidates
}

/// Distro names out of `wsl.exe --list --quiet`, already decoded.
///
/// `--quiet` drops the header, but a default distro is still marked with a
/// trailing ` (Default)` in some Windows builds, and every line carries the
/// `\r` of a CRLF stream.
fn wsl_distros(wsl_list: &str) -> Vec<String> {
    wsl_list
        .lines()
        .map(|line| line.trim().trim_end_matches("(Default)").trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// `wsl.exe` writes UTF-16LE, so its output is mostly NUL bytes to anything
/// expecting UTF-8 — decoding it as such yields one distro name per *two*
/// bytes of garbage, which is why this is spelled out rather than left to
/// `String::from_utf8_lossy`.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine offering exactly the named programs.
    fn offering(available: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |program| available.contains(&program)
    }

    #[test]
    fn the_users_own_shell_leads_the_unix_list() {
        let candidates = unix_candidates(
            Some("/bin/zsh"),
            "/bin/bash\n/bin/zsh\n",
            offering(&["/bin/zsh", "/bin/bash"]),
        );
        assert_eq!(candidates[0].id, "system");
        assert_eq!(candidates[0].label, "Default (zsh)");
        assert_eq!(candidates[0].program, "/bin/zsh");
    }

    /// The default shell is not offered a second time under its own name:
    /// picking "zsh" and picking "Default (zsh)" would spawn the same thing.
    #[test]
    fn the_default_shell_is_not_listed_twice() {
        let candidates = unix_candidates(
            Some("/bin/zsh"),
            "/bin/bash\n/bin/zsh\n",
            offering(&["/bin/zsh", "/bin/bash"]),
        );
        let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["system", "bash"]);
    }

    /// Two paths to one shell are one entry — otherwise the id, which is
    /// what `settings.toml` stores, would not be unique.
    #[test]
    fn two_paths_to_the_same_shell_collapse_to_one_entry() {
        let candidates = unix_candidates(
            None,
            "/bin/zsh\n/usr/bin/zsh\n",
            offering(&["/bin/zsh", "/usr/bin/zsh"]),
        );
        let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["zsh"]);
        assert_eq!(candidates[0].program, "/bin/zsh");
    }

    #[test]
    fn comments_blank_lines_and_disabled_accounts_are_not_shells() {
        let candidates = unix_candidates(
            None,
            "# /etc/shells\n\n/usr/sbin/nologin\n/bin/false\n/bin/bash\n",
            offering(&["/usr/sbin/nologin", "/bin/false", "/bin/bash"]),
        );
        let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["bash"]);
    }

    /// A slim container image has no `/etc/shells` at all.
    #[test]
    fn a_missing_etc_shells_falls_back_to_the_usual_paths() {
        let candidates = unix_candidates(None, "", offering(&["/bin/sh"]));
        let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["sh"]);
    }

    /// `$SHELL` pointing at something uninstalled is not a reason to offer
    /// nothing — the rest of the machine's shells still work.
    #[test]
    fn an_unlaunchable_shell_env_is_skipped_rather_than_offered() {
        let candidates =
            unix_candidates(Some("/bin/gone"), "/bin/bash\n", offering(&["/bin/bash"]));
        let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["bash"]);
    }

    #[test]
    fn a_candidate_spawns_its_program_and_args() {
        let spec = ShellCandidate::new("wsl:Ubuntu", "Ubuntu (WSL)", "wsl.exe", &["-d", "Ubuntu"])
            .to_spec();
        assert_eq!(spec.program, "wsl.exe");
        assert_eq!(spec.args, vec!["-d".to_string(), "Ubuntu".to_string()]);
        assert_eq!(spec.cwd, None);
        assert!(spec.env.is_empty());
    }

    // The Windows catalogue is tested here, on Linux CI, for the reason
    // ADR-0007 gave `WindowsShellKind`: there is no Windows runner, and a
    // list built from injected answers needs none.

    #[test]
    fn windows_offers_every_shell_it_finds_powershell_first() {
        let candidates = windows_candidates(
            "",
            &["C:\\Program Files\\Git\\bin\\bash.exe"],
            offering(&[
                "pwsh.exe",
                "powershell.exe",
                "cmd.exe",
                "C:\\Program Files\\Git\\bin\\bash.exe",
            ]),
        );
        let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["pwsh", "powershell", "cmd", "git-bash"]);
    }

    /// PowerShell 7 is an optional install; a stock machine has the other two.
    #[test]
    fn a_machine_without_powershell_7_still_offers_the_stock_shells() {
        let candidates = windows_candidates("", &[], offering(&["powershell.exe", "cmd.exe"]));
        let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["powershell", "cmd"]);
    }

    #[test]
    fn each_wsl_distro_becomes_its_own_candidate() {
        let candidates = windows_candidates("Ubuntu\r\ndebian\r\n", &[], offering(&[]));
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].id, "wsl:Ubuntu");
        assert_eq!(candidates[0].label, "Ubuntu (WSL)");
        assert_eq!(candidates[0].program, "wsl.exe");
        assert_eq!(
            candidates[0].args,
            vec!["-d".to_string(), "Ubuntu".to_string()]
        );
        assert_eq!(candidates[1].id, "wsl:debian");
    }

    #[test]
    fn the_default_wsl_distro_keeps_its_bare_name() {
        let candidates = windows_candidates("Ubuntu (Default)\r\n", &[], offering(&[]));
        assert_eq!(candidates[0].id, "wsl:Ubuntu");
    }

    #[test]
    fn a_machine_without_wsl_offers_no_distros() {
        assert!(windows_candidates("", &[], offering(&[])).is_empty());
    }

    /// The one that bites: `wsl.exe` writes UTF-16LE with a BOM, and
    /// reading it as UTF-8 yields a name interleaved with NULs.
    #[test]
    fn wsl_output_is_decoded_as_utf16le_not_utf8() {
        let mut bytes = vec![0xFF, 0xFE]; // BOM
        for unit in "Ubuntu\r\n".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_utf16le(&bytes), "Ubuntu\r\n");

        let candidates = windows_candidates(&decode_utf16le(&bytes), &[], offering(&[]));
        assert_eq!(candidates[0].id, "wsl:Ubuntu");
    }
}
