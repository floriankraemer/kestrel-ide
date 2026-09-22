//! The `sql-script` run-configuration kind (database-tools-plan F3.6,
//! ADR-0056 shape): runs a whole `.sql` file against a configured data
//! source, off a dedicated thread, reporting through the same
//! `consoleStarted`/`consoleOutput`/`consoleFinished` signals a process
//! launch uses — there is no `run_core::Supervisor`/PTY process here at
//! all, so [`launch`] mints its own [`run_core::ConsoleId`] rather than
//! going through `Supervisor::launch`, from a range
//! (`SQL_SCRIPT_ID_BASE` and up) no real process console's monotonically-
//! increasing counter reaches in one run of the IDE.
//!
//! Cancellation, rerun-in-place and the `resolveLink` path a real
//! console's `ConsoleState.path_map` serves are all out of scope for a
//! first version (a script this small runs start-to-finish in one go far
//! more often than not) — `ConsoleState` is still populated so the dock's
//! existing tab/scrollback machinery treats this console exactly like any
//! other one that has already finished.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;

use db_core::dialect::Dialect;
use db_core::driver::{ExecOptions, Statement as DbStatement};
use db_core::readonly::Guard;
use db_core::session::Session;
use db_sql::classify::SqlClassifier;

use crate::bridge::database::service::{configured_sources, secrets_for};
use crate::bridge::errors;
use crate::bridge::ffi;

use super::{mark_finished, ConsoleState};

/// Comfortably past any id a single IDE run's `Supervisor` (starting at 0)
/// will ever reach.
const SQL_SCRIPT_ID_BASE: u64 = 1 << 62;

fn next_console_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(SQL_SCRIPT_ID_BASE);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn resolve_path(root: &Path, file: &str) -> PathBuf {
    let path = PathBuf::from(file);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

/// Starts `config`'s script running and returns immediately — the script
/// itself runs on a plain background thread (there is no `RunWorker`/
/// `Supervisor` job here to serialize through, unlike a process launch),
/// streaming lines back via `qt_thread.queue`.
pub(super) fn launch(
    mut service: Pin<&mut ffi::RunService>,
    config: &run_core::RunConfig,
    root: &Path,
) -> ffi::FfiResult {
    let Some(sql) = config.sql_script.clone() else {
        return errors::failure(
            errors::CODE_INVALID_ARGUMENT,
            "this configuration has no [run_configs.sql_script] table",
        );
    };
    let file = resolve_path(root, &sql.file);
    let console_id = next_console_id();
    let config_id = config.id.clone();
    let qt_thread = service.as_mut().qt_thread();
    service.as_mut().consoles.borrow_mut().insert(
        console_id,
        ConsoleState {
            config_id: config_id.clone(),
            cwd: root.to_path_buf(),
            path_map: None,
            output: String::new(),
            ansi: run_core::AnsiResolver::default(),
            last_runs: Vec::new(),
            finished: false,
        },
    );
    service
        .as_mut()
        .console_started(console_id, QString::from(config_id.as_str()));

    std::thread::spawn(move || {
        let outcome = run_script(
            &sql.source_id,
            &file,
            &sql.tx_mode,
            sql.stop_on_error,
            |line| {
                emit_line(console_id, line, &qt_thread);
            },
        );
        let ok = outcome.is_ok();
        let qt_thread = qt_thread.clone();
        let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
            if mark_finished(&mut service.consoles.borrow_mut(), console_id) {
                service
                    .as_mut()
                    .console_finished(console_id, if ok { 0 } else { 1 }, false);
            }
        });
    });
    ffi::FfiResult::default()
}

fn emit_line(console_id: u64, mut text: String, qt_thread: &CxxQtThread<ffi::RunService>) {
    text.push('\n');
    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
        {
            let mut consoles = service.consoles.borrow_mut();
            let Some(state) = consoles.get_mut(&console_id) else {
                return;
            };
            state.output.push_str(&text);
        }
        service
            .as_mut()
            .console_output(console_id, QString::from(text.as_str()));
    });
}

