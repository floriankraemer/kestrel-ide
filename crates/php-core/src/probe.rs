//! The interpreter probe: one `php -r` call that reports the version, the
//! loaded `php.ini`, and whether Xdebug/PCOV are loaded (and Xdebug's mode).

use std::fmt;
use std::path::Path;
use std::time::Duration;

use process_exec::host::ExecHost;
use process_exec::{run_on, Failure};
use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(10);

/// The script `php -r` runs; prints one JSON object.
const SCRIPT: &str = concat!(
    "echo json_encode(['version'=>PHP_VERSION,'ini'=>php_ini_loaded_file(),",
    "'xdebug'=>extension_loaded('xdebug'),'pcov'=>extension_loaded('pcov'),",
    "'xdebug_mode'=>ini_get('xdebug.mode')]);"
);

/// What the interpreter reported about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpProbe {
    /// `8.3.6`, as `PHP_VERSION`.
    pub version: String,
    /// The loaded `php.ini`, if any.
    pub ini_file: Option<String>,
    pub xdebug: bool,
    /// Xdebug's `xdebug.mode` words (`debug`, `coverage`, ...); empty when
    /// Xdebug is absent or the mode is `off`.
    pub xdebug_modes: Vec<String>,
    pub pcov: bool,
}

impl PhpProbe {
    /// Xdebug is loaded with `debug` among its modes.
    pub fn can_debug(&self) -> bool {
        self.xdebug && self.xdebug_modes.iter().any(|m| m == "debug")
    }

    /// The first thing wrong for debugging, if anything.
    pub fn xdebug_issue(&self) -> Option<XdebugIssue> {
        if !self.xdebug {
            Some(XdebugIssue::NotLoaded)
        } else if !self.can_debug() {
            Some(XdebugIssue::DebugModeOff)
        } else {
            None
        }
    }

    /// Some coverage driver is available (PCOV, or Xdebug in `coverage` mode).
    pub fn can_cover(&self) -> bool {
        self.pcov || (self.xdebug && self.xdebug_modes.iter().any(|m| m == "coverage"))
    }
}

/// What stands between this interpreter and a debug session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XdebugIssue {
    /// The extension is not loaded: no debug run can connect.
    NotLoaded,
    /// Loaded, but `xdebug.mode` in the ini lacks `debug`. A run the IDE
    /// starts sets `XDEBUG_MODE=debug` itself, so only requests started
    /// elsewhere are affected.
    DebugModeOff,
}

impl XdebugIssue {
    /// A debug run cannot work at all.
    pub fn blocks_debugging(&self) -> bool {
        matches!(self, Self::NotLoaded)
    }

    /// What to tell the user, with the container hint when PHP runs in one.
    pub fn advice(&self, in_container: bool) -> String {
        let mut text = match self {
            Self::NotLoaded => "Xdebug is not loaded in this PHP interpreter. Install the \
                 xdebug extension and enable it in php.ini."
                .to_string(),
            Self::DebugModeOff => "Xdebug is loaded, but xdebug.mode in php.ini does not \
                 include `debug`. Debug runs started from the IDE enable it themselves; set \
                 xdebug.mode=debug to debug requests started elsewhere."
                .to_string(),
        };
        if in_container {
            text.push_str(CONTAINER_HINT);
        }
        text
    }
}

/// On Linux Docker `host.docker.internal` only resolves when the container
/// is told how.
pub const CONTAINER_HINT: &str = " On Linux Docker, the container also needs \
     `extra_hosts: [\"host.docker.internal:host-gateway\"]` (or \
     `--add-host=host.docker.internal:host-gateway`) so Xdebug can reach the IDE.";

/// Why the probe produced nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum ProbeError {
    NotFound,
    TimedOut,
    /// The interpreter ran but failed or printed something unexpected.
    Failed(String),
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "the PHP interpreter was not found"),
            Self::TimedOut => write!(f, "the PHP interpreter did not answer in time"),
            Self::Failed(m) => write!(f, "the PHP interpreter probe failed: {m}"),
        }
    }
}

impl std::error::Error for ProbeError {}

