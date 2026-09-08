//! PHPUnit's `--teamcity` reporter: one `##teamcity[...]` service message
//! per line, streamed as the run progresses (D2 — the plan's primary
//! format; see ADR-0048 for why over JUnit XML alone).
//!
//! ```text
//! ##teamcity[testSuiteStarted name='Tests\GreeterTest']
//! ##teamcity[testStarted name='testGreets']
//! ##teamcity[testFinished name='testGreets' duration='3']
//! ##teamcity[testSuiteFinished name='Tests\GreeterTest']
//! ```
//!
//! [`TeamCityParser`] is a streaming, stateful parser: [`TeamCityParser::feed`]
//! takes an arbitrary chunk (a process's stdout arrives in whatever pieces
//! the OS pipe hands over, not necessarily whole lines) and returns every
//! complete [`TeamCityEvent`] found so far, buffering a trailing partial
//! line for the next call — the same incremental shape
//! `build_core::parser::DiagnosticParser::feed` already has.

use std::collections::HashMap;

use crate::tree::TestId;

/// One recognised service message, already resolved against the parser's
/// open-suite stack so `parent` is the enclosing suite's [`TestId`], if
/// any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamCityEvent {
    SuiteStarted {
        parent: Option<TestId>,
        name: String,
    },
    SuiteFinished {
        parent: Option<TestId>,
        name: String,
    },
    TestStarted {
        parent: Option<TestId>,
        name: String,
    },
    TestFinished {
        parent: Option<TestId>,
        name: String,
        duration_ms: Option<u64>,
    },
    TestFailed {
        parent: Option<TestId>,
        name: String,
        message: String,
        details: String,
    },
    TestIgnored {
        parent: Option<TestId>,
        name: String,
        message: String,
    },
}

/// Streaming parser: one instance per run, fed chunks as they arrive.
#[derive(Debug, Default)]
pub struct TeamCityParser {
    buffer: String,
    /// Currently open suites, outermost first — a test never nests
    /// another test, so only `testSuiteStarted`/`testSuiteFinished` push
    /// and pop this.
    stack: Vec<TestId>,
}

impl TeamCityParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of raw process output, returning every service message
    /// completed by it. A line with no recognised message (PHPUnit's own
    /// progress dots, a PHP notice) yields nothing — it is still valid
    /// output for the raw pane, just not a tree event, so the caller feeds
    /// the same chunk to the output pane separately.
    pub fn feed(&mut self, chunk: &str) -> Vec<TeamCityEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=pos).collect();
            if let Some(event) = self.parse_line(line.trim_end_matches(['\r', '\n'])) {
                events.push(event);
            }
        }
        events
    }

    /// Flush a trailing line with no terminating newline — a process's
    /// last write before it exits does not always end in one.
    pub fn finish(&mut self) -> Vec<TeamCityEvent> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let line = std::mem::take(&mut self.buffer);
        self.parse_line(line.trim_end_matches(['\r', '\n']))
            .into_iter()
            .collect()
    }

    fn parse_line(&mut self, line: &str) -> Option<TeamCityEvent> {
        let message = ServiceMessage::parse(line)?;
        let parent = self.stack.last().cloned();
        match message.name.as_str() {
            "testSuiteStarted" => {
                let name = message.get("name")?.to_string();
                let id = TestId::child(parent.as_ref(), &name);
                self.stack.push(id);
                Some(TeamCityEvent::SuiteStarted { parent, name })
            }
            "testSuiteFinished" => {
                let name = message.get("name")?.to_string();
                self.stack.pop();
                let parent = self.stack.last().cloned();
                Some(TeamCityEvent::SuiteFinished { parent, name })
            }
            "testStarted" => Some(TeamCityEvent::TestStarted {
                parent,
                name: message.get("name")?.to_string(),
            }),
            "testFinished" => Some(TeamCityEvent::TestFinished {
                parent,
                name: message.get("name")?.to_string(),
                duration_ms: message.get("duration").and_then(|d| d.parse().ok()),
            }),
            "testFailed" => Some(TeamCityEvent::TestFailed {
                parent,
                name: message.get("name")?.to_string(),
                message: message.get("message").unwrap_or_default().to_string(),
                details: message.get("details").unwrap_or_default().to_string(),
            }),
            "testIgnored" => Some(TeamCityEvent::TestIgnored {
                parent,
                name: message.get("name")?.to_string(),
                message: message.get("message").unwrap_or_default().to_string(),
            }),
            _ => None,
        }
    }
}

/// One `##teamcity[name key='value' ...]` line, decoded.
struct ServiceMessage {
    name: String,
    attrs: HashMap<String, String>,
}

impl ServiceMessage {
    fn get(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(String::as_str)
    }

    /// `None` for anything not shaped like a service message at all —
    /// PHPUnit's plain-text progress output, warnings, deprecation
    /// notices — which is the common case and not an error.
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        let inner = line.strip_prefix("##teamcity[")?.strip_suffix(']')?;
        let name_end = inner.find(' ').unwrap_or(inner.len());
        let name = inner[..name_end].to_string();
        if name.is_empty() {
            return None;
        }
        let mut rest = &inner[name_end..];
        let mut attrs = HashMap::new();
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                break;
            }
            let Some(eq) = rest.find('=') else { break };
            let key = rest[..eq].to_string();
            rest = &rest[eq + 1..];
            if !rest.starts_with('\'') {
                break;
            }
            rest = &rest[1..];
            let (value, remainder) = read_quoted_value(rest);
            attrs.insert(key, value);
            rest = remainder;
        }
        Some(ServiceMessage { name, attrs })
    }
}

