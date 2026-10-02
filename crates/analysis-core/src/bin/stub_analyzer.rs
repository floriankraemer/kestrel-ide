//! A minimal stub analyzer, used as a test fixture so `analysis-core`'s
//! wiring — detection, the scheduler, `checkstyle-xml` parsing, and the
//! shared diagnostics store — can be exercised end to end with no real PHP
//! or Composer installed (the PHP tooling plan's E1, on the
//! `lsp_core::bin::stub_server` precedent: a `[[bin]]` of this crate rather
//! than an example, so a test can locate it with
//! `env!("CARGO_BIN_EXE_stub_analyzer")` — a path Cargo guarantees, instead
//! of guessing at `target/debug` layout).
//!
//! Plays `vendor/bin/phpstan` in a fixture Composer project. `phpstan analyse
//! --error-format=checkstyle --no-progress <project-root>` is the real
//! invocation (`plugin-host/builtin/php-tools/plugin.toml`); this stub reads
//! only the last argument (the project root, exactly what
//! `AnalysisServiceRust::run_next` appends after the manifest's own `args`)
//! and prints the same two canned `checkstyle-xml` findings against
//! `src/Greeter.php` under it — the same file, lines, column and messages as
//! `analysis-core/tests/fixtures/checkstyle_one_file.xml`, so the two stay
//! provably in sync rather than drifting apart as two hand-maintained copies
//! of the same fixture data.
//!
//! Per-file mode (the PHP parity plan's E2): when the last argument is an
//! existing file (`phpstan analyse <file>`) or `-` with a
//! `--stdin-path=<path>` argument (`phpcs --stdin-path=<path> -`, the
//! buffer on stdin), it reports one finding per line containing
//! `STUB_FINDING`, so a test chooses where findings land by typing that
//! marker. The finding's `source` is `PHPStan.stubFinding` when the binary
//! is named `phpstan` (how `vendor/bin/phpstan` is seeded) and
//! `Stub.Sniff.Finding` otherwise. The project-root mode above is kept.
//!
//! Exits 1, matching a real PHPStan run that found something to report
//! (PHPStan's own exit code for "no errors" is 0; the E2E fixture wants the
//! non-trivial path, matching `RunOutput`'s "non-zero is the normal case"
//! contract documented on `analysis::publish_result`).

use std::env;
use std::io::{Read, Write};
use std::path::Path;

/// The stub's "formatting": tabs become four spaces and the text ends with
/// exactly one newline — enough to tell formatted from unformatted text.
fn format_text(text: &str) -> String {
    format!("{}\n", text.replace('\t', "    ").trim_end_matches('\n'))
}

/// Formatter mode (ADR-0070), selected by an argument so the same binary
/// plays php-cs-fixer/Pint (`--format-file <path>`, rewrites the file) and
/// phpcbf (`--format-stdin`, filters stdin to stdout). `--exit=N` sets the
/// exit code; `--no-output` makes a stdin run print nothing.
fn run_formatter(args: &[String]) -> Option<i32> {
    let stdin_mode = args.iter().any(|a| a == "--format-stdin");
    let file = args
        .iter()
        .position(|a| a == "--format-file")
        .and_then(|i| args.get(i + 1));
    if !stdin_mode && file.is_none() {
        return None;
    }
    if stdin_mode {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).expect("stdin");
        if !args.iter().any(|a| a == "--no-output") {
            std::io::stdout()
                .write_all(format_text(&text).as_bytes())
                .expect("stdout");
        }
    } else if let Some(file) = file {
        let text = std::fs::read_to_string(file).expect("file");
        std::fs::write(file, format_text(&text)).expect("write");
    }
    let code = args
        .iter()
        .find_map(|a| a.strip_prefix("--exit=")?.parse().ok());
    Some(code.unwrap_or(0))
}

/// Per-file mode: `last` is the file to read, or `-` for stdin (whose
/// reported path comes from `--stdin-path=`). Returns the exit code.
fn run_file_mode(args: &[String], last: &str) -> i32 {
    let (path, text) = if last == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).expect("stdin");
        let path = args
            .iter()
            .find_map(|a| a.strip_prefix("--stdin-path="))
            .unwrap_or("stdin.php")
            .to_string();
        (path, text)
    } else {
        (
            last.to_string(),
            std::fs::read_to_string(last).expect("file"),
        )
    };
    let program = env::args().next().unwrap_or_default();
    let source = if Path::new(&program)
        .file_stem()
        .is_some_and(|n| n == "phpstan")
    {
        "PHPStan.stubFinding"
    } else {
        "Stub.Sniff.Finding"
    };
    let findings: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("STUB_FINDING"))
        .map(|(i, _)| i + 1)
        .collect();
    println!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<checkstyle version=\"3.7.1\">\n <file name=\"{path}\">"
    );
    for line in &findings {
        println!(
            "  <error line=\"{line}\" column=\"1\" severity=\"error\" \
             message=\"Stub finding\" source=\"{source}\"/>"
        );
    }
    println!(" </file>\n</checkstyle>");
    i32::from(!findings.is_empty())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if let Some(code) = run_formatter(&args) {
        std::process::exit(code);
    }
    let last = args.last().cloned().unwrap_or_default();
    if last == "-" || Path::new(&last).is_file() {
        std::process::exit(run_file_mode(&args, &last));
    }
    let project_root = last;
    let file = Path::new(&project_root).join("src/Greeter.php");
    println!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <checkstyle version=\"3.7.1\">\n\
         \x20<file name=\"{}\">\n\
         \x20\x20<error line=\"10\" column=\"5\" severity=\"error\" \
         message=\"Undefined variable: $name\" source=\"PHPStan.undefinedVariable\"/>\n\
         \x20\x20<error line=\"20\" severity=\"warning\" \
         message=\"Unused variable $x\"/>\n\
         \x20</file>\n\
         </checkstyle>",
        file.display()
    );
    std::process::exit(1);
}
