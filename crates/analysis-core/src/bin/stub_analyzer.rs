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
//! and prints one canned `checkstyle-xml` finding against `src/Greeter.php`
//! under it — the same file, line, column and message as
//! `analysis-core/tests/fixtures/checkstyle_one_file.xml`, so the two stay
//! provably in sync rather than drifting apart as two hand-maintained copies
//! of the same fixture data.
//!
//! Exits 1, matching a real PHPStan run that found something to report
//! (PHPStan's own exit code for "no errors" is 0; the E2E fixture wants the
//! non-trivial path, matching `RunOutput`'s "non-zero is the normal case"
//! contract documented on `analysis::publish_result`).

use std::env;
use std::path::Path;

fn main() {
    let project_root = env::args().next_back().unwrap_or_default();
    let file = Path::new(&project_root).join("src/Greeter.php");
    println!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <checkstyle version=\"3.7.1\">\n\
         \x20<file name=\"{}\">\n\
         \x20\x20<error line=\"10\" column=\"5\" severity=\"error\" \
         message=\"Undefined variable: $name\" source=\"PHPStan.undefinedVariable\"/>\n\
         \x20</file>\n\
         </checkstyle>",
        file.display()
    );
    std::process::exit(1);
}