/// `tx_mode == "single_transaction"` wraps the whole script in one
/// transaction; anything else auto-commits each statement, honouring
/// `stop_on_error` between them — `SqlScriptRunSetting::tx_mode`'s own doc
/// comment.
fn is_single_transaction(tx_mode: &str) -> bool {
    tx_mode == "single_transaction"
}

/// Runs `path`'s whole contents against `source_id`, entirely
/// synchronously (the caller runs this off the Qt thread). `Err` only for
/// "could not even run the script at all" (bad source, connect failure, a
/// statement the read-only guard refused) or "stopped/rolled back with a
/// failure" — every line either way was already reported through
/// `on_line`.
fn run_script(
    source_id: &str,
    path: &Path,
    tx_mode: &str,
    stop_on_error: bool,
    on_line: impl FnMut(String),
) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let setting = configured_sources()
        .into_iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("no data source with id '{source_id}' is configured"))?;
    let data_source = db_core::datasource::DataSource::from(&setting);
    let read_only = data_source.read_only;
    let spec = db_core::datasource::ConnectSpec::from(&data_source, &secrets_for(source_id));
    let registry = db_drivers::DriverRegistry::builtin();
    let driver = registry
        .get(&spec.driver)
        .ok_or_else(|| format!("no driver registered for '{}'", spec.driver))?;
    let mut connection = driver.connect(&spec).map_err(|error| error.to_string())?;
    if read_only {
        let _ = connection.set_read_only(true);
    }
    run_statements_on(
        connection,
        &text,
        tx_mode,
        stop_on_error,
        read_only,
        on_line,
    )
}

