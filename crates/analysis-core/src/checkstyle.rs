//! `checkstyle-xml`: the report format PHP_CodeSniffer's `--report=checkstyle`
//! (and PHPStan's `--error-format=checkstyle`) emit.
//!
//! Named as its own output-format id (`AnalyzerContribution::output_format
//! == "checkstyle-xml"`, B2) rather than baked into a `phpcs`-specific
//! parser, because it is Checkstyle's own report shape: several PHP and JVM
//! linters speak it, so one parser here serves every analyzer contribution
//! that names this format, present or future.
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8"?>
//! <checkstyle version="3.7.1">
//!  <file name="/project/src/Greeter.php">
//!   <error line="10" column="5" severity="error" message="Undefined variable: $name" source="PHPStan.undefinedVariable"/>
//!  </file>
//! </checkstyle>
//! ```

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::AnalyzerDef;
use diagnostics_core::{Diagnostic, Position, Range};

/// One `<error>` (or `<warning>`, which Checkstyle treats identically)
/// element, still in the tool's own vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckstyleFinding {
    /// The path from the enclosing `<file name="...">`.
    pub file: String,
    /// 1-based, as Checkstyle counts it and as the Problems dock/editor
    /// both already expect.
    pub line: u32,
    /// 1-based when present; Checkstyle allows a line-only finding.
    pub column: Option<u32>,
    /// The tool's own severity word (`"error"`, `"warning"`, ...),
    /// unparsed — [`to_diagnostics`] resolves it through an
    /// [`AnalyzerDef`]'s `severity-map`.
    pub severity: String,
    pub message: String,
    /// The rule id, e.g. `PSR2.Files.EndFileNewline`. Carried through for
    /// a future quick-fix table (out of scope here, per the plan) but kept
    /// on the finding rather than discarded, since re-parsing the same XML
    /// later to recover it would be wasted work.
    pub source: Option<String>,
}

/// Why a checkstyle-xml document could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed checkstyle-xml: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// Parse a whole checkstyle-xml document into its findings, across every
/// `<file>` element.
pub fn parse(xml: &str) -> Result<Vec<CheckstyleFinding>, ParseError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut findings = Vec::new();
    let mut current_file = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(tag)) | Ok(Event::Empty(tag)) => {
                let name = tag.name();
                let local = name.as_ref();
                if local == b"file" {
                    current_file = attr(&tag, "name")?.unwrap_or_default();
                } else if local == b"error" || local == b"warning" {
                    let line = attr(&tag, "line")?
                        .ok_or_else(|| ParseError("<error> is missing line".into()))?
                        .parse::<u32>()
                        .map_err(|e| ParseError(format!("line is not a number: {e}")))?;
                    let column = attr(&tag, "column")?
                        .map(|v| {
                            v.parse::<u32>()
                                .map_err(|e| ParseError(format!("column is not a number: {e}")))
                        })
                        .transpose()?;
                    let severity = attr(&tag, "severity")?.unwrap_or_else(|| "error".to_string());
                    let message = attr(&tag, "message")?
                        .ok_or_else(|| ParseError("<error> is missing message".into()))?;
                    let source = attr(&tag, "source")?;
                    findings.push(CheckstyleFinding {
                        file: current_file.clone(),
                        line,
                        column,
                        severity,
                        message,
                        source,
                    });
                }
            }
            Ok(_) => {}
            Err(e) => return Err(ParseError(e.to_string())),
        }
        buf.clear();
    }

    Ok(findings)
}

