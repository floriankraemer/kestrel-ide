//! Code coverage (PHP parity plan T4): reading a Clover report and
//! summarising it per file and per directory.
//!
//! A framework that offers coverage names its report-writing arguments in
//! `coverage-args`; [`args`] fills in the report path. The report path is
//! project-relative so it lands in the project mount on every host, and
//! [`Coverage::map_paths_from`] turns the absolute paths the tool printed
//! (a container's `/app/src/Foo.php`) into local ones.
//!
//! ```xml
//! <coverage><project>
//!  <file name="/app/src/Greeter.php">
//!   <line num="7" type="stmt" count="3"/>
//!   <line num="9" type="stmt" count="0"/>
//!  </file>
//! </project></coverage>
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::Reader;

/// Where a coverage run writes its Clover report, relative to the project
/// root (`.ide/local/` is never committed).
pub const REPORT_PATH: &str = ".ide/local/coverage/clover.xml";

/// The environment that makes PHP collect coverage. Xdebug needs
/// `XDEBUG_MODE=coverage`; PCOV ignores it, so one answer fits both drivers.
pub fn env() -> Vec<(String, String)> {
    vec![("XDEBUG_MODE".to_string(), "coverage".to_string())]
}

/// `coverage_args` with `$COVERAGE_FILE$` replaced by [`REPORT_PATH`].
pub fn args(coverage_args: &[String]) -> Vec<String> {
    coverage_args
        .iter()
        .map(|arg| arg.replace("$COVERAGE_FILE$", REPORT_PATH))
        .collect()
}

/// One source file's line hits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCoverage {
    pub path: PathBuf,
    /// 1-based line to how often it ran, for every executable line.
    pub lines: BTreeMap<u32, u64>,
}

impl FileCoverage {
    pub fn covered(&self) -> usize {
        self.lines.values().filter(|hits| **hits > 0).count()
    }

    pub fn total(&self) -> usize {
        self.lines.len()
    }
}

/// A whole report.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Coverage {
    pub files: Vec<FileCoverage>,
}

/// A Clover document that is not well-formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "malformed clover report: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// One row of the Coverage dock: a file or a directory with the lines
/// beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageRow {
    /// Relative to the project root; empty for the root itself.
    pub path: PathBuf,
    pub is_file: bool,
    pub covered: usize,
    pub total: usize,
}

impl CoverageRow {
    /// Whether the row has any executable line: an interface or a file of
    /// declarations has none, and "0% (0/0)" would read as a failure.
    pub fn has_lines(&self) -> bool {
        self.total > 0
    }

    /// Share of executable lines that ran, 0 to 100; 0 for no lines.
    pub fn percent(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.covered as f64 * 100.0 / self.total as f64
        }
    }
}

