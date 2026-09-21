//! `SessionWorker`: one `std::thread` per data source, holding a live
//! `db_core::session::Session` — the same "never block the Qt thread on a
//! call that may hang talking to an unreachable server" shape
//! `container_core::watcher`'s forwarder thread already gives the
//! Containers dock (`bridge/containers/service.rs`'s doc comment).
//!
//! Self-contained on purpose: no `ffi`/QObject types here at all. F2's
//! `DatabaseService` and F3's console/grid worker each own their own
//! translation from a [`SessionEvent`] into a `qt_thread().queue` closure
//! that updates their own QObject's state and emits their own signal —
//! this module only runs a command loop and hands results back through
//! whatever `on_event` closure its owner supplied.
//!
//! Cancel-race guard: reuses `db_core::session::Session`'s own generation
//! counter (`Session::generation_handle`) rather than inventing a second
//! one — every [`SessionEvent`] carries the generation the session was at
//! when the command was dispatched, so a caller drops a reply whose
//! generation is behind what [`SessionWorker::generation`] reports *now*
//! (bumped on a force-refresh or a disconnect that started before this
//! reply arrived).
//!
//! Per-scope caching: kept here, not in `db_core`, because it is a UI
//! session's own runtime state (what has this particular tree already
//! fetched), not a rule about the domain — the same reason
//! `bridge::containers::service::Connection` (not `container_core`) holds
//! a connection's watcher handle.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use db_core::driver::{CancelHandle, ExecOptions, RowStream, Statement};
use db_core::error::DbError;
use db_core::schema::{IntrospectLevel, IntrospectScope, ObjectRef, SchemaSnapshot};
use db_core::session::Session;
use db_core::value::{ColumnMeta, Value};

/// One request a [`SessionWorker`]'s thread runs against its `Session`.
pub enum SessionCommand {
    /// Fetch `scope` at `level`, honouring the worker's own cache unless
    /// `force` drops it first.
    Introspect {
        scope: IntrospectScope,
        level: IntrospectLevel,
        force: bool,
    },
    /// Drop every cached snapshot whose scope is exactly `scope` — a
    /// plain "Refresh" on one node, as opposed to `force: true` above's
    /// "Force refresh", which drops the whole cache.
    DropCached {
        scope: IntrospectScope,
    },
    DdlOf {
        object: ObjectRef,
    },
    /// Run one statement and report success/failure only — the object
    /// actions (drop/truncate/rename/comment) never need the result rows
    /// a `SELECT` would return, only whether it succeeded.
    RunStatement {
        statement: Statement,
    },
    /// Run one console statement (F3.3). A `Rows`-shaped result parks its
    /// stream in the worker's own local state (keyed by nothing but "the
    /// one active result", since a new statement on the same console
    /// closes the previous one first — `ConsoleService`'s own rule, not
    /// this module's) and reports its first page through
    /// [`SessionEvent::Batch`]; every other shape reports directly.
    Execute {
        statement: Statement,
        options: ExecOptions,
    },
    /// Pull the parked stream's next page, up to `options.fetch_size` rows
    /// (the same size the statement executed with) — `Err` via
    /// [`SessionEvent::Batch`]'s `Error` case when nothing is parked (the
    /// result already finished, or a paging request raced a new
    /// `Execute`).
    FetchMore,
    BeginManual,
    Commit,
    Rollback,
    Shutdown,
}

/// [`SessionCommand::Execute`]/[`SessionCommand::FetchMore`]'s outcome —
/// one shape, reused by both since paging a `Rows` result is exactly "ask
/// for another page of the same shape".
pub enum BatchOutcome {
    /// A page of a `SELECT`-shaped result. `done` once the stream is
    /// exhausted — the same page that reports it also carries any rows it
    /// still had left, so a caller never has to tell "no more rows" apart
    /// from "no more rows, and here are the last few".
    Rows {
        columns: Vec<ColumnMeta>,
        rows: Vec<Vec<Value>>,
        done: bool,
    },
    Affected(u64),
    Ok,
    Error(DbError),
}

/// What a [`SessionCommand`] produced, tagged with the session's
/// generation at the moment the command was dispatched (see this
/// module's doc comment).
pub enum SessionEvent {
    Introspected {
        generation: u64,
        result: Result<SchemaSnapshot, DbError>,
    },
    Ddl {
        generation: u64,
        result: Result<String, DbError>,
    },
    Ran {
        generation: u64,
        result: Result<(), DbError>,
    },
    Batch {
        generation: u64,
        outcome: BatchOutcome,
    },
    TxChanged {
        generation: u64,
        result: Result<(), DbError>,
    },
}

/// One cached snapshot, keyed by the exact scope it was fetched for.
/// `Vec`-backed rather than a `HashMap`: a tree's own open scopes number
/// in the tens, not thousands, so a linear scan costs nothing a hash
/// would meaningfully improve on, and `IntrospectScope` need not derive
/// `Hash` just for this.
struct CacheEntry {
    scope: IntrospectScope,
    level: IntrospectLevel,
    snapshot: SchemaSnapshot,
}