fn attr(tag: &quick_xml::events::BytesStart<'_>, key: &str) -> Result<Option<String>, ParseError> {
    for a in tag.attributes() {
        let a = a.map_err(|e| ParseError(e.to_string()))?;
        if a.key.as_ref() == key.as_bytes() {
            // `normalized_value` needs an `XmlVersion` this crate has no
            // reason to track; plain attribute unescaping is all a
            // checkstyle report's `name`/`message`/`source` need.
            #[allow(deprecated)]
            let value = a
                .unescape_value()
                .map_err(|e| ParseError(e.to_string()))?
                .into_owned();
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// Turn every finding for `path` into a [`Diagnostic`], resolving each
/// one's severity through `analyzer`'s `severity-map` and naming the
/// diagnostic's source after the analyzer (`phpstan`, `phpcs`, ...) rather
/// than the rule id, matching every other source in the shared store.
///
/// Checkstyle findings are 1-based lines/columns; [`Position`] is 0-based
/// (ADR-0046), so both are shifted down by one here, at the one place a
/// checkstyle finding becomes the shared model, rather than leaving every
/// caller to remember the offset.
pub fn to_diagnostics(
    findings: &[CheckstyleFinding],
    path: &str,
    analyzer: &AnalyzerDef,
) -> Vec<Diagnostic> {
    findings
        .iter()
        .filter(|f| f.file == path)
        .map(|f| Diagnostic {
            range: Range {
                start: Position {
                    line: f.line.saturating_sub(1),
                    character: f.column.unwrap_or(1).saturating_sub(1),
                },
                end: None,
            },
            severity: analyzer.severity_for(&f.severity),
            message: f.message.clone(),
            source: analyzer.name.clone(),
            raw: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_api::AnalyzerContribution;

    fn phpstan() -> AnalyzerDef {
        AnalyzerDef::from_contribution(&AnalyzerContribution {
            id: "phpstan".into(),
            name: "phpstan".into(),
            program_candidates: vec!["phpstan".into()],
            args: vec![],
            output_format: "checkstyle-xml".into(),
            severity_map: [
                ("error".to_string(), "error".to_string()),
                ("warning".to_string(), "warning".to_string()),
            ]
            .into_iter()
            .collect(),
        })
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn one_file_two_findings() {
        let findings = parse(&fixture("checkstyle_one_file.xml")).unwrap();
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "/project/src/Greeter.php");
        assert_eq!(findings[0].line, 10);
        assert_eq!(findings[0].column, Some(5));
        assert_eq!(findings[0].severity, "error");
        assert_eq!(findings[0].message, "Undefined variable: $name");
        assert_eq!(
            findings[0].source.as_deref(),
            Some("PHPStan.undefinedVariable")
        );
        assert_eq!(findings[1].severity, "warning");
        assert_eq!(findings[1].column, None);
    }

    #[test]
    fn multiple_files_are_each_attributed_correctly() {
        let findings = parse(&fixture("checkstyle_two_files.xml")).unwrap();
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].file, "/project/src/A.php");
        assert_eq!(findings[1].file, "/project/src/B.php");
    }

    #[test]
    fn an_empty_report_has_no_findings() {
        let findings = parse(&fixture("checkstyle_empty.xml")).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn malformed_xml_is_a_typed_error_not_a_panic() {
        let err = parse("<checkstyle><file name=\"x\"><error line=\"nope\"/></file>").unwrap_err();
        assert!(err.0.contains("line"), "{err}");
    }

    #[test]
    fn findings_become_diagnostics_with_zero_based_positions_and_mapped_severity() {
        let findings = parse(&fixture("checkstyle_one_file.xml")).unwrap();
        let diagnostics = to_diagnostics(&findings, "/project/src/Greeter.php", &phpstan());
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].range.start.line, 9);
        assert_eq!(diagnostics[0].range.start.character, 4);
        assert_eq!(diagnostics[0].severity, diagnostics_core::Severity::Error);
        assert_eq!(diagnostics[1].severity, diagnostics_core::Severity::Warning);
        assert_eq!(diagnostics[0].source, "phpstan");
    }

    #[test]
    fn diagnostics_are_filtered_to_the_requested_path() {
        let findings = parse(&fixture("checkstyle_two_files.xml")).unwrap();
        let diagnostics = to_diagnostics(&findings, "/project/src/A.php", &phpstan());
        assert_eq!(diagnostics.len(), 1);
    }
}
