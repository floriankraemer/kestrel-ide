//! JUnit XML: the batch report format PHPUnit's `--log-junit` (and most
//! other frameworks) can emit — the fallback for a framework whose manifest
//! names no streaming format (D2; see ADR-0048 for why TeamCity is
//! preferred where available).
//!
//! Unlike [`crate::teamcity`], this parses a whole finished document at
//! once: JUnit XML has no per-test timeline, only the end state, so there
//! is nothing to stream — [`crate::tree::TestTree::apply_junit`] fills the
//! tree from the complete list in one pass.
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8"?>
//! <testsuites>
//!  <testsuite name="Tests\GreeterTest">
//!   <testcase name="testGreets" time="0.001"/>
//!   <testcase name="testAdds" time="0.002">
//!    <failure type="AssertionError">Failed asserting that 1 matches 2.</failure>
//!   </testcase>
//!  </testsuite>
//! </testsuites>
//! ```

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::tree::{TestFailure, TestStatus};

/// One `<testcase>`, resolved to this crate's neutral status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JUnitTestCase {
    /// The enclosing `<testsuite name="...">`.
    pub suite: String,
    pub name: String,
    pub duration_ms: Option<u64>,
    pub status: TestStatus,
    pub failure: Option<TestFailure>,
}

/// Why a JUnit-XML document could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed junit-xml: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// Parse a whole JUnit-XML document into its test cases, across every
/// `<testsuite>` element (nested `<testsuites>` need not be present —
/// PHPUnit always writes one, but a single bare `<testsuite>` root is
/// accepted too).
pub fn parse(xml: &str) -> Result<Vec<JUnitTestCase>, ParseError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut cases = Vec::new();
    let mut current_suite = String::new();
    let mut current: Option<PendingCase> = None;
    // `<failure>`/`<error>`/`<skipped>` content is a text child, not an
    // attribute, so it is accumulated between that tag's start and end.
    let mut collecting: Option<(TestStatus, String)> = None;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(tag)) => match tag.name().as_ref() {
                b"testsuite" => {
                    current_suite = attr(&tag, "name")?.unwrap_or_default();
                }
                b"testcase" => {
                    current = Some(new_pending_case(&tag, &current_suite)?);
                }
                b"failure" | b"error" => {
                    let message = attr(&tag, "message")?.unwrap_or_default();
                    collecting = Some((TestStatus::Failed, message));
                }
                b"skipped" => {
                    collecting = Some((TestStatus::Skipped, String::new()));
                }
                _ => {}
            },
            Ok(Event::Empty(tag)) => match tag.name().as_ref() {
                b"testsuite" => {
                    current_suite = attr(&tag, "name")?.unwrap_or_default();
                }
                // A self-closing `<testcase .../>` (the common case: a
                // passing test with no child element) has no `End` event
                // to finalize it on, so it is pushed immediately here
                // rather than through `current`.
                b"testcase" => {
                    cases.push(new_pending_case(&tag, &current_suite)?.into());
                }
                b"failure" | b"error" => {
                    if let Some(case) = current.as_mut() {
                        case.status = TestStatus::Failed;
                        case.failure = Some(TestFailure {
                            message: attr(&tag, "message")?.unwrap_or_default(),
                            details: String::new(),
                        });
                    }
                }
                b"skipped" => {
                    if let Some(case) = current.as_mut() {
                        case.status = TestStatus::Skipped;
                    }
                }
                _ => {}
            },
            Ok(Event::Text(text)) => {
                if let Some((_, message)) = collecting.as_mut() {
                    let raw = text.decode().map_err(|e| ParseError(e.to_string()))?;
                    let decoded = quick_xml::escape::unescape(&raw)
                        .map_err(|e| ParseError(e.to_string()))?
                        .into_owned();
                    if message.is_empty() {
                        *message = decoded;
                    } else {
                        message.push('\n');
                        message.push_str(&decoded);
                    }
                }
            }
            Ok(Event::End(tag)) => match tag.name().as_ref() {
                b"failure" | b"error" | b"skipped" => {
                    if let (Some((status, message)), Some(case)) =
                        (collecting.take(), current.as_mut())
                    {
                        case.status = status;
                        if status == TestStatus::Failed {
                            case.failure = Some(TestFailure {
                                message: message.lines().next().unwrap_or("").to_string(),
                                details: message,
                            });
                        }
                    }
                }
                b"testcase" => {
                    if let Some(case) = current.take() {
                        cases.push(case.into());
                    }
                }
                _ => {}
            },
            Ok(_) => {}
            Err(e) => return Err(ParseError(e.to_string())),
        }
        buf.clear();
    }

    Ok(cases)
}

fn new_pending_case(
    tag: &quick_xml::events::BytesStart<'_>,
    suite: &str,
) -> Result<PendingCase, ParseError> {
    let name = attr(tag, "name")?.ok_or_else(|| ParseError("<testcase> is missing name".into()))?;
    let duration_ms = attr(tag, "time")?.and_then(|t| parse_seconds_to_ms(&t));
    Ok(PendingCase {
        suite: suite.to_string(),
        name,
        duration_ms,
        status: TestStatus::Passed,
        failure: None,
    })
}

struct PendingCase {
    suite: String,
    name: String,
    duration_ms: Option<u64>,
    status: TestStatus,
    failure: Option<TestFailure>,
}

impl From<PendingCase> for JUnitTestCase {
    fn from(case: PendingCase) -> Self {
        JUnitTestCase {
            suite: case.suite,
            name: case.name,
            duration_ms: case.duration_ms,
            status: case.status,
            failure: case.failure,
        }
    }
}

fn parse_seconds_to_ms(value: &str) -> Option<u64> {
    value
        .parse::<f64>()
        .ok()
        .map(|s| (s * 1000.0).round() as u64)
}

fn attr(tag: &quick_xml::events::BytesStart<'_>, key: &str) -> Result<Option<String>, ParseError> {
    for a in tag.attributes() {
        let a = a.map_err(|e| ParseError(e.to_string()))?;
        if a.key.as_ref() == key.as_bytes() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn a_passing_and_a_failing_case_are_both_reported() {
        let cases = parse(&fixture("junit_one_suite.xml")).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].suite, "Tests\\GreeterTest");
        assert_eq!(cases[0].name, "testGreets");
        assert_eq!(cases[0].status, TestStatus::Passed);
        assert_eq!(cases[0].duration_ms, Some(1));

        assert_eq!(cases[1].name, "testAdds");
        assert_eq!(cases[1].status, TestStatus::Failed);
        assert_eq!(
            cases[1].failure.as_ref().unwrap().message,
            "Failed asserting that 1 matches 2."
        );
    }

    #[test]
    fn a_skipped_case_has_no_failure() {
        let cases = parse(&fixture("junit_skipped.xml")).unwrap();
        assert_eq!(cases[0].status, TestStatus::Skipped);
        assert!(cases[0].failure.is_none());
    }

    #[test]
    fn an_error_element_is_treated_like_a_failure() {
        let cases = parse(&fixture("junit_error.xml")).unwrap();
        assert_eq!(cases[0].status, TestStatus::Failed);
        assert!(cases[0].failure.is_some());
    }

    #[test]
    fn multiple_suites_each_attribute_their_own_cases() {
        let cases = parse(&fixture("junit_two_suites.xml")).unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].suite, "Tests\\ATest");
        assert_eq!(cases[1].suite, "Tests\\BTest");
    }

    #[test]
    fn malformed_xml_is_a_typed_error_not_a_panic() {
        let err = parse("<testsuites><testsuite></testsuites>").unwrap_err();
        assert!(!err.0.is_empty());
    }
}