/// Whether a cached snapshot fetched at `have` already covers a request
/// for `want` — `Full` covers everything, `Columns` covers `Names`, and a
/// level only ever covers itself or something shallower.
fn level_covers(have: IntrospectLevel, want: IntrospectLevel) -> bool {
    fn rank(level: IntrospectLevel) -> u8 {
        match level {
            IntrospectLevel::Names => 0,
            IntrospectLevel::Columns => 1,
            IntrospectLevel::Full => 2,
        }
    }
    rank(have) >= rank(want)
}

/// Turn a freshly executed (or paged) statement's shape into the one
/// event both `Execute` and `FetchMore` report through, pulling exactly
/// one page from `stream` when the shape is `Rows` (a page's size is
/// whatever `options.fetch_size` asked `execute` for — see this module's
/// doc comment on [`SessionCommand::FetchMore`]).
fn next_batch_outcome(stream: &mut Box<dyn RowStream>) -> BatchOutcome {
    match stream.next_batch() {
        Ok(Some(batch)) => BatchOutcome::Rows {
            columns: batch.columns,
            rows: batch.rows,
            done: false,
        },
        Ok(None) => BatchOutcome::Rows {
            columns: Vec::new(),
            rows: Vec::new(),
            done: true,
        },
        Err(error) => BatchOutcome::Error(error),
    }
}

fn run(
    mut session: Session,
    receiver: std::sync::mpsc::Receiver<SessionCommand>,
    on_event: impl Fn(SessionEvent) + Send + 'static,
) {
    let mut cache: Vec<CacheEntry> = Vec::new();
    let mut parked_stream: Option<Box<dyn RowStream>> = None;
    while let Ok(command) = receiver.recv() {
        match command {
            SessionCommand::Shutdown => break,
            SessionCommand::Execute { statement, options } => {
                let generation = session.generation();
                parked_stream = None;
                let outcome = match session.execute(&statement, &options) {
                    Ok(db_core::driver::Execution::Rows(mut stream)) => {
                        let first = next_batch_outcome(&mut stream);
                        if !matches!(first, BatchOutcome::Rows { done: true, .. }) {
                            parked_stream = Some(stream);
                        }
                        first
                    }
                    Ok(db_core::driver::Execution::Affected(n)) => BatchOutcome::Affected(n),
                    Ok(db_core::driver::Execution::Ok) => BatchOutcome::Ok,
                    Ok(db_core::driver::Execution::Multi(_)) => BatchOutcome::Error(DbError::new(
                        db_core::error::DbErrorCode::InvalidStatement,
                        "a console statement must not itself be a multi-statement script",
                    )),
                    Err(error) => BatchOutcome::Error(error),
                };
                on_event(SessionEvent::Batch {
                    generation,
                    outcome,
                });
            }
            SessionCommand::FetchMore => {
                let generation = session.generation();
                let outcome = match parked_stream.as_mut() {
                    Some(stream) => {
                        let outcome = next_batch_outcome(stream);
                        if matches!(outcome, BatchOutcome::Rows { done: true, .. }) {
                            parked_stream = None;
                        }
                        outcome
                    }
                    None => BatchOutcome::Error(DbError::new(
                        db_core::error::DbErrorCode::Unknown,
                        "no result is parked to fetch more of",
                    )),
                };
                on_event(SessionEvent::Batch {
                    generation,
                    outcome,
                });
            }
            SessionCommand::BeginManual => {
                let generation = session.generation();
                let result = session.begin_manual();
                on_event(SessionEvent::TxChanged { generation, result });
            }
            SessionCommand::Commit => {
                let generation = session.generation();
                let result = session.commit();
                on_event(SessionEvent::TxChanged { generation, result });
            }
            SessionCommand::Rollback => {
                let generation = session.generation();
                let result = session.rollback();
                on_event(SessionEvent::TxChanged { generation, result });
            }
            SessionCommand::DropCached { scope } => {
                cache.retain(|entry| entry.scope != scope);
            }
            SessionCommand::Introspect {
                scope,
                level,
                force,
            } => {
                let generation = session.generation();
                if force {
                    cache.clear();
                }
                let cached = cache
                    .iter()
                    .find(|entry| entry.scope == scope && level_covers(entry.level, level))
                    .map(|entry| entry.snapshot.clone());
                let result = match cached {
                    Some(snapshot) => Ok(snapshot),
                    None => {
                        let outcome = session.introspect(&scope, level);
                        if let Ok(snapshot) = &outcome {
                            cache.retain(|entry| entry.scope != scope);
                            cache.push(CacheEntry {
                                scope: scope.clone(),
                                level,
                                snapshot: snapshot.clone(),
                            });
                        }
                        outcome
                    }
                };
                on_event(SessionEvent::Introspected { generation, result });
            }
            SessionCommand::DdlOf { object } => {
                let generation = session.generation();
                let result = session.ddl_of(&object);
                on_event(SessionEvent::Ddl { generation, result });
            }
            SessionCommand::RunStatement { statement } => {
                let generation = session.generation();
                let result = session
                    .execute(&statement, &ExecOptions::default())
                    .map(|_| ());
                on_event(SessionEvent::Ran { generation, result });
            }
        }
    }
    let _ = session.close();
}

