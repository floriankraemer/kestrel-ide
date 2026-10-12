//! PHPUnit 12 reports a test whose `#[DataProvider]` is invalid (missing
//! method, not iterable, throws) only in the text summary at the end of its
//! stdout; `--teamcity` emits no `testStarted`/`testFailed` for it, so the
//! Tests tree and the gutter would show the test as never run (#412):
//!
//! ```text
//! There was 1 PHPUnit error:
//!
//! 1) App\Tests\ProviderTest::testBad
//! The data provider App\Tests\ProviderTest::missing specified for App\Tests\ProviderTest::testBad is invalid
//! Method App\Tests\ProviderTest::missing() does not exist
//!
//! /w/tests/ProviderTest.php:8
//! ```
//!
//! [`InvalidProviderScanner`] reads that summary from the run's output and
//! synthesises the events the reporter left out. The generic TeamCity parser
//! stays framework-agnostic; this is PHPUnit's text, so it lives here.

use std::collections::{HashMap, HashSet};

use test_core::parse_location_hint;
use test_core::{TeamCityEvent, TestId};

use crate::tests::EventLocator;

#[derive(Debug, Default)]
pub struct InvalidProviderScanner {
    /// The trailing partial line of the output so far.
    partial: String,
    /// The numbered summary entry being read, header first; ends at a blank line.
    entry: Vec<String>,
    /// File of each class suite the run reported, as the run printed it.
    class_files: HashMap<String, String>,
    /// Tests the reporter did report, which need no synthesising.
    reported: HashSet<TestId>,
    /// Suites this run's tree has seen start, the reporter's and our own: a
    /// class whose tests all have invalid providers never gets one (#412).
    started_suites: HashSet<TestId>,
}

impl InvalidProviderScanner {
    /// Note an event the run reported, for the files and tests it names.
    pub fn observe(&mut self, event: &TeamCityEvent) {
        match event {
            TeamCityEvent::SuiteStarted {
                parent,
                name,
                location,
            } => {
                self.started_suites.insert(child_id(parent.as_ref(), name));
                if let Some(location) = location.as_ref().filter(|l| !l.file.is_empty()) {
                    self.class_files.insert(name.clone(), location.file.clone());
                }
            }
            TeamCityEvent::TestStarted { parent, name, .. } => {
                self.reported.insert(child_id(parent.as_ref(), name));
            }
            _ => {}
        }
    }

    /// Feed a chunk of the run's output (ANSI already stripped); returns the
    /// events for every invalid-provider entry it completed.
    pub fn feed(&mut self, chunk: &str, locator: &EventLocator) -> Vec<TeamCityEvent> {
        self.partial.push_str(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=end).collect();
            let line = line.trim_end_matches(['\r', '\n']);
            if line.trim().is_empty() {
                // PHPUnit puts a blank line between the explanation and the
                // `file:line` that ends the entry, so only the blank after
                // that line (or a second one) closes it.
                match self.entry.last() {
                    Some(last) if last.is_empty() || is_file_line(last) => {
                        events.extend(self.close_entry(locator));
                    }
                    Some(_) => self.entry.push(String::new()),
                    None => {}
                }
            } else if is_entry_header(line) {
                events.extend(self.close_entry(locator));
                self.entry.push(line.to_string());
            } else if !self.entry.is_empty() {
                self.entry.push(line.to_string());
            }
        }
        events
    }

    fn close_entry(&mut self, locator: &EventLocator) -> Vec<TeamCityEvent> {
        let entry = std::mem::take(&mut self.entry);
        let Some((class, method, message, details)) = invalid_provider_entry(&entry) else {
            return Vec::new();
        };
        let parent = TestId(class.clone());
        if self.reported.contains(&child_id(Some(&parent), &method)) {
            return Vec::new();
        }
        let file = self
            .class_files
            .get(&class)
            .cloned()
            .or_else(|| locator.class_file(&class))
            // The summary names the file too: all that is known of a class
            // whose tests the reporter never started.
            .or_else(|| details.rsplit_once(':').map(|(file, _)| file.to_string()))
            .filter(|file| !file.is_empty());
        let location = file
            .as_ref()
            .and_then(|file| parse_location_hint(&format!("php_qn://{file}::\\{class}::{method}")));
        let mut events = Vec::new();
        if self.started_suites.insert(parent.clone()) {
            events.push(TeamCityEvent::SuiteStarted {
                parent: None,
                name: class.clone(),
                location: file
                    .as_ref()
                    .and_then(|file| parse_location_hint(&format!("php_qn://{file}::\\{class}"))),
            });
        }
        let parent = Some(parent);
        events.extend([
            TeamCityEvent::TestStarted {
                parent: parent.clone(),
                name: method.clone(),
                location,
            },
            TeamCityEvent::TestFailed {
                parent: parent.clone(),
                name: method.clone(),
                message,
                details,
                comparison: None,
            },
            TeamCityEvent::TestFinished {
                parent,
                name: method,
                duration_ms: None,
            },
        ]);
        events
    }
}

