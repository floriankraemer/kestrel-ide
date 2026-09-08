//! Cargo diagnostics, read from `--message-format=json` (B1-2).
//!
//! The toolchain we dogfood is the one toolchain that will not be parsed
//! with a regex. Cargo emits one JSON object per line, each carrying the
//! rendered message *and* the exact spans, so a diagnostic's file, line and
//! column are read rather than recovered — and a message that happens to
//! contain something shaped like `foo.rs:12:3` cannot be mistaken for a
//! second diagnostic.

use std::path::Path;

use crate::diagnostics::{severity_from_word, BuildDiagnostic};

/// The argument that makes Cargo emit the JSON this module reads. Appended
/// by the build spec for a Cargo build, and nowhere else.
pub const MESSAGE_FORMAT_ARG: &str = "--message-format=json";

/// Parse one line of Cargo's JSON stream.
///
/// Returns nothing for the lines that are not diagnostics — `build-script`
/// output, artifact notifications, the final `build-finished` — and for a
/// diagnostic with no primary span, which is Cargo's shape for a summary
/// like "aborting due to 3 previous errors".
pub fn parse_line(line: &str, project_root: &Path) -> Option<BuildDiagnostic> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("reason")?.as_str()? != "compiler-message" {
        return None;
    }
    let message = value.get("message")?;

    let spans = message.get("spans")?.as_array()?;
    let primary = spans
        .iter()
        .find(|span| span.get("is_primary").and_then(|p| p.as_bool()) == Some(true))?;

    let file = primary.get("file_name")?.as_str()?;
    let path = crate::diagnostics::resolve_path(file, project_root);

    Some(BuildDiagnostic {
        path,
        line: primary
            .get("line_start")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        column: primary
            .get("column_start")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        severity: severity_from_word(message.get("level")?.as_str()?),
        message: message.get("message")?.as_str()?.to_string(),
        code: message
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Severity;
    use std::path::PathBuf;

    const ERROR: &str = r#"{"reason":"compiler-message","message":{"message":"mismatched types","code":{"code":"E0308"},"level":"error","spans":[{"file_name":"src/main.rs","line_start":4,"column_start":9,"is_primary":true}]}}"#;

    #[test]
    fn a_compiler_message_becomes_a_diagnostic_at_its_primary_span() {
        let diagnostic = parse_line(ERROR, Path::new("/p")).unwrap();
        assert_eq!(diagnostic.path, PathBuf::from("/p/src/main.rs"));
        assert_eq!(diagnostic.line, 4);
        assert_eq!(diagnostic.column, 9);
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.message, "mismatched types");
        assert_eq!(diagnostic.code, "E0308");
    }

    #[test]
    fn an_absolute_path_is_not_joined_onto_the_project_root() {
        let line = ERROR.replace("src/main.rs", "/elsewhere/lib.rs");
        let diagnostic = parse_line(&line, Path::new("/p")).unwrap();
        assert_eq!(diagnostic.path, PathBuf::from("/elsewhere/lib.rs"));
    }

    #[test]
    fn a_non_primary_span_is_never_the_diagnostics_location() {
        let line = r#"{"reason":"compiler-message","message":{"message":"unused","level":"warning","spans":[{"file_name":"a.rs","line_start":1,"column_start":1,"is_primary":false},{"file_name":"b.rs","line_start":9,"column_start":2,"is_primary":true}]}}"#;
        let diagnostic = parse_line(line, Path::new("/p")).unwrap();
        assert_eq!(diagnostic.path, PathBuf::from("/p/b.rs"));
        assert_eq!(diagnostic.line, 9);
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(diagnostic.code, "", "no code is empty, never a placeholder");
    }

    #[test]
    fn lines_that_are_not_diagnostics_are_skipped() {
        for line in [
            r#"{"reason":"compiler-artifact","target":{"name":"app"}}"#,
            r#"{"reason":"build-finished","success":false}"#,
            r#"{"reason":"compiler-message","message":{"message":"aborting due to 1 previous error","level":"error","spans":[]}}"#,
            "   ",
            "warning: unused variable (plain text, not JSON)",
        ] {
            assert!(parse_line(line, Path::new("/p")).is_none(), "{line}");
        }
    }

    #[test]
    fn a_message_containing_something_that_looks_like_a_location_is_not_reparsed() {
        let line = r#"{"reason":"compiler-message","message":{"message":"expected `src/other.rs:99:1`","level":"error","spans":[{"file_name":"src/main.rs","line_start":4,"column_start":9,"is_primary":true}]}}"#;
        let diagnostic = parse_line(line, Path::new("/p")).unwrap();
        assert_eq!(diagnostic.path, PathBuf::from("/p/src/main.rs"));
        assert_eq!(diagnostic.line, 4);
    }
}
