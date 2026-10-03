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

use std::path::{Path, PathBuf};

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

impl CheckstyleFinding {
    /// This finding's rule id: its `source` attribute, or — for a tool that
    /// reports none and prefixes its messages with the rule (`Id: text`,
    /// Psalm) — that prefix.
    pub fn code(&self, analyzer: &AnalyzerDef) -> Option<String> {
        if let Some(source) = self.source.as_deref().filter(|s| !s.is_empty()) {
            return Some(source.to_string());
        }
        if !analyzer.code_in_message {
            return None;
        }
        let (id, _) = self.message.split_once(": ")?;
        let is_identifier =
            !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        is_identifier.then(|| id.to_string())
    }
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
            // `unescape_value()` only exists when quick-xml's `encoding`
            // feature is off (its own doc comment warns that depending on
            // it "will fail to compile" the moment anything else in the
            // build enables that feature — `db-exchange`'s `calamine`
            // dependency now does, workspace-wide, via feature
            // unification). `normalized_value` is the feature-agnostic
            // replacement it names; `Implicit1_0` matches this parser's
            // only-ever-seen-as-1.0 checkstyle XML, and it does not
            // resolve any entity beyond the five predefined XML ones,
            // which is all a `name`/`message`/`source` attribute needs.
            let value = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|e| ParseError(e.to_string()))?
                .into_owned();
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// The local file a checkstyle `file=` names for a run rooted at `root`.
///
/// A tool run under WSL prints Linux paths, which this process cannot open
/// until [`process_exec::host::ExecHost::path_from_tool`] translates them;
/// a relative path (PHPStan reports relative to its working directory,
/// which is always `root`) is resolved against `root` so the file's URI
/// matches the one the editor opened.
pub fn locate_file(root: &Path, file: &str) -> PathBuf {
    locate_file_on(&process_exec::host::ExecHost::for_path(root), root, file)
}

/// [`locate_file`] for a tool that ran on `host`: a container's paths
/// under the project mount map back to the local file.
pub fn locate_file_on(host: &process_exec::host::ExecHost, root: &Path, file: &str) -> PathBuf {
    let path = host.path_from_tool(file);
    if path.is_relative() {
        root.join(path)
    } else {
        path
    }
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
            code: f.code(analyzer),
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
            languages: vec![],
            file_args: vec![],
            buffer: None,
            composer_package: None,
            requires_interpreter: None,
            suppress_comment: None,
            code_in_message: false,
            fixer: None,
            config_file_candidates: vec![],
            ruleset_default: None,
            project_paths_config: vec![],
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
    fn a_relative_finding_path_resolves_against_the_project_root() {
        assert_eq!(
            locate_file(Path::new("/p"), "src/a.php"),
            PathBuf::from("/p/src/a.php")
        );
        assert_eq!(
            locate_file(Path::new("/p"), "/p/src/a.php"),
            PathBuf::from("/p/src/a.php")
        );
    }

    #[test]
    fn a_linux_finding_path_under_a_wsl_root_becomes_a_unc_path() {
        assert_eq!(
            locate_file(
                Path::new("//wsl.localhost/Ubuntu/home/f/proj"),
                "/home/f/proj/src/a.php"
            ),
            PathBuf::from("//wsl.localhost/Ubuntu/home/f/proj/src/a.php")
        );
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

    #[test]
    fn a_file_a_container_tool_printed_maps_back_to_the_local_file() {
        let root = Path::new("/home/f/proj");
        let host = process_exec::host::ExecHost::Container(process_exec::host::ContainerHost {
            program: "docker".into(),
            prefix_args: vec![],
            engine_env: vec![],
            via_wsl: false,
            verb_args: vec![],
            target: vec![],
            path_map: process_exec::host::PathMap::new(root, "/var/www"),
        });
        assert_eq!(
            locate_file_on(&host, root, "/var/www/src/A.php"),
            root.join("src/A.php")
        );
        // PHPStan prints paths relative to the working directory.
        assert_eq!(
            locate_file_on(&host, root, "src/A.php"),
            root.join("src/A.php")
        );
    }

    fn finding(source: Option<&str>, message: &str) -> CheckstyleFinding {
        CheckstyleFinding {
            file: "a.php".into(),
            line: 1,
            column: None,
            severity: "error".into(),
            message: message.into(),
            source: source.map(String::from),
        }
    }

    #[test]
    fn the_code_is_the_source_attribute() {
        let def = phpstan();
        assert_eq!(
            finding(Some("variable.undefined"), "x")
                .code(&def)
                .as_deref(),
            Some("variable.undefined")
        );
        assert_eq!(finding(None, "Foo: bar").code(&def), None);
    }

    #[test]
    fn a_code_in_message_tool_reads_the_message_prefix() {
        let mut def = phpstan();
        def.code_in_message = true;
        let f = finding(None, "UndefinedVariable: Cannot find $x");
        assert_eq!(f.code(&def).as_deref(), Some("UndefinedVariable"));
        assert_eq!(finding(None, "no prefix here").code(&def), None);
        assert_eq!(finding(None, "Has spaces: x").code(&def), None);
    }

    #[test]
    fn diagnostics_carry_the_code() {
        let diagnostics = to_diagnostics(&[finding(Some("a.b"), "m")], "a.php", &phpstan());
        assert_eq!(diagnostics[0].code.as_deref(), Some("a.b"));
    }
}