/// A live connection's own worker thread. Dropping this joins the thread
/// after asking it to shut down — a `DatabaseService` that drops a
/// disconnected source's worker never leaves an orphaned thread (or an
/// open tunnel/connection) behind it.
pub struct SessionWorker {
    sender: std::sync::mpsc::Sender<SessionCommand>,
    generation: Arc<AtomicU64>,
    /// The connection's cancel handle, obtained once at spawn time (see
    /// `db_core::session::Session::cancel_handle`'s doc comment) and kept
    /// outside the command channel entirely — a running `Execute` occupies
    /// the worker thread for as long as it blocks, so a cancel that only
    /// took effect once its own turn came up on that same channel would
    /// never arrive in time to interrupt it. `None` when the backend has
    /// no server-side cancel at all.
    cancel_handle: Arc<Mutex<Option<Box<dyn CancelHandle>>>>,
    thread: Option<JoinHandle<()>>,
}

impl SessionWorker {
    /// Spawn a worker owning `session`. Every [`SessionEvent`] the thread
    /// produces is handed to `on_event` — the caller's own bridge to
    /// `qt_thread().queue`, so this module never depends on cxx-qt.
    pub fn spawn(session: Session, on_event: impl Fn(SessionEvent) + Send + 'static) -> Self {
        let generation = session.generation_handle();
        let cancel_handle = Arc::new(Mutex::new(session.cancel_handle()));
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || run(session, receiver, on_event));
        Self {
            sender,
            generation,
            cancel_handle,
            thread: Some(thread),
        }
    }

    /// Interrupt whatever this connection is doing *right now*, from any
    /// thread — see [`Self::cancel_handle`]'s doc comment. `Err` when the
    /// backend never offered a cancel handle at all; the caller's own
    /// client-side [`db_core::driver::CancelToken`] (checked between pages,
    /// not while blocked inside one `next_batch`/`execute` call) is the
    /// fallback that still stops paging further once a batch returns.
    pub fn cancel_now(&self) -> Result<(), DbError> {
        match self.cancel_handle.lock().unwrap().as_ref() {
            Some(handle) => handle.cancel(),
            None => Err(DbError::new(
                db_core::error::DbErrorCode::NotSupported,
                "this data source has no server-side cancel",
            )),
        }
    }

    /// The session's generation right now — compare a [`SessionEvent`]'s
    /// own `generation` against this to tell a stale reply from a live
    /// one.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Bump the generation so any reply already dispatched before this
    /// point reads as stale once it arrives — call before a disconnect or
    /// a reconnect that should make every in-flight answer irrelevant.
    pub fn invalidate(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Enqueue `command`. `Err` only when the worker thread has already
    /// exited (a `Shutdown` raced this call, or the thread panicked) —
    /// the caller treats that the same as any other connection failure.
    pub fn send(&self, command: SessionCommand) -> Result<(), DbError> {
        self.sender.send(command).map_err(|_| {
            DbError::new(
                db_core::error::DbErrorCode::Unknown,
                "the connection's worker thread is no longer running",
            )
        })
    }
}

impl Drop for SessionWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(SessionCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
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
        let worker = SessionWorker::spawn(session, move |event| {
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
        let worker = SessionWorker::spawn(session, move |event| match event {
            SessionEvent::Ddl { result, .. } => ddls_clone.lock().unwrap().push(result),
            SessionEvent::Ran { result, .. } => ran_clone.lock().unwrap().push(result),
            SessionEvent::Introspected { .. }
            | SessionEvent::Batch { .. }
            | SessionEvent::TxChanged { .. } => {}
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
        let worker = SessionWorker::spawn(session, move |event| {
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
        let worker = SessionWorker::spawn(session, move |event| {
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
        let worker = SessionWorker::spawn(session, |_event| {});
        worker.cancel_now().unwrap();
        assert_eq!(cancel_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn begin_commit_rollback_report_through_tx_changed() {
        let session = Session::new(Box::new(CountingConnection {
            introspect_calls: Arc::new(AtomicU64::new(0)),
            cancel_calls: Arc::new(AtomicU64::new(0)),
        }));
        let results = Arc::new(Mutex::new(Vec::new()));
        let results_clone = Arc::clone(&results);
        let worker = SessionWorker::spawn(session, move |event| {
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
}