fn attr(tag: &quick_xml::events::BytesStart<'_>, name: &str) -> Result<Option<String>, ParseError> {
    for attribute in tag.attributes() {
        let attribute = attribute.map_err(|e| ParseError(e.to_string()))?;
        if attribute.key.as_ref() == name.as_bytes() {
            let value = attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|e| ParseError(e.to_string()))?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

/// Parse a Clover report. Lines of every type (`stmt`, `method`, `cond`)
/// count as executable; a line listed twice keeps its highest hit count.
pub fn parse_clover(xml: &str) -> Result<Coverage, ParseError> {
    let mut reader = Reader::from_str(xml);
    let mut files: Vec<FileCoverage> = Vec::new();
    let mut current: Option<FileCoverage> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(tag)) | Ok(Event::Empty(tag)) => match tag.name().as_ref() {
                b"file" => {
                    if let Some(done) = current.take() {
                        files.push(done);
                    }
                    if let Some(name) = attr(&tag, "name")? {
                        current = Some(FileCoverage {
                            path: PathBuf::from(name),
                            lines: BTreeMap::new(),
                        });
                    }
                }
                b"line" => {
                    if let (Some(file), Some(num), Some(count)) =
                        (current.as_mut(), attr(&tag, "num")?, attr(&tag, "count")?)
                    {
                        if let (Ok(num), Ok(count)) = (num.parse::<u32>(), count.parse::<u64>()) {
                            let hits = file.lines.entry(num).or_insert(0);
                            *hits = (*hits).max(count);
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::End(tag)) if tag.name().as_ref() == b"file" => {
                if let Some(done) = current.take() {
                    files.push(done);
                }
            }
            Ok(_) => {}
            Err(e) => return Err(ParseError(e.to_string())),
        }
        buf.clear();
    }
    if let Some(done) = current.take() {
        files.push(done);
    }
    Ok(Coverage { files })
}

impl Coverage {
    /// The same report with every path as `host` printed it turned into a
    /// path this process can open (a container's or WSL distro's).
    pub fn map_paths_from(mut self, host: &process_exec::host::ExecHost) -> Self {
        for file in &mut self.files {
            file.path = host.path_from_tool(&file.path.to_string_lossy());
        }
        self
    }

    /// The hits of the file at `path`, if the report has it.
    pub fn file(&self, path: &Path) -> Option<&FileCoverage> {
        self.files.iter().find(|file| file.path == path)
    }

    /// Every directory and file under `root` with its rolled-up line
    /// counts, sorted by path so a parent precedes its children. Files
    /// outside `root` are left out. The root itself is the first row.
    pub fn rows(&self, root: &Path) -> Vec<CoverageRow> {
        let mut rows: BTreeMap<PathBuf, CoverageRow> = BTreeMap::new();
        for file in &self.files {
            let Ok(relative) = file.path.strip_prefix(root) else {
                continue;
            };
            let (covered, total) = (file.covered(), file.total());
            rows.insert(
                relative.to_path_buf(),
                CoverageRow {
                    path: relative.to_path_buf(),
                    is_file: true,
                    covered,
                    total,
                },
            );
            for ancestor in relative.ancestors().skip(1) {
                let row = rows
                    .entry(ancestor.to_path_buf())
                    .or_insert_with(|| CoverageRow {
                        path: ancestor.to_path_buf(),
                        is_file: false,
                        covered: 0,
                        total: 0,
                    });
                row.covered += covered;
                row.total += total;
            }
        }
        rows.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOVER: &str = r#"<?xml version="1.0"?>
<coverage generated="1"><project timestamp="1">
 <package name="App">
  <file name="/app/src/Greeter.php">
   <class name="Greeter" namespace="App"><metrics methods="1"/></class>
   <line num="7" type="method" name="greet" count="3"/>
   <line num="8" type="stmt" count="3"/>
   <line num="9" type="stmt" count="0"/>
   <metrics loc="10"/>
  </file>
 </package>
 <file name="/app/src/Util/Math.php">
  <line num="3" type="stmt" count="0"/>
  <line num="4" type="stmt" count="0"/>
 </file>
 <file name="/elsewhere/Other.php"><line num="1" type="stmt" count="1"/></file>
</project></coverage>"#;

    #[test]
    fn a_clover_report_gives_per_file_line_hits_inside_or_outside_a_package() {
        let coverage = parse_clover(CLOVER).unwrap();
        assert_eq!(coverage.files.len(), 3);
        let greeter = coverage.file(Path::new("/app/src/Greeter.php")).unwrap();
        assert_eq!(greeter.lines.get(&8), Some(&3));
        assert_eq!(greeter.lines.get(&9), Some(&0));
        assert_eq!((greeter.covered(), greeter.total()), (2, 3));
    }

    #[test]
    fn rows_roll_lines_up_into_every_directory_under_the_root() {
        let rows = parse_clover(CLOVER).unwrap().rows(Path::new("/app"));
        let summary: Vec<(String, bool, usize, usize)> = rows
            .iter()
            .map(|r| (r.path.display().to_string(), r.is_file, r.covered, r.total))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("".to_string(), false, 2, 5),
                ("src".to_string(), false, 2, 5),
                ("src/Greeter.php".to_string(), true, 2, 3),
                ("src/Util".to_string(), false, 0, 2),
                ("src/Util/Math.php".to_string(), true, 0, 2),
            ]
        );
        assert!((rows[2].percent() - 66.666).abs() < 0.01);
    }

    #[test]
    fn a_file_without_executable_lines_has_no_percentage_to_show() {
        let xml = r#"<coverage><project><file name="/app/src/Contract.php"></file>
            <file name="/app/src/Run.php"><line num="3" type="stmt" count="1"/></file></project></coverage>"#;
        let rows = parse_clover(xml).unwrap().rows(Path::new("/app"));
        let has_lines: Vec<(String, bool)> = rows
            .iter()
            .map(|r| (r.path.display().to_string(), r.has_lines()))
            .collect();
        assert_eq!(
            has_lines,
            vec![
                ("".to_string(), true),
                ("src".to_string(), true),
                ("src/Contract.php".to_string(), false),
                ("src/Run.php".to_string(), true),
            ]
        );
    }

    #[test]
    fn a_wsl_report_maps_to_local_paths() {
        let host = process_exec::host::ExecHost::Wsl(process_exec::host::WslHost {
            distro: "Ubuntu".into(),
            unc_prefix: "//wsl.localhost/Ubuntu".into(),
        });
        let coverage = parse_clover(CLOVER).unwrap().map_paths_from(&host);
        assert!(coverage.files[0]
            .path
            .to_string_lossy()
            .contains("wsl.localhost"));
    }

    #[test]
    fn malformed_xml_is_a_typed_error() {
        assert!(parse_clover("<coverage><file name=\"a\"></coverage>").is_err());
    }

    #[test]
    fn args_name_the_project_relative_report() {
        let args = args(&["--coverage-clover".into(), "$COVERAGE_FILE$".into()]);
        assert_eq!(args, ["--coverage-clover", REPORT_PATH]);
        assert_eq!(env(), [("XDEBUG_MODE".to_string(), "coverage".to_string())]);
    }
}