/// Probe `interpreter` on `host`, from `cwd`.
pub fn probe(host: &ExecHost, interpreter: &str, cwd: &Path) -> Result<PhpProbe, ProbeError> {
    let out =
        run_on(host, interpreter, &["-r", SCRIPT], cwd, None, TIMEOUT, &[]).map_err(
            |f| match f {
                Failure::NotFound => ProbeError::NotFound,
                Failure::TimedOut => ProbeError::TimedOut,
                Failure::Io(m) => ProbeError::Failed(m),
            },
        )?;
    if !out.status.success() {
        return Err(ProbeError::Failed(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// Parse the probe script's output. Public for tests and for callers that
/// obtained the output by other means.
pub fn parse(output: &str) -> Result<PhpProbe, ProbeError> {
    let value: Value = serde_json::from_str(output.trim())
        .map_err(|e| ProbeError::Failed(format!("unexpected output: {e}")))?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| ProbeError::Failed("no PHP version in the output".into()))?
        .to_string();
    let flag = |key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
    let xdebug = flag("xdebug");
    let xdebug_modes = value
        .get("xdebug_mode")
        .and_then(Value::as_str)
        .filter(|_| xdebug)
        .map(|modes| {
            modes
                .split(',')
                .map(str::trim)
                .filter(|m| !m.is_empty() && *m != "off")
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(PhpProbe {
        version,
        ini_file: value.get("ini").and_then(Value::as_str).map(str::to_string),
        xdebug,
        xdebug_modes,
        pcov: flag("pcov"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_report() {
        let p = parse(
            r#"{"version":"8.3.6","ini":"/etc/php.ini","xdebug":true,"pcov":false,"xdebug_mode":"debug, coverage"}"#,
        )
        .unwrap();
        assert_eq!(p.version, "8.3.6");
        assert_eq!(p.ini_file.as_deref(), Some("/etc/php.ini"));
        assert_eq!(p.xdebug_modes, ["debug", "coverage"]);
        assert!(p.can_debug() && p.can_cover());
    }

    #[test]
    fn without_xdebug_nothing_debugs() {
        let p = parse(
            r#"{"version":"8.1.0","ini":false,"xdebug":false,"pcov":true,"xdebug_mode":false}"#,
        )
        .unwrap();
        assert_eq!(p.ini_file, None);
        assert!(!p.can_debug());
        assert!(p.can_cover());
    }

    #[test]
    fn the_xdebug_issue_is_the_first_thing_wrong() {
        let report = |json: &str| parse(json).unwrap().xdebug_issue();
        assert_eq!(
            report(r#"{"version":"8.3.0","xdebug":false}"#),
            Some(XdebugIssue::NotLoaded)
        );
        assert_eq!(
            report(r#"{"version":"8.3.0","xdebug":true,"xdebug_mode":"coverage"}"#),
            Some(XdebugIssue::DebugModeOff)
        );
        assert_eq!(
            report(r#"{"version":"8.3.0","xdebug":true,"xdebug_mode":"debug"}"#),
            None
        );
    }

    #[test]
    fn only_a_missing_extension_blocks_and_a_container_adds_the_host_hint() {
        assert!(XdebugIssue::NotLoaded.blocks_debugging());
        assert!(!XdebugIssue::DebugModeOff.blocks_debugging());
        assert!(!XdebugIssue::NotLoaded
            .advice(false)
            .contains("host-gateway"));
        assert!(XdebugIssue::NotLoaded.advice(true).contains("host-gateway"));
    }

    #[test]
    fn xdebug_mode_off_is_empty() {
        let p = parse(r#"{"version":"8.2.1","xdebug":true,"xdebug_mode":"off"}"#).unwrap();
        assert!(p.xdebug && p.xdebug_modes.is_empty());
        assert!(!p.can_debug() && !p.can_cover());
    }

    #[test]
    fn garbage_is_a_failure() {
        assert!(matches!(parse("PHP Warning"), Err(ProbeError::Failed(_))));
        assert!(matches!(parse("{}"), Err(ProbeError::Failed(_))));
    }

    #[cfg(unix)]
    #[test]
    fn probes_a_stub_interpreter() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("php");
        std::fs::write(
            &stub,
            "#!/bin/sh\necho '{\"version\":\"8.4.1\",\"xdebug\":false,\"pcov\":false}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = probe(&ExecHost::Local, stub.to_str().unwrap(), dir.path()).unwrap();
        assert_eq!(p.version, "8.4.1");
    }

    #[test]
    fn a_missing_interpreter_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            probe(&ExecHost::Local, "/nonexistent/php-binary", dir.path()),
            Err(ProbeError::NotFound)
        );
    }
}