/// How the tree names a test under its suite.
fn child_id(parent: Option<&TestId>, name: &str) -> TestId {
    match parent {
        Some(parent) => TestId(format!("{}::{name}", parent.as_str())),
        None => TestId(name.to_string()),
    }
}

/// `3) Ns\Class::testMethod`
fn is_entry_header(line: &str) -> bool {
    let Some((number, rest)) = line.split_once(") ") else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) && method_of(rest).is_some()
}

/// The class and method of `Ns\Class::testMethod`; a data-set suffix or any
/// other trailing text makes it no plain method.
fn method_of(subject: &str) -> Option<(&str, &str)> {
    let (class, method) = subject.rsplit_once("::")?;
    let plain = !method.is_empty() && method.chars().all(|c| c.is_alphanumeric() || c == '_');
    (plain && !class.is_empty() && !class.contains(' ')).then_some((class, method))
}

/// `(class, method, message, details)` of a summary entry about an invalid
/// data provider: the explanation lines are the message, the trailing
/// `file:line` the details.
fn invalid_provider_entry(entry: &[String]) -> Option<(String, String, String, String)> {
    let (header, body) = entry.split_first()?;
    let (class, method) = method_of(header.split_once(") ")?.1)?;
    let headline = body.first()?;
    if !(headline.contains("data provider") && headline.ends_with("is invalid")) {
        return None;
    }
    let (details, explanation) = match body.split_last() {
        Some((last, rest)) if !rest.is_empty() && is_file_line(last) => (last.clone(), rest),
        _ => (String::new(), body),
    };
    let blanks = explanation
        .iter()
        .rev()
        .take_while(|l| l.is_empty())
        .count();
    let explanation = &explanation[..explanation.len() - blanks];
    Some((
        class.to_string(),
        method.to_string(),
        explanation.join("\n"),
        details,
    ))
}

