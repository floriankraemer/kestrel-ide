use super::*;
use db_core::driver::{CancelHandle, Connection, Execution};
use db_core::schema::{Children, Node, ObjectKind};
use std::sync::Mutex;

/// A canned two-batch `RowStream`: `[1, 2]` then `[3]`, then exhausted.
struct FakeRowStream {
    remaining: Vec<Vec<Value>>,
}

impl RowStream for FakeRowStream {
    fn next_batch(&mut self) -> Result<Option<db_core::value::RowBatch>, DbError> {
        if self.remaining.is_empty() {
            return Ok(None);
        }
        let row = self.remaining.remove(0);
        Ok(Some(db_core::value::RowBatch {
            columns: vec![ColumnMeta {
                name: "n".to_string(),
                type_name: "int".to_string(),
                nullable: false,
                origin: None,
            }],
            rows: vec![row],
        }))
    }
}

struct FakeCancelHandle(Arc<AtomicU64>);
impl CancelHandle for FakeCancelHandle {
    fn cancel(&self) -> Result<(), DbError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A `Connection` double whose `introspect` counts every call it
/// receives — proves the worker's own cache actually avoids a repeat
/// query, which a mock that just returns canned data could not.
/// `execute` also recognises `"SELECT ROWS"` (a two-batch
/// [`FakeRowStream`]) and offers a [`FakeCancelHandle`] that counts how
/// many times it was invoked.
struct CountingConnection {
    introspect_calls: Arc<AtomicU64>,
    cancel_calls: Arc<AtomicU64>,
}

impl Connection for CountingConnection {
    fn dialect(&self) -> db_core::dialect::Dialect {
        db_core::dialect::Dialect::Sqlite
    }
    fn server_info(&self) -> String {
        "counting".to_string()
    }
    fn introspect(
        &mut self,
        _scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        self.introspect_calls.fetch_add(1, Ordering::SeqCst);
        Ok(SchemaSnapshot::new(
            level,
            vec![Node::leaf("t", ObjectKind::Table)],
        ))
    }
    fn execute(
        &mut self,
        statement: &Statement,
        _options: &ExecOptions,
    ) -> Result<Execution, DbError> {
        if statement.text.contains("FAIL") {
            return Err(DbError::new(
                db_core::error::DbErrorCode::InvalidStatement,
                "boom",
            ));
        }
        if statement.text.contains("ROWS") {
            return Ok(Execution::Rows(Box::new(FakeRowStream {
                remaining: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            })));
        }
        Ok(Execution::Affected(0))
    }
    fn begin(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    fn set_read_only(&mut self, _read_only: bool) -> Result<(), DbError> {
        Ok(())
    }
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        Some(Box::new(FakeCancelHandle(Arc::clone(&self.cancel_calls))))
    }
    fn ddl_of(&mut self, object: &ObjectRef) -> Result<String, DbError> {
        Ok(format!("CREATE TABLE {}", object.name))
    }
    fn apply(&mut self, _statements: &[Statement]) -> Result<u64, DbError> {
        Ok(0)
    }
    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

fn worker_with_events() -> (SessionWorker, Arc<Mutex<Vec<u64>>>, Arc<AtomicU64>) {
    let calls = Arc::new(AtomicU64::new(0));
    let session = Session::new(Box::new(CountingConnection {
        introspect_calls: Arc::clone(&calls),
        cancel_calls: Arc::new(AtomicU64::new(0)),
    }));
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_clone = Arc::clone(&received);
    let worker = SessionWorker::spawn(session, None, move |event| {
        if let SessionEvent::Introspected { generation, .. } = event {
            received_clone.lock().unwrap().push(generation);
        }
    });
    (worker, received, calls)
}

/// Blocks until `predicate` sees at least `n` items — the worker
/// thread runs asynchronously, so a test polls its shared sink rather
/// than asserting immediately after `send`.
fn wait_for<T>(shared: &Mutex<Vec<T>>, n: usize) {
    for _ in 0..200 {
        if shared.lock().unwrap().len() >= n {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("timed out waiting for {n} event(s)");
}

#[test]
fn a_second_introspect_of_the_same_scope_is_served_from_cache() {
    let (worker, received, calls) = worker_with_events();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    wait_for(&received, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_forced_introspect_bypasses_the_cache() {
    let (worker, received, calls) = worker_with_events();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: true,
        })
        .unwrap();
    wait_for(&received, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn dropping_a_cached_scope_makes_the_next_request_a_real_query_again() {
    let (worker, received, calls) = worker_with_events();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    wait_for(&received, 1);
    worker
        .send(SessionCommand::DropCached {
            scope: IntrospectScope::default(),
        })
        .unwrap();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    wait_for(&received, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn a_names_level_cache_entry_does_not_satisfy_a_columns_level_request() {
    let (worker, received, calls) = worker_with_events();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Columns,
            force: false,
        })
        .unwrap();
    wait_for(&received, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn a_columns_level_cache_entry_satisfies_a_names_level_request() {
    let (worker, received, calls) = worker_with_events();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Columns,
            force: false,
        })
        .unwrap();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    wait_for(&received, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn ddl_of_and_run_statement_round_trip_through_the_worker() {
    let calls = Arc::new(AtomicU64::new(0));
    let session = Session::new(Box::new(CountingConnection {
        introspect_calls: calls,
        cancel_calls: Arc::new(AtomicU64::new(0)),
    }));
    let ddls = Arc::new(Mutex::new(Vec::new()));
    let ran = Arc::new(Mutex::new(Vec::new()));
    let ddls_clone = Arc::clone(&ddls);
    let ran_clone = Arc::clone(&ran);
    let worker = SessionWorker::spawn(session, None, move |event| match event {
        SessionEvent::Ddl { result, .. } => ddls_clone.lock().unwrap().push(result),
        SessionEvent::Ran { result, .. } => ran_clone.lock().unwrap().push(result),
        SessionEvent::Introspected { .. }
        | SessionEvent::Batch { .. }
        | SessionEvent::TxChanged { .. }
        | SessionEvent::Applied { .. } => {}
    });
    worker
        .send(SessionCommand::DdlOf {
            object: ObjectRef::new("t"),
        })
        .unwrap();
    worker
        .send(SessionCommand::RunStatement {
            statement: Statement::sql("DROP TABLE t"),
        })
        .unwrap();
    worker
        .send(SessionCommand::RunStatement {
            statement: Statement::sql("FAIL"),
        })
        .unwrap();
    wait_for(&ddls, 1);
    wait_for(&ran, 2);
    assert_eq!(ddls.lock().unwrap()[0].as_deref(), Ok("CREATE TABLE t"));
    assert!(ran.lock().unwrap()[0].is_ok());
    assert!(ran.lock().unwrap()[1].is_err());
}

#[test]
fn invalidate_bumps_the_generation_the_worker_reports() {
    let (worker, _received, _calls) = worker_with_events();
    let before = worker.generation();
    let after = worker.invalidate();
    assert_eq!(after, before + 1);
    assert_eq!(worker.generation(), after);
}

#[test]
fn events_carry_the_generation_at_dispatch_time() {
    let (worker, received, _calls) = worker_with_events();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    wait_for(&received, 1);
    assert_eq!(received.lock().unwrap()[0], 0);
}

#[test]
fn dropping_the_worker_stops_its_thread_without_a_panic() {
    let (worker, _received, _calls) = worker_with_events();
    drop(worker);
}

#[test]
fn a_scope_that_never_matches_is_a_no_op_cache_drop() {
    let (worker, received, calls) = worker_with_events();
    worker
        .send(SessionCommand::DropCached {
            scope: IntrospectScope::for_schema("nothing-cached-yet"),
        })
        .unwrap();
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    wait_for(&received, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn schema_snapshot_children_survive_the_cache_round_trip() {
    let calls = Arc::new(AtomicU64::new(0));
    let session = Session::new(Box::new(CountingConnection {
        introspect_calls: calls,
        cancel_calls: Arc::new(AtomicU64::new(0)),
    }));
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_clone = Arc::clone(&received);
    let worker = SessionWorker::spawn(session, None, move |event| {
        if let SessionEvent::Introspected { result, .. } = event {
            received_clone.lock().unwrap().push(result);
        }
    });
    worker
        .send(SessionCommand::Introspect {
            scope: IntrospectScope::default(),
            level: IntrospectLevel::Names,
            force: false,
        })
        .unwrap();
    wait_for(&received, 1);
    let snapshot = received.lock().unwrap().remove(0).unwrap();
    assert!(matches!(snapshot.roots[0].children, Children::NotLoaded));
}

fn batches_worker() -> (SessionWorker, Arc<Mutex<Vec<BatchOutcome>>>) {
    let session = Session::new(Box::new(CountingConnection {
        introspect_calls: Arc::new(AtomicU64::new(0)),
        cancel_calls: Arc::new(AtomicU64::new(0)),
    }));
    let batches = Arc::new(Mutex::new(Vec::new()));
    let batches_clone = Arc::clone(&batches);
    let worker = SessionWorker::spawn(session, None, move |event| {
        if let SessionEvent::Batch { outcome, .. } = event {
            batches_clone.lock().unwrap().push(outcome);
        }
    });
    (worker, batches)
}

#[test]
fn executing_a_rows_statement_parks_the_stream_and_reports_the_first_page() {
    let (worker, batches) = batches_worker();
    worker
        .send(SessionCommand::Execute {
            statement: Statement::sql("SELECT ROWS"),
            options: ExecOptions::default(),
        })
        .unwrap();
    wait_for(&batches, 1);
    let guard = batches.lock().unwrap();
    match &guard[0] {
        BatchOutcome::Rows { rows, done, .. } => {
            assert_eq!(rows, &vec![vec![Value::Int(1)]]);
            assert!(!done);
        }
        _ => panic!("expected a Rows batch"),
    }
}

#[test]
fn fetch_more_pages_the_parked_stream_until_it_is_exhausted() {
    let (worker, batches) = batches_worker();
    worker
        .send(SessionCommand::Execute {
            statement: Statement::sql("SELECT ROWS"),
            options: ExecOptions::default(),
        })
        .unwrap();
    worker.send(SessionCommand::FetchMore).unwrap();
    worker.send(SessionCommand::FetchMore).unwrap();
    wait_for(&batches, 3);
    let batches = batches.lock().unwrap();
    assert!(
        matches!(&batches[1], BatchOutcome::Rows { rows, done: false, .. } if rows == &vec![vec![Value::Int(2)]])
    );
    assert!(matches!(&batches[2], BatchOutcome::Rows { done: true, .. }));
}

#[test]
fn fetch_more_with_nothing_parked_reports_an_error() {
    let (worker, batches) = batches_worker();
    worker.send(SessionCommand::FetchMore).unwrap();
    wait_for(&batches, 1);
    assert!(matches!(
        &batches.lock().unwrap()[0],
        BatchOutcome::Error(_)
    ));
}

#[test]
fn a_non_rows_execute_reports_affected_directly_with_nothing_parked() {
    let (worker, batches) = batches_worker();
    worker
        .send(SessionCommand::Execute {
            statement: Statement::sql("UPDATE t SET x = 1"),
            options: ExecOptions::default(),
        })
        .unwrap();
    wait_for(&batches, 1);
    assert!(matches!(
        batches.lock().unwrap()[0],
        BatchOutcome::Affected(0)
    ));
    // Nothing was parked — a follow-up `FetchMore` is refused, not
    // silently served from a previous statement's leftover stream.
    worker.send(SessionCommand::FetchMore).unwrap();
    wait_for(&batches, 2);
    assert!(matches!(batches.lock().unwrap()[1], BatchOutcome::Error(_)));
}

#[test]
fn cancel_now_invokes_the_connections_cancel_handle_from_outside_the_command_queue() {
    let cancel_calls = Arc::new(AtomicU64::new(0));
    let session = Session::new(Box::new(CountingConnection {
        introspect_calls: Arc::new(AtomicU64::new(0)),
        cancel_calls: Arc::clone(&cancel_calls),
    }));
    let worker = SessionWorker::spawn(session, None, |_event| {});
    worker.cancel_now().unwrap();
    assert_eq!(cancel_calls.load(Ordering::SeqCst), 1);
}

/// A `Connection` double for [`SessionCommand::Apply`]'s own tests:
/// records every `begin`/`commit`/`rollback`/`apply` call in order, and
/// `apply` fails when any statement's text contains `"FAIL"`.
#[derive(Default)]
struct ApplyConnection {
    calls: Arc<Mutex<Vec<&'static str>>>,
    fail: bool,
}

impl Connection for ApplyConnection {
    fn dialect(&self) -> db_core::dialect::Dialect {
        db_core::dialect::Dialect::Sqlite
    }
    fn server_info(&self) -> String {
        "apply".to_string()
    }
    fn introspect(
        &mut self,
        _scope: &IntrospectScope,
        _level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        unimplemented!("not exercised by the Apply tests")
    }
    fn execute(
        &mut self,
        _statement: &Statement,
        _options: &ExecOptions,
    ) -> Result<Execution, DbError> {
        unimplemented!("not exercised by the Apply tests")
    }
    fn begin(&mut self) -> Result<(), DbError> {
        self.calls.lock().unwrap().push("begin");
        Ok(())
    }
    fn commit(&mut self) -> Result<(), DbError> {
        self.calls.lock().unwrap().push("commit");
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        self.calls.lock().unwrap().push("rollback");
        Ok(())
    }
    fn set_read_only(&mut self, _read_only: bool) -> Result<(), DbError> {
        Ok(())
    }
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        None
    }
    fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
        unimplemented!("not exercised by the Apply tests")
    }
    fn apply(&mut self, statements: &[Statement]) -> Result<u64, DbError> {
        self.calls.lock().unwrap().push("apply");
        if self.fail || statements.iter().any(|s| s.text.contains("FAIL")) {
            return Err(DbError::new(
                db_core::error::DbErrorCode::InvalidStatement,
                "boom",
            ));
        }
        Ok(statements.len() as u64)
    }
    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

#[test]
fn apply_in_auto_mode_wraps_a_begin_and_commit_around_it() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let session = Session::new(Box::new(ApplyConnection {
        calls: Arc::clone(&calls),
        fail: false,
    }));
    let results = Arc::new(Mutex::new(Vec::new()));
    let results_clone = Arc::clone(&results);
    let worker = SessionWorker::spawn(session, None, move |event| {
        if let SessionEvent::Applied { result, .. } = event {
            results_clone.lock().unwrap().push(result);
        }
    });
    worker
        .send(SessionCommand::Apply {
            statements: vec![Statement::sql("UPDATE t SET x = 1")],
        })
        .unwrap();
    wait_for(&results, 1);
    assert_eq!(*calls.lock().unwrap(), vec!["begin", "apply", "commit"]);
    assert_eq!(results.lock().unwrap()[0], Ok(1));
}

#[test]
fn apply_in_auto_mode_rolls_back_on_failure_and_reports_the_error() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let session = Session::new(Box::new(ApplyConnection {
        calls: Arc::clone(&calls),
        fail: true,
    }));
    let results = Arc::new(Mutex::new(Vec::new()));
    let results_clone = Arc::clone(&results);
    let worker = SessionWorker::spawn(session, None, move |event| {
        if let SessionEvent::Applied { result, .. } = event {
            results_clone.lock().unwrap().push(result);
        }
    });
    worker
        .send(SessionCommand::Apply {
            statements: vec![Statement::sql("UPDATE t SET x = 1")],
        })
        .unwrap();
    wait_for(&results, 1);
    assert_eq!(*calls.lock().unwrap(), vec!["begin", "apply", "rollback"]);
    assert!(results.lock().unwrap()[0].is_err());
}

#[test]
fn apply_in_manual_mode_neither_begins_nor_commits_its_own_transaction() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut session = Session::new(Box::new(ApplyConnection {
        calls: Arc::clone(&calls),
        fail: false,
    }));
    session.begin_manual().unwrap();
    calls.lock().unwrap().clear();
    let results = Arc::new(Mutex::new(Vec::new()));
    let results_clone = Arc::clone(&results);
    let worker = SessionWorker::spawn(session, None, move |event| {
        if let SessionEvent::Applied { result, .. } = event {
            results_clone.lock().unwrap().push(result);
        }
    });
    worker
        .send(SessionCommand::Apply {
            statements: vec![Statement::sql("UPDATE t SET x = 1")],
        })
        .unwrap();
    wait_for(&results, 1);
    // Only the apply itself — no begin/commit of its own, since a
    // manual transaction is already open.
    assert_eq!(*calls.lock().unwrap(), vec!["apply"]);
    assert_eq!(results.lock().unwrap()[0], Ok(1));
}

#[test]
fn begin_commit_rollback_report_through_tx_changed() {
    let session = Session::new(Box::new(CountingConnection {
        introspect_calls: Arc::new(AtomicU64::new(0)),
        cancel_calls: Arc::new(AtomicU64::new(0)),
    }));
    let results = Arc::new(Mutex::new(Vec::new()));
    let results_clone = Arc::clone(&results);
    let worker = SessionWorker::spawn(session, None, move |event| {
        if let SessionEvent::TxChanged { result, .. } = event {
            results_clone.lock().unwrap().push(result);
        }
    });
    worker.send(SessionCommand::BeginManual).unwrap();
    worker.send(SessionCommand::Commit).unwrap();
    worker.send(SessionCommand::Rollback).unwrap();
    wait_for(&results, 3);
    let results = results.lock().unwrap();
    assert!(results[0].is_ok());
    assert!(results[1].is_ok());
    // A `Rollback` right after a `Commit` re-opened auto mode: the fake
    // connection accepts it (it only counts calls), proving `run` wires
    // the command through rather than the outcome mattering here.
    assert!(results[2].is_ok());
}

/// A fake tunnel that records into a shared log on `Drop` — proves
/// [`SessionWorker::spawn`]'s drop order (F7c): the connection must
/// close before the tunnel it was routed through does, never after.
struct LoggingTunnel {
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl Tunnel for LoggingTunnel {
    fn local_port(&self) -> u16 {
        12345
    }
}

impl Drop for LoggingTunnel {
    fn drop(&mut self) {
        self.log.lock().unwrap().push("tunnel");
    }
}

/// A `Connection` double whose `close` records into the same log —
/// paired with [`LoggingTunnel`] below.
struct LoggingConnection {
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl Connection for LoggingConnection {
    fn dialect(&self) -> db_core::dialect::Dialect {
        db_core::dialect::Dialect::Sqlite
    }
    fn server_info(&self) -> String {
        "logging".to_string()
    }
    fn introspect(
        &mut self,
        _scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        Ok(SchemaSnapshot::new(level, Vec::new()))
    }
    fn execute(
        &mut self,
        _statement: &Statement,
        _options: &ExecOptions,
    ) -> Result<Execution, DbError> {
        Ok(Execution::Ok)
    }
    fn begin(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    fn set_read_only(&mut self, _read_only: bool) -> Result<(), DbError> {
        Ok(())
    }
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        None
    }
    fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
        unimplemented!("not exercised by this test")
    }
    fn apply(&mut self, _statements: &[Statement]) -> Result<u64, DbError> {
        unimplemented!("not exercised by this test")
    }
    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

impl Drop for LoggingConnection {
    fn drop(&mut self) {
        self.log.lock().unwrap().push("connection");
    }
}

#[test]
fn dropping_the_worker_closes_the_connection_before_the_tunnel() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let session = Session::new(Box::new(LoggingConnection { log: log.clone() }));
    let tunnel: Box<dyn Tunnel> = Box::new(LoggingTunnel { log: log.clone() });
    let worker = SessionWorker::spawn(session, Some(tunnel), |_event| {});
    drop(worker); // sends Shutdown and joins the thread (see the Drop impl)
    assert_eq!(*log.lock().unwrap(), vec!["connection", "tunnel"]);
}

/// A `Connection` double that logs `"close"` on `Connection::close`
/// and otherwise answers trivially — [`an_idle_session_closes_and_
/// reconnects_lazily_on_the_next_command`]'s own fake, built fresh by
/// its `reconnect` closure each time so a real driver's "the old
/// connection is gone, the new one is a fresh handle" shape is
/// exercised, not just one object mutated in place.
struct IdleLoggingConnection {
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl Connection for IdleLoggingConnection {
    fn dialect(&self) -> db_core::dialect::Dialect {
        db_core::dialect::Dialect::Sqlite
    }
    fn server_info(&self) -> String {
        "idle-test".to_string()
    }
    fn introspect(
        &mut self,
        _scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        Ok(SchemaSnapshot::new(level, vec![]))
    }
    fn execute(
        &mut self,
        _statement: &Statement,
        _options: &ExecOptions,
    ) -> Result<db_core::driver::Execution, DbError> {
        Ok(db_core::driver::Execution::Affected(0))
    }
    fn begin(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    fn commit(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        Ok(())
    }
    fn set_read_only(&mut self, _read_only: bool) -> Result<(), DbError> {
        Ok(())
    }
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        None
    }
    fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
        unimplemented!("not exercised by this test")
    }
    fn apply(&mut self, _statements: &[Statement]) -> Result<u64, DbError> {
        unimplemented!("not exercised by this test")
    }
    fn close(&mut self) -> Result<(), DbError> {
        self.log.lock().unwrap().push("close");
        Ok(())
    }
}

#[test]
fn an_idle_session_closes_and_reconnects_lazily_on_the_next_command() {
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let reconnect_calls = Arc::new(AtomicU64::new(0));

    let session = Session::new(Box::new(IdleLoggingConnection {
        log: Arc::clone(&log),
    }));
    let log_for_reconnect = Arc::clone(&log);
    let reconnect_calls_for_closure = Arc::clone(&reconnect_calls);
    let reconnect: Reconnect = Box::new(move || {
        reconnect_calls_for_closure.fetch_add(1, Ordering::SeqCst);
        log_for_reconnect.lock().unwrap().push("reconnect");
        Ok((
            Box::new(IdleLoggingConnection {
                log: Arc::clone(&log_for_reconnect),
            }) as Box<dyn Connection>,
            None,
        ))
    });

    let ran = Arc::new(Mutex::new(Vec::new()));
    let ran_clone = Arc::clone(&ran);
    let worker = SessionWorker::spawn_with_idle_timeout(
        session,
        None,
        Some(Duration::from_secs(1)),
        Some(reconnect),
        move |event| {
            if let SessionEvent::Ran { result, .. } = event {
                ran_clone.lock().unwrap().push(result.is_ok());
            }
        },
    );

    // Nothing sent for longer than the 1-second idle timeout: the
    // worker's own `recv_timeout` loop (`IDLE_CHECK_TICK`) notices and
    // closes the connection with nobody asking it to.
    std::thread::sleep(Duration::from_millis(1300));
    assert_eq!(*log.lock().unwrap(), vec!["close"]);
    assert_eq!(reconnect_calls.load(Ordering::SeqCst), 0);

    // The next command reopens it lazily, through `reconnect` — and
    // still succeeds, proving the new connection is actually used,
    // not just logged.
    worker
        .send(SessionCommand::RunStatement {
            statement: Statement::sql("SELECT 1"),
        })
        .expect("send");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(*log.lock().unwrap(), vec!["close", "reconnect"]);
    assert_eq!(reconnect_calls.load(Ordering::SeqCst), 1);
    assert_eq!(*ran.lock().unwrap(), vec![true]);
}

#[test]
fn an_idle_timeout_with_no_reconnect_closure_never_closes_the_connection() {
    // `run` refuses to close a connection it could never reopen — the
    // guard every existing `SessionWorker::spawn` caller relies on
    // implicitly (they pass `None` for both).
    let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let session = Session::new(Box::new(IdleLoggingConnection {
        log: Arc::clone(&log),
    }));
    let worker = SessionWorker::spawn_with_idle_timeout(
        session,
        None,
        Some(Duration::from_millis(300)),
        None,
        |_| {},
    );
    std::thread::sleep(Duration::from_millis(800));
    assert!(log.lock().unwrap().is_empty());
    drop(worker);
}