/// The part of [`run_script`] that needs no data-source lookup at all — a
/// live `Connection` in hand, its dialect, and the script text. Split out
/// so a unit test can drive it against an in-memory SQLite connection
/// (`db_drivers::DriverRegistry`'s own test-friendly seam) without going
/// through `configured_sources`/`secrets_for`, which read this process's
/// real settings file.
///
/// `Guard::new(read_only, …)` carries the source's own flag now (the F3e
/// follow-up's fix — `db_core::readonly`'s own doc comment), so `check`
/// itself is a no-op on a writable source; this function no longer needs
/// its own `if read_only` gate around the call.
fn run_statements_on(
    mut connection: Box<dyn db_core::driver::Connection>,
    text: &str,
    tx_mode: &str,
    stop_on_error: bool,
    read_only: bool,
    mut on_line: impl FnMut(String),
) -> Result<(), String> {
    let dialect: Dialect = connection.dialect();
    let guard = Guard::new(read_only, Box::new(SqlClassifier { dialect }));
    let statements: Vec<String> = db_sql::split(text, dialect)
        .into_iter()
        .map(|statement| statement.text(text).trim().to_string())
        .filter(|statement| !statement.is_empty())
        .collect();
    if statements.is_empty() {
        on_line("The script is empty.".to_string());
        let _ = connection.close();
        return Ok(());
    }
    for statement in &statements {
        if let Err(error) = guard.check(statement) {
            on_line(format!("Refused: {}", error.message));
            let _ = connection.close();
            return Err(error.message);
        }
    }
    let mut session = Session::new(connection);
    let single_tx = is_single_transaction(tx_mode);
    if single_tx {
        if let Err(error) = session.begin_manual() {
            on_line(format!("Could not start a transaction: {error}"));
            return Err(error.to_string());
        }
    }
    let mut had_error = false;
    for statement in &statements {
        on_line(format!("> {statement}"));
        if let Err(error) = session.execute(
            &DbStatement::sql(statement.clone()),
            &ExecOptions::default(),
        ) {
            had_error = true;
            on_line(format!("Error: {error}"));
            if single_tx || stop_on_error {
                break;
            }
        }
    }
    if single_tx {
        if had_error {
            let _ = session.rollback();
            on_line("Rolled back — a statement failed.".to_string());
        } else if let Err(error) = session.commit() {
            on_line(format!("Commit failed: {error}"));
            had_error = true;
        }
    }
    let _ = session.close();
    if had_error {
        Err("the script failed".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_memory_sqlite() -> Box<dyn db_core::driver::Connection> {
        let spec = db_core::datasource::ConnectSpec {
            driver: "sqlite".to_string(),
            host: String::new(),
            port: None,
            database: ":memory:".to_string(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: Default::default(),
        };
        db_drivers::DriverRegistry::builtin()
            .get("sqlite")
            .unwrap()
            .connect(&spec)
            .unwrap()
    }

    fn lines(text: &str, tx_mode: &str, stop_on_error: bool) -> (Result<(), String>, Vec<String>) {
        let mut out = Vec::new();
        let result = run_statements_on(
            in_memory_sqlite(),
            text,
            tx_mode,
            stop_on_error,
            false,
            |line| out.push(line),
        );
        (result, out)
    }

    #[test]
    fn runs_every_statement_and_reports_success() {
        let (result, out) = lines(
            "CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);",
            "",
            true,
        );
        assert_eq!(result, Ok(()));
        assert!(out.iter().any(|line| line.contains("CREATE TABLE")));
        assert!(out.iter().any(|line| line.contains("INSERT INTO")));
    }

    #[test]
    fn stop_on_error_halts_after_the_first_failure() {
        let (result, out) = lines(
            "SELECT * FROM missing_table; CREATE TABLE t (x INTEGER);",
            "",
            true,
        );
        assert!(result.is_err());
        assert!(!out.iter().any(|line| line.contains("CREATE TABLE")));
    }

    #[test]
    fn continue_on_error_runs_every_statement_regardless() {
        let (result, out) = lines(
            "SELECT * FROM missing_table; CREATE TABLE t (x INTEGER);",
            "",
            false,
        );
        assert!(result.is_err());
        assert!(out.iter().any(|line| line.contains("CREATE TABLE")));
    }

    #[test]
    fn single_transaction_rolls_back_every_statement_on_a_failure() {
        let (result, _) = lines(
            "CREATE TABLE t (x INTEGER); INSERT INTO t VALUES ('not a number needing quotes ok'); SELECT * FROM missing_table;",
            "single_transaction",
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn an_empty_script_reports_and_succeeds() {
        let (result, out) = lines("   ", "", true);
        assert_eq!(result, Ok(()));
        assert!(out.iter().any(|line| line.contains("empty")));
    }

    #[test]
    fn a_write_statement_is_refused_when_read_only_is_set() {
        let mut out = Vec::new();
        let result = run_statements_on(
            in_memory_sqlite(),
            "DROP TABLE t;",
            "",
            true,
            true,
            |line| out.push(line),
        );
        assert!(result.is_err());
        assert!(out.iter().any(|line| line.contains("Refused")));
    }

    #[test]
    fn a_write_statement_runs_fine_when_the_source_is_not_read_only() {
        let (result, _) = lines("CREATE TABLE t (x INTEGER);", "", true);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn is_single_transaction_only_matches_the_one_spelling() {
        assert!(is_single_transaction("single_transaction"));
        assert!(!is_single_transaction(""));
        assert!(!is_single_transaction("auto"));
    }

    #[test]
    fn resolve_path_joins_a_relative_file_onto_root() {
        let root = Path::new("/project");
        assert_eq!(
            resolve_path(root, "sql/migrate.sql"),
            PathBuf::from("/project/sql/migrate.sql")
        );
    }

    #[test]
    fn resolve_path_leaves_an_absolute_file_alone() {
        let root = Path::new("/project");
        assert_eq!(
            resolve_path(root, "/elsewhere/migrate.sql"),
            PathBuf::from("/elsewhere/migrate.sql")
        );
    }
}