/// `/w/tests/ProviderTest.php:8`
fn is_file_line(line: &str) -> bool {
    line.rsplit_once(':').is_some_and(|(file, number)| {
        file.ends_with(".php") && !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use process_exec::host::ExecHost;
    use std::path::PathBuf;
    use test_core::TeamCityParser;

    const OUTPUT: &str = include_str!("fixtures/phpunit12_invalid_data_provider.txt");

    fn locator() -> EventLocator {
        EventLocator::new(Vec::new(), ExecHost::Local, PathBuf::from("/w"))
    }

    /// The run's events as the sink sees them: the reporter's, then the
    /// scanner's, the output arriving in `chunk`-byte pieces.
    fn replay(output: &str, chunk: usize) -> Vec<TeamCityEvent> {
        let (mut parser, mut scanner) = (TeamCityParser::new(), InvalidProviderScanner::default());
        let mut all = Vec::new();
        let chars: Vec<char> = output.chars().collect();
        for piece in chars.chunks(chunk) {
            let piece: String = piece.iter().collect();
            for event in parser.feed(&piece) {
                scanner.observe(&event);
                all.push(event);
            }
            all.extend(scanner.feed(&piece, &locator()));
        }
        all
    }

    fn failures(events: &[TeamCityEvent]) -> Vec<(&str, &str, &str)> {
        events
            .iter()
            .filter_map(|e| match e {
                TeamCityEvent::TestFailed {
                    parent: Some(parent),
                    name,
                    message,
                    ..
                } => Some((parent.as_str(), name.as_str(), message.as_str())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_invalid_provider_becomes_a_failed_test_under_its_class() {
        let events = replay(OUTPUT, usize::MAX);
        let got = failures(&events);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "App\\Tests\\ProviderTest");
        assert_eq!(got[0].1, "testBad");
        assert!(got[0]
            .2
            .contains("Method App\\Tests\\ProviderTest::missing() does not exist"));
        assert_eq!(got[1].1, "testBad2");
        assert!(got[1].2.contains("does not return an iterable"));
    }

    #[test]
    fn the_synthesised_test_points_at_its_method_in_the_class_file() {
        let events = replay(OUTPUT, usize::MAX);
        let location = events.iter().find_map(|e| match e {
            TeamCityEvent::TestStarted { name, location, .. } if name == "testBad" => {
                location.clone()
            }
            _ => None,
        });
        let location = location.expect("testBad starts with a location");
        assert_eq!(location.file, "/w/tests/ProviderTest.php");
        assert_eq!(
            location.symbol,
            Some(test_core::TestSymbol::Method {
                class: "App\\Tests\\ProviderTest".into(),
                method: "testBad".into()
            })
        );
    }

    #[test]
    fn a_class_whose_tests_all_have_invalid_providers_gets_its_suite_synthesised() {
        // Real PHPUnit 12.5 output: no ##teamcity line at all, exit code 2.
        let events = replay(
            include_str!("fixtures/phpunit12_invalid_provider_no_class_suite.txt"),
            usize::MAX,
        );
        let class = "App\\Tests\\OnlyBadTest";
        let suites: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                TeamCityEvent::SuiteStarted {
                    parent,
                    name,
                    location,
                } => Some((parent.clone(), name.as_str(), location.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(suites.len(), 1, "one suite for both tests");
        assert_eq!((suites[0].0.clone(), suites[0].1), (None, class));
        assert_eq!(
            suites[0].2.as_ref().map(|l| l.file.as_str()),
            Some("/w/tests/OnlyBadTest.php")
        );
        assert!(matches!(events[0], TeamCityEvent::SuiteStarted { .. }));

        let mut tree = test_core::TestTree::new();
        for event in events {
            tree.apply(event);
        }
        assert_eq!(tree.roots(), &[TestId(class.into())], "no orphan nodes");
        assert_eq!(tree.failed_leaf_ids().len(), 2);
    }

    #[test]
    fn the_output_may_arrive_in_any_pieces() {
        assert_eq!(replay(OUTPUT, 7), replay(OUTPUT, usize::MAX));
    }

    #[test]
    fn ordinary_failures_in_the_summary_are_left_alone() {
        let summary = "There was 1 failure:\n\n1) App\\FooTest::testX\nFailed asserting that false is true.\n\n/w/tests/FooTest.php:9\n\nFAILURES!\n";
        assert!(failures(&replay(summary, usize::MAX)).is_empty());
    }

    #[test]
    fn a_test_the_reporter_did_report_is_not_reported_twice() {
        let output = format!(
            "##teamcity[testSuiteStarted name='App\\Tests\\ProviderTest']\n##teamcity[testStarted name='testBad']\n{}",
            "\n1) App\\Tests\\ProviderTest::testBad\nThe data provider specified for App\\Tests\\ProviderTest::testBad is invalid\n\n"
        );
        assert!(failures(&replay(&output, usize::MAX)).is_empty());
    }

    #[test]
    fn the_message_form_without_the_provider_name_is_recognised() {
        let summary = "1) App\\FooTest::testBad\nThe data provider specified for App\\FooTest::testBad is invalid\nboom\n\n/w/tests/FooTest.php:9\n\n";
        let events = replay(summary, usize::MAX);
        assert_eq!(failures(&events).len(), 1);
    }
}
