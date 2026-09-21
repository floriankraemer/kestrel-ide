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
use std::sync::Arc;
use std::thread::JoinHandle;

use db_core::driver::{ExecOptions, Statement};
use db_core::error::DbError;
use db_core::schema::{IntrospectLevel, IntrospectScope, ObjectRef, SchemaSnapshot};
use db_core::session::Session;

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
    Shutdown,
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

fn run(
    mut session: Session,
    receiver: std::sync::mpsc::Receiver<SessionCommand>,
    on_event: impl Fn(SessionEvent) + Send + 'static,
) {
    let mut cache: Vec<CacheEntry> = Vec::new();
    while let Ok(command) = receiver.recv() {
        match command {
            SessionCommand::Shutdown => break,
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
    thread: Option<JoinHandle<()>>,
}

impl SessionWorker {
    /// Spawn a worker owning `session`. Every [`SessionEvent`] the thread
    /// produces is handed to `on_event` — the caller's own bridge to
    /// `qt_thread().queue`, so this module never depends on cxx-qt.
    pub fn spawn(session: Session, on_event: impl Fn(SessionEvent) + Send + 'static) -> Self {
        let generation = session.generation_handle();
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || run(session, receiver, on_event));
        Self {
            sender,
            generation,
            thread: Some(thread),
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

    /// A `Connection` double whose `introspect` counts every call it
    /// receives — proves the worker's own cache actually avoids a repeat
    /// query, which a mock that just returns canned data could not.
    struct CountingConnection {
        introspect_calls: Arc<AtomicU64>,
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
            None
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
        }));
        let ddls = Arc::new(Mutex::new(Vec::new()));
        let ran = Arc::new(Mutex::new(Vec::new()));
        let ddls_clone = Arc::clone(&ddls);
        let ran_clone = Arc::clone(&ran);
        let worker = SessionWorker::spawn(session, move |event| match event {
            SessionEvent::Ddl { result, .. } => ddls_clone.lock().unwrap().push(result),
            SessionEvent::Ran { result, .. } => ran_clone.lock().unwrap().push(result),
            SessionEvent::Introspected { .. } => {}
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
}
