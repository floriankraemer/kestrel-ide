//! Q3 — run a code formatter over a buffer and hand back the formatted text
//! (ADR-0070).
//!
//! The same two buffer strategies the analyzers use, with the same
//! host-aware run: a `temp-copy` tool rewrites a dotfile beside the
//! original ([`crate::write_temp_copy`]) that is read back, a `stdin` tool
//! prints the result. A formatter is whole-file by construction; mapping the
//! result back onto an editor buffer is the caller's job.

use std::path::{Path, PathBuf};
use std::time::Duration;

use plugin_api::FormatterContribution;
use process_exec::host::ExecHost;

use crate::buffer::{write_temp_copy, BufferStrategy};

/// One formatter, ready to run: a [`FormatterContribution`] with its buffer
/// strategy parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatterDef {
    pub id: String,
    pub name: String,
    pub languages: Vec<String>,
    pub program_candidates: Vec<String>,
    pub args: Vec<String>,
    /// `Stdin` or `TempCopy` only; the manifest loader rejects the rest.
    pub buffer: BufferStrategy,
    pub success_exit_codes: Vec<i32>,
    pub requires_interpreter: Option<String>,
}

impl FormatterDef {
    pub fn from_contribution(c: &FormatterContribution) -> Self {
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            languages: c.languages.clone(),
            program_candidates: c.program_candidates.clone(),
            args: c.args.clone(),
            buffer: match c.buffer.as_deref() {
                Some("stdin") => BufferStrategy::Stdin,
                _ => BufferStrategy::TempCopy,
            },
            success_exit_codes: c.success_exit_codes.clone(),
            requires_interpreter: c.requires_interpreter.clone(),
        }
    }

    /// The argv for a run reading `file` (the temp copy, or the real path
    /// for a stdin tool's `--stdin-path={file}` hint).
    fn run_args(&self, file: &Path) -> Vec<String> {
        let file = file.to_string_lossy();
        self.args
            .iter()
            .map(|a| a.replace("{file}", &file))
            .collect()
    }
}

/// Why a format run produced no text. Each variant carries what the user
/// needs to act on, not a raw `io::Error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// No candidate program resolved on the host.
    NotInstalled,
    TimedOut,
    /// The tool exited with a code outside `success-exit-codes`.
    Failed {
        code: Option<i32>,
        stderr: String,
    },
    /// A stdin tool printed nothing, or text that is not UTF-8.
    BadOutput(&'static str),
    Io(String),
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => write!(f, "the formatter is not installed"),
            Self::TimedOut => write!(f, "the formatter timed out"),
            Self::Failed { code, stderr } => {
                let stderr = stderr.trim();
                match code {
                    Some(code) if stderr.is_empty() => {
                        write!(f, "the formatter exited with {code}")
                    }
                    Some(code) => write!(f, "the formatter exited with {code}: {stderr}"),
                    None => write!(f, "the formatter was terminated"),
                }
            }
            Self::BadOutput(why) => write!(f, "the formatter {why}"),
            Self::Io(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for FormatError {}

/// Format `text` (the buffer of `path`) with `def`, running on `host`.
///
/// `php_binary` is the configured interpreter for a `requires-interpreter =
/// "php"` tool. Returns the formatted text, which equals `text` when the
/// file was already formatted.
pub fn format(
    def: &FormatterDef,
    text: &str,
    path: &Path,
    project_root: &Path,
    host: &ExecHost,
    php_binary: &str,
    timeout: Duration,
) -> Result<String, FormatError> {
    let program = crate::find_program_on(&def.program_candidates, project_root, host)
        .ok_or(FormatError::NotInstalled)?;
    let (program, prefix) = match def.requires_interpreter.as_deref() {
        Some("php") => crate::php_invocation(&program, Some(php_binary)),
        _ => (program, Vec::new()),
    };

    let temp = match def.buffer {
        BufferStrategy::TempCopy => Some(
            write_temp_copy(project_root, path, text.as_bytes())
                .map_err(|e| FormatError::Io(format!("could not write a temporary copy: {e}")))?,
        ),
        _ => None,
    };
    let target: PathBuf = temp
        .as_ref()
        .map_or_else(|| path.to_path_buf(), |t| t.path().to_path_buf());
    let args: Vec<String> = prefix.into_iter().chain(def.run_args(&target)).collect();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let stdin = (def.buffer == BufferStrategy::Stdin).then_some(text.as_bytes());

    let output = process_exec::run_on(
        host,
        &program.to_string_lossy(),
        &arg_refs,
        project_root,
        stdin,
        timeout,
        &[],
    )
    .map_err(|e| match e {
        process_exec::Failure::NotFound => FormatError::NotInstalled,
        process_exec::Failure::TimedOut => FormatError::TimedOut,
        process_exec::Failure::Io(msg) => FormatError::Io(msg),
    })?;

    let code = output.status.code();
    if !code.is_some_and(|c| def.success_exit_codes.contains(&c)) {
        return Err(FormatError::Failed {
            code,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    match temp {
        Some(temp) => {
            let bytes = std::fs::read(temp.path())
                .map_err(|e| FormatError::Io(format!("could not read the formatted copy: {e}")))?;
            String::from_utf8(bytes).map_err(|_| FormatError::BadOutput("produced non-UTF-8 text"))
        }
        None => {
            // An empty reply for a non-empty buffer is a tool that failed
            // quietly; applying it would erase the file.
            if output.stdout.is_empty() && !text.is_empty() {
                return Err(FormatError::BadOutput("printed nothing"));
            }
            String::from_utf8(output.stdout)
                .map_err(|_| FormatError::BadOutput("produced non-UTF-8 text"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contribution(buffer: Option<&str>) -> FormatterContribution {
        FormatterContribution {
            id: "pint".into(),
            name: "Pint".into(),
            languages: vec!["php".into()],
            program_candidates: vec!["vendor/bin/pint".into()],
            args: vec!["--quiet".into(), "{file}".into()],
            buffer: buffer.map(String::from),
            success_exit_codes: vec![0],
            config_file_candidates: vec![],
            composer_package: None,
            requires_interpreter: None,
        }
    }

    #[test]
    fn an_absent_buffer_is_a_temp_copy_and_stdin_is_kept() {
        assert_eq!(
            FormatterDef::from_contribution(&contribution(None)).buffer,
            BufferStrategy::TempCopy
        );
        assert_eq!(
            FormatterDef::from_contribution(&contribution(Some("stdin"))).buffer,
            BufferStrategy::Stdin
        );
    }

    #[test]
    fn the_placeholder_is_replaced_in_every_argument() {
        let def = FormatterDef::from_contribution(&contribution(None));
        assert_eq!(
            def.run_args(Path::new("/p/.A.php.tmp.php")),
            vec!["--quiet", "/p/.A.php.tmp.php"]
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            FormatError::Failed {
                code: Some(2),
                stderr: " boom\n".into()
            }
            .to_string(),
            "the formatter exited with 2: boom"
        );
    }
}
