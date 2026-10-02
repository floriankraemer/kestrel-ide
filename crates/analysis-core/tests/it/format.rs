//! `analysis_core::format` against the stub analyzer's formatter mode: no
//! PHP or Composer needed.

use std::path::Path;
use std::time::Duration;

use analysis_core::{format, BufferStrategy, FormatError, FormatterDef};
use process_exec::host::ExecHost;

const STUB: &str = env!("CARGO_BIN_EXE_stub_analyzer");

fn def(buffer: BufferStrategy, args: &[&str], success: &[i32]) -> FormatterDef {
    FormatterDef {
        id: "stub".into(),
        name: "Stub".into(),
        languages: vec!["php".into()],
        program_candidates: vec![STUB.into()],
        args: args.iter().map(|a| a.to_string()).collect(),
        buffer,
        success_exit_codes: success.to_vec(),
        requires_interpreter: None,
    }
}

fn run(def: &FormatterDef, root: &Path, text: &str) -> Result<String, FormatError> {
    let path = root.join("A.php");
    std::fs::write(&path, text).unwrap();
    format(
        def,
        text,
        &path,
        root,
        &ExecHost::Local,
        "php",
        Duration::from_secs(20),
    )
}

#[test]
fn a_temp_copy_formatter_returns_the_rewritten_copy_and_leaves_the_original() {
    let root = tempfile::tempdir().unwrap();
    let d = def(BufferStrategy::TempCopy, &["--format-file", "{file}"], &[0]);
    let out = run(&d, root.path(), "<?php\n\techo 1;").unwrap();
    assert_eq!(out, "<?php\n    echo 1;\n");
    assert_eq!(
        std::fs::read_to_string(root.path().join("A.php")).unwrap(),
        "<?php\n\techo 1;"
    );
    let leftovers = std::fs::read_dir(root.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains("ide-analysis-tmp"))
        .count();
    assert_eq!(leftovers, 0, "the temp copy is deleted");
}

#[test]
fn a_stdin_formatter_returns_stdout_and_accepts_its_listed_exit_code() {
    let root = tempfile::tempdir().unwrap();
    let d = def(
        BufferStrategy::Stdin,
        &["--format-stdin", "--exit=1"],
        &[0, 1],
    );
    assert_eq!(run(&d, root.path(), "a\tb").unwrap(), "a    b\n");
}

#[test]
fn an_exit_code_outside_the_success_list_is_a_failure() {
    let root = tempfile::tempdir().unwrap();
    let d = def(
        BufferStrategy::Stdin,
        &["--format-stdin", "--exit=2"],
        &[0, 1],
    );
    assert!(matches!(
        run(&d, root.path(), "x"),
        Err(FormatError::Failed { code: Some(2), .. })
    ));
}

#[test]
fn an_empty_stdout_for_a_non_empty_buffer_is_rejected_not_applied() {
    let root = tempfile::tempdir().unwrap();
    let d = def(
        BufferStrategy::Stdin,
        &["--format-stdin", "--no-output"],
        &[0],
    );
    assert_eq!(
        run(&d, root.path(), "x"),
        Err(FormatError::BadOutput("printed nothing"))
    );
}

#[test]
fn a_missing_program_is_not_installed() {
    let root = tempfile::tempdir().unwrap();
    let mut d = def(BufferStrategy::Stdin, &[], &[0]);
    d.program_candidates = vec!["vendor/bin/nope".into()];
    assert_eq!(run(&d, root.path(), "x"), Err(FormatError::NotInstalled));
}
