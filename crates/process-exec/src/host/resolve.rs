//! Tool discovery on a remote host: where a program lives inside a WSL
//! distro or a container, probed once and memoised.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use super::{container_unavailable_reason, ContainerHost, ExecHost};
use crate::suppress_console_window;

type ResolveCache = Mutex<HashMap<(String, String), Option<String>>>;

fn resolve_cache() -> &'static ResolveCache {
    static CACHE: OnceLock<ResolveCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve `program` to an absolute path *inside the distro* `host` names,
/// memoised per `(distro, candidate)` ([`probe_candidate`]) for the process's lifetime — one probe
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

    let candidate = probe_candidate(host, program, cwd);
    let key = (wsl.distro.clone(), candidate.clone());
    if let Some(cached) = resolve_cache().lock().unwrap().get(&key) {
        return cached.clone().ok_or(None);
    }

    let resolved = if is_path(program) {
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
    let candidate = probe_candidate(host, program, cwd);
    let key = (
        format!(
            "container:{} {:?} {:?}",
            container.program, container.prefix_args, container.target
        ),
        candidate.clone(),
    );
    if let Some(Some(hit)) = resolve_cache().lock().unwrap().get(&key) {
        return Ok(hit.clone());
    }

    let script = if is_path(program) {
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
    // A path candidate is tested by its remote path, a bare name looked up
    // on the container's PATH; a remote path always has a `/`, a bare name
    // never does, so the script tells them apart the same way.
    let remote: Vec<String> = candidates
        .iter()
        .map(|candidate| probe_candidate(host, candidate, cwd))
        .collect();
    let key = (
        format!(
            "container-first:{} {:?} {:?}",
            container.program, container.prefix_args, container.target
        ),
        remote.join("\0"),
    );
    if let Some(Some(hit)) = resolve_cache().lock().unwrap().get(&key) {
        return Some(hit.clone());
    }
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

fn is_path(program: &str) -> bool {
    program.contains('/') || program.contains('\\')
}

/// What a probe looks up, and what its answer is memoised under: a path
/// candidate as its absolute remote path — `vendor/bin/phpunit` from a
/// package is a different file than from the project root — a bare name
/// as is.
fn probe_candidate(host: &ExecHost, program: &str, cwd: &Path) -> String {
    if is_path(program) {
        host.to_remote(&cwd.join(program))
    } else {
        program.to_string()
    }
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

#[cfg(test)]
mod tests {
    use super::super::tests::{prepend_to_path, restore_path, write_executable_script, PATH_LOCK};
    use super::super::{PathMap, WslHost};
    use super::*;

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

    #[test]
    fn a_package_miss_does_not_poison_the_root_probe_in_wsl() {
        let _guard = PATH_LOCK.lock().unwrap();
        let bin_dir = tempfile::tempdir().unwrap();
        write_executable_script(
            bin_dir.path(),
            "wsl.exe",
            "case \"$*\" in\n  *packages/a/vendor/bin/phpunit*) exit 1;;\n  *vendor/bin/phpunit*) exit 0;;\n  *) exit 1;;\nesac\n",
        );
        let host = ExecHost::Wsl(WslHost {
            distro: "PoisonTest".into(),
            unc_prefix: "//wsl.localhost/PoisonTest".into(),
        });
        let root = Path::new("//wsl.localhost/PoisonTest/proj");

        let original_path = prepend_to_path(bin_dir.path());
        let package = resolve_program(&host, "vendor/bin/phpunit", &root.join("packages/a"));
        let project = resolve_program(&host, "vendor/bin/phpunit", root);
        restore_path(original_path);

        assert_eq!(package, None);
        assert_eq!(project.as_deref(), Some("/proj/vendor/bin/phpunit"));
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

    fn write_tool(dir: &Path) {
        std::fs::create_dir_all(dir.join("vendor/bin")).unwrap();
        write_executable_script(&dir.join("vendor/bin"), "phpunit", "exit 0\n");
    }

    #[test]
    fn a_package_hit_is_not_reused_for_the_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("starts");
        let host = counting_container(dir.path(), &count);
        let package = dir.path().join("packages/a");
        write_tool(&package);
        write_tool(dir.path());
        let root_tool = dir.path().join("vendor/bin/phpunit");
        let package_tool = package.join("vendor/bin/phpunit");

        let from_package = resolve_program(&host, "vendor/bin/phpunit", &package);
        let from_root = resolve_program(&host, "vendor/bin/phpunit", dir.path());
        assert_eq!(
            from_package,
            Some(package_tool.to_string_lossy().into_owned())
        );
        assert_eq!(from_root, Some(root_tool.to_string_lossy().into_owned()));

        let list = vec!["vendor/bin/phpunit".to_string()];
        assert_eq!(
            resolve_first(&host, &list, &package),
            Some(package_tool.to_string_lossy().into_owned())
        );
        assert_eq!(
            resolve_first(&host, &list, dir.path()),
            Some(root_tool.to_string_lossy().into_owned())
        );
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
