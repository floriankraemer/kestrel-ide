//! The argv `AnalyzerDef::file_run_args` builds, driven against the stub
//! analyzer's per-file modes: a saved-only PHPStan-shaped run and a stdin
//! PHPCS-shaped run, so the E2E flow's wiring is proven below the UI.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use analysis_core::{parse_checkstyle_xml, AnalyzerDef, CheckstyleFinding};
use plugin_api::AnalyzerContribution;

const STUB: &str = env!("CARGO_BIN_EXE_stub_analyzer");

fn contribution(id: &str, args: &[&str], file_args: &[&str], buffer: &str) -> AnalyzerContribution {
    AnalyzerContribution {
        id: id.into(),
        name: id.into(),
        program_candidates: vec![id.into()],
        args: args.iter().map(|a| a.to_string()).collect(),
        output_format: "checkstyle-xml".into(),
        severity_map: Default::default(),
        languages: vec!["php".into()],
        file_args: file_args.iter().map(|a| a.to_string()).collect(),
        buffer: Some(buffer.into()),
        composer_package: None,
        requires_interpreter: None,
        suppress_comment: None,
        code_in_message: false,
        fixer: None,
        config_file_candidates: vec![],
        ruleset_default: None,
        project_paths_config: vec![],
        required_config: vec![],
        config_init: None,
    }
}

/// The stub copied to `<dir>/<name>`, the way a Composer project seeds
/// `vendor/bin/<name>`.
fn seeded(dir: &Path, name: &str) -> std::path::PathBuf {
    let program = dir.join(name);
    std::fs::copy(STUB, &program).unwrap();
    program
}

fn run(program: &Path, argv: &[String], stdin: Option<&str>) -> Vec<CheckstyleFinding> {
    let mut child = Command::new(program)
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // A saved-only run never reads stdin, so the pipe may already be closed.
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.unwrap_or_default().as_bytes());
    let out = child.wait_with_output().unwrap();
    parse_checkstyle_xml(&String::from_utf8(out.stdout).unwrap()).unwrap()
}

#[test]
fn a_saved_only_run_reads_the_file_and_reports_each_marked_line_as_phpstan() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("A.php");
    std::fs::write(&file, "<?php\n// STUB_FINDING\n$ok;\n// STUB_FINDING\n").unwrap();
    let def = AnalyzerDef::from_contribution(&contribution(
        "phpstan",
        &["analyse", "--error-format=checkstyle", "--no-progress"],
        &[],
        "saved-only",
    ));
    let findings = run(
        &seeded(dir.path(), "phpstan"),
        &def.file_run_args(&file),
        None,
    );
    let lines: Vec<u32> = findings.iter().map(|f| f.line).collect();
    assert_eq!(lines, [2, 4]);
    assert_eq!(findings[0].source.as_deref(), Some("PHPStan.stubFinding"));
    assert_eq!(findings[0].file, file.to_string_lossy());
}

#[test]
fn a_stdin_run_lints_the_buffer_under_the_given_path_as_a_sniff() {
    let dir = tempfile::tempdir().unwrap();
    let def = AnalyzerDef::from_contribution(&contribution(
        "phpcs",
        &["--report=checkstyle"],
        &["--stdin-path={file}", "-"],
        "stdin",
    ));
    let argv = def.file_run_args(Path::new("/p/src/B.php"));
    // The file on disk does not exist: only the buffer on stdin counts.
    let findings = run(
        &seeded(dir.path(), "phpcs"),
        &argv,
        Some("<?php\n$a;\n// STUB_FINDING\n"),
    );
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].line, 3);
    assert_eq!(findings[0].file, "/p/src/B.php");
    assert_eq!(findings[0].source.as_deref(), Some("Stub.Sniff.Finding"));
}