/// Read a TeamCity-escaped value from the start of `rest` up to (and past)
/// its closing `'`, returning the decoded value and what follows it.
///
/// Escapes: `|'` -> `'`, `|n` -> newline, `|r` -> carriage return,
/// `|[`/`|]` -> literal bracket, `||` -> `|`. Anything else after a `|` is
/// passed through unescaped rather than treated as an error — a future
/// escape this parser doesn't know about should degrade, not panic.
fn read_quoted_value(rest: &str) -> (String, &str) {
    let mut value = String::new();
    let chars = rest.char_indices();
    let mut escape = false;
    for (idx, c) in chars {
        if escape {
            match c {
                '\'' => value.push('\''),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                '[' => value.push('['),
                ']' => value.push(']'),
                '|' => value.push('|'),
                other => value.push(other),
            }
            escape = false;
            continue;
        }
        match c {
            '|' => escape = true,
            '\'' => return (value, &rest[idx + 1..]),
            _ => value.push(c),
        }
    }
    // No closing quote found — malformed input; return what was read and
    // an empty remainder rather than panicking on a truncated line.
    (value, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(text: &str) -> Vec<TeamCityEvent> {
        let mut parser = TeamCityParser::new();
        let mut events = parser.feed(text);
        events.extend(parser.finish());
        events
    }

    #[test]
    fn a_plain_test_reports_started_then_finished() {
        let out = events(
            "##teamcity[testStarted name='testGreets']\n\
             ##teamcity[testFinished name='testGreets' duration='3']\n",
        );
        assert_eq!(
            out,
            vec![
                TeamCityEvent::TestStarted {
                    parent: None,
                    name: "testGreets".into()
                },
                TeamCityEvent::TestFinished {
                    parent: None,
                    name: "testGreets".into(),
                    duration_ms: Some(3),
                },
            ]
        );
    }

    #[test]
    fn a_suite_wraps_its_tests_with_the_suite_as_parent() {
        let out = events(
            "##teamcity[testSuiteStarted name='Tests\\GreeterTest']\n\
             ##teamcity[testStarted name='testGreets']\n\
             ##teamcity[testFinished name='testGreets' duration='3']\n\
             ##teamcity[testSuiteFinished name='Tests\\GreeterTest']\n",
        );
        let TeamCityEvent::TestStarted { parent, .. } = &out[1] else {
            panic!("expected TestStarted, got {:?}", out[1]);
        };
        assert_eq!(parent.as_ref().unwrap().as_str(), "Tests\\GreeterTest");
    }

    #[test]
    fn a_failed_test_carries_message_and_details() {
        let out = events(
            "##teamcity[testStarted name='testAdds']\n\
             ##teamcity[testFailed name='testAdds' message='Failed asserting that 1 matches 2.' details='at FooTest.php:12']\n",
        );
        assert_eq!(
            out[1],
            TeamCityEvent::TestFailed {
                parent: None,
                name: "testAdds".into(),
                message: "Failed asserting that 1 matches 2.".into(),
                details: "at FooTest.php:12".into(),
            }
        );
    }

    #[test]
    fn escaped_pipe_bracket_and_newline_are_decoded() {
        let out = events(
            "##teamcity[testFailed name='t' message='a |'quoted|' word' details='line1|nline2 |[bracketed|]']\n",
        );
        let TeamCityEvent::TestFailed {
            message, details, ..
        } = &out[0]
        else {
            panic!("expected TestFailed, got {:?}", out[0]);
        };
        assert_eq!(message, "a 'quoted' word");
        assert_eq!(details, "line1\nline2 [bracketed]");
    }

    #[test]
    fn non_service_message_lines_are_ignored() {
        let out = events("PHPUnit 10.5.0 by Sebastian Bergmann and contributors.\n...F..\n");
        assert!(out.is_empty());
    }

    #[test]
    fn ignored_tests_carry_their_message() {
        let out = events("##teamcity[testIgnored name='testSkip' message='needs php-ext']\n");
        assert_eq!(
            out[0],
            TeamCityEvent::TestIgnored {
                parent: None,
                name: "testSkip".into(),
                message: "needs php-ext".into(),
            }
        );
    }

    #[test]
    fn a_chunk_split_mid_line_is_buffered_until_complete() {
        let mut parser = TeamCityParser::new();
        assert!(parser.feed("##teamcity[testStarted nam").is_empty());
        let events = parser.feed("e='testX']\n");
        assert_eq!(
            events,
            vec![TeamCityEvent::TestStarted {
                parent: None,
                name: "testX".into()
            }]
        );
    }

    #[test]
    fn a_trailing_line_with_no_newline_is_recovered_by_finish() {
        let mut parser = TeamCityParser::new();
        assert!(parser
            .feed("##teamcity[testStarted name='testX']")
            .is_empty());
        assert_eq!(
            parser.finish(),
            vec![TeamCityEvent::TestStarted {
                parent: None,
                name: "testX".into()
            }]
        );
    }

    #[test]
    fn nested_suites_pop_back_to_the_right_parent() {
        let out = events(
            "##teamcity[testSuiteStarted name='Outer']\n\
             ##teamcity[testSuiteStarted name='Inner']\n\
             ##teamcity[testStarted name='deep']\n\
             ##teamcity[testFinished name='deep' duration='1']\n\
             ##teamcity[testSuiteFinished name='Inner']\n\
             ##teamcity[testStarted name='shallow']\n",
        );
        let TeamCityEvent::TestStarted { parent, name } = &out[5] else {
            panic!("expected TestStarted, got {:?}", out[5]);
        };
        assert_eq!(name, "shallow");
        assert_eq!(parent.as_ref().unwrap().as_str(), "Outer");
    }
}
