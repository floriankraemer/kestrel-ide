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
use std::time::{Duration, Instant};

use db_core::driver::{CancelHandle, Connection, ExecOptions, RowStream, Statement};
use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{IntrospectLevel, IntrospectScope, ObjectRef, SchemaSnapshot};
use db_core::session::{Session, TxMode};
use db_core::tunnel::Tunnel;
use db_core::value::{ColumnMeta, Value};

/// Reopens a connection (and its tunnel, if any) exactly the way it was
/// first opened — `bridge::database::open_session`'s own closure, so a
/// data source's SSH tunnel/TLS/auth settings never need re-deriving
/// here. Used only by the idle-close reconnect below (database-tools-plan
/// FZ); every other caller passes `None` through [`SessionWorker::spawn`]
/// and never grows an idle timer at all.
pub type Reconnect =
    Box<dyn Fn() -> Result<(Box<dyn Connection>, Option<Box<dyn Tunnel>>), DbError> + Send>;

/// A placeholder connection an idle-closed [`Session`] holds between the
/// moment its real connection was dropped (freeing the socket/tunnel) and
/// the next command that actually needs one — every method reports
/// `ConnectionFailed` except `close`, which is already a no-op the real
/// close path already ran. `run`'s own loop always reconnects (or reports
/// this error) *before* ever handing a command to the session, so none of
/// these bodies are reachable in practice; they exist only so `Session`
/// never holds a dangling `Box<dyn Connection>`.
struct ClosedConnection;

impl Connection for ClosedConnection {
    fn dialect(&self) -> db_core::dialect::Dialect {
        db_core::dialect::Dialect::Sqlite
    }
    fn server_info(&self) -> String {
        "closed (idle) — reconnecting on next use".to_string()
    }
    fn introspect(
        &mut self,
        _scope: &IntrospectScope,
        _level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        Err(closed_err())
    }
    fn execute(
        &mut self,
        _statement: &Statement,
        _options: &ExecOptions,
    ) -> Result<db_core::driver::Execution, DbError> {
        Err(closed_err())
    }
    fn begin(&mut self) -> Result<(), DbError> {
        Err(closed_err())
    }
    fn commit(&mut self) -> Result<(), DbError> {
        Err(closed_err())
    }
    fn rollback(&mut self) -> Result<(), DbError> {
        Err(closed_err())
    }
    fn set_read_only(&mut self, _read_only: bool) -> Result<(), DbError> {
        Err(closed_err())
    }
    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        None
    }
    fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
        Err(closed_err())
    }
    fn apply(&mut self, _statements: &[Statement]) -> Result<u64, DbError> {
        Err(closed_err())
    }
    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

fn closed_err() -> DbError {
    DbError::new(
        DbErrorCode::ConnectionFailed,
        "this connection was closed for being idle and could not be reopened",
    )
}

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
    /// Runs a data-editor submit's bound statements (F4.2) through
    /// `Session::apply` — auto mode: whatever transaction state the
    /// session is already in (its own `apply` does not open one of its
    /// own); manual mode: inside the console's already-open transaction,
    /// same as any other statement run while one is open.
    Apply {
        statements: Vec<Statement>,
    },
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
    /// [`SessionCommand::Apply`]'s outcome — the total row count
    /// `Connection::apply` reported, or the first statement's failure.
    Applied {
        generation: u64,
        result: Result<u64, DbError>,
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

/// How often the idle-close loop wakes up to check the clock — a `recv_
/// timeout` tick, not a second thread (database-tools-plan FZ: "the
/// worker's own `recv_timeout` loop, no extra thread"). Short enough that
/// a 1-second test setting (see the tests below) still closes promptly,
/// long enough not to spin a channel-idle worker thread.
const IDLE_CHECK_TICK: Duration = Duration::from_millis(200);

#[allow(clippy::too_many_arguments)]
fn run(
    mut session: Session,
    mut tunnel: Option<Box<dyn Tunnel>>,
    receiver: std::sync::mpsc::Receiver<SessionCommand>,
    idle_timeout: Option<Duration>,
    reconnect: Option<Reconnect>,
    cancel_handle: Arc<Mutex<Option<Box<dyn CancelHandle>>>>,
    on_event: impl Fn(SessionEvent) + Send + 'static,
) {
    let mut cache: Vec<CacheEntry> = Vec::new();
    let mut parked_stream: Option<Box<dyn RowStream>> = None;
    // `None` (`idle_close_minutes_or_default`'s own "0 = never" contract,
    // `app-config::database`, mapped to `None` by the caller) and "no
    // reconnect closure at all" (every existing `SessionWorker::spawn`
    // caller) both disable the timer.
    let idle_timeout = reconnect.is_some().then_some(idle_timeout).flatten();
    let mut last_used = Instant::now();
    let mut closed = false;

    loop {
        let command = match receiver.recv_timeout(IDLE_CHECK_TICK) {
            Ok(command) => command,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Manual tx and a parked cursor each pin a connection —
                // matching decision #9 in database-tools-plan.md §3 — so
                // idle-close only ever fires on an otherwise-quiescent
                // Auto-mode session.
                if !closed
                    && parked_stream.is_none()
                    && session.tx_mode() == TxMode::Auto
                    && idle_timeout.is_some_and(|timeout| last_used.elapsed() >= timeout)
                {
                    let _ = session.close();
                    session.replace_connection(Box::new(ClosedConnection));
                    tunnel = None;
                    *cancel_handle.lock().unwrap() = None;
                    cache.clear();
                    closed = true;
                }
                continue;
            }
        };
        if matches!(command, SessionCommand::Shutdown) {
            break;
        }
        last_used = Instant::now();
        if closed {
            // Lazily reopen — through the very same `open_session`
            // construction path the worker was originally spawned with,
            // so an SSH tunnel comes back too, never just the bare
            // connection.
            let reconnect_fn = reconnect.as_ref().expect("idle_timeout implies Some");
            match reconnect_fn() {
                Ok((connection, new_tunnel)) => {
                    session.replace_connection(connection);
                    *cancel_handle.lock().unwrap() = session.cancel_handle();
                    tunnel = new_tunnel;
                    closed = false;
                }
                Err(error) => {
                    report_reconnect_failure(&command, session.generation(), error, &on_event);
                    continue;
                }
            }
        }
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
            SessionCommand::Apply { statements } => {
                let generation = session.generation();
                // Auto mode: this submit's own all-or-nothing transaction
                // — a mid-batch failure must not leave some of a
                // multi-row edit applied and some not. Manual mode: the
                // console already has a transaction open (or the user
                // will commit/rollback explicitly either way), so this
                // never begins/commits one of its own — it runs inside
                // whatever is already open, same as any other statement.
                let auto_wrapped = session.tx_mode() == db_core::session::TxMode::Auto;
                if auto_wrapped {
                    if let Err(error) = session.begin_manual() {
                        on_event(SessionEvent::Applied {
                            generation,
                            result: Err(error),
                        });
                        continue;
                    }
                }
                let result = session.apply(&statements);
                if auto_wrapped {
                    let closed = if result.is_ok() {
                        session.commit()
                    } else {
                        session.rollback()
                    };
                    if let Err(close_error) = closed {
                        on_event(SessionEvent::Applied {
                            generation,
                            result: Err(close_error),
                        });
                        continue;
                    }
                }
                on_event(SessionEvent::Applied { generation, result });
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
    // Explicit, not relying on parameter declaration order (which drops
    // in reverse — `tunnel` before `session` — the opposite of what this
    // module's own doc comment requires): the connection must be fully
    // dropped *before* the tunnel it was using, never the other way
    // around, or the tunnel severs a port the connection is still on.
    drop(session);
    drop(tunnel);
}

/// [`SessionCommand`]'s matching failure [`SessionEvent`], reported when a
/// lazy reconnect fails instead of ever handing the command to the
/// session — every variant maps to exactly the event its own arm below
/// would have emitted on an `Err` from the connection itself, so a caller
/// sees "this command failed", never a hang or a silently dropped reply.
/// `DropCached`/`Shutdown` need no event: the former never emitted one to
/// begin with, and the latter can never reach this function (`run`
/// handles it before ever checking `closed`).
fn report_reconnect_failure(
    command: &SessionCommand,
    generation: u64,
    error: DbError,
    on_event: &impl Fn(SessionEvent),
) {
    match command {
        SessionCommand::Introspect { .. } => on_event(SessionEvent::Introspected {
            generation,
            result: Err(error),
        }),
        SessionCommand::DdlOf { .. } => on_event(SessionEvent::Ddl {
            generation,
            result: Err(error),
        }),
        SessionCommand::RunStatement { .. } => on_event(SessionEvent::Ran {
            generation,
            result: Err(error),
        }),
        SessionCommand::Execute { .. } | SessionCommand::FetchMore => {
            on_event(SessionEvent::Batch {
                generation,
                outcome: BatchOutcome::Error(error),
            })
        }
        SessionCommand::BeginManual | SessionCommand::Commit | SessionCommand::Rollback => {
            on_event(SessionEvent::TxChanged {
                generation,
                result: Err(error),
            })
        }
        SessionCommand::Apply { .. } => on_event(SessionEvent::Applied {
            generation,
            result: Err(error),
        }),
        SessionCommand::DropCached { .. } | SessionCommand::Shutdown => {}
    }
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
    /// Spawn a worker owning `session`, and `tunnel` (if `session`'s
    /// connection was opened through one — F7c, `bridge::database::
    /// open_session`). Every [`SessionEvent`] the thread produces is handed
    /// to `on_event` — the caller's own bridge to `qt_thread().queue`, so
    /// this module never depends on cxx-qt.
    ///
    /// `tunnel` is captured by the same thread closure as `session`, after
    /// it, so it drops only once [`run`] returns — i.e. only after the
    /// connection inside `session` has already been closed (`Drop for
    /// SessionWorker` sends `Shutdown` and joins this thread before
    /// returning), never before. Closing the tunnel first would sever the
    /// forwarded port out from under a connection still using it.
    pub fn spawn(
        session: Session,
        tunnel: Option<Box<dyn Tunnel>>,
        on_event: impl Fn(SessionEvent) + Send + 'static,
    ) -> Self {
        Self::spawn_with_idle_close(session, tunnel, 0, None, on_event)
    }

    /// [`Self::spawn`] plus an idle-close timer (database-tools-plan FZ,
    /// `app_config::database::idle_close_minutes`): after `idle_close_
    /// minutes` of no command at all, the worker drops the real
    /// connection (and `tunnel`) to free the socket, and lazily reopens
    /// one — via `reconnect` — the next time a command actually needs it.
    /// `idle_close_minutes == 0` or `reconnect: None` disables the timer
    /// entirely, which is exactly what [`Self::spawn`] passes, so every
    /// existing caller is unaffected.
    ///
    /// Manual tx and a parked cursor each pin a connection regardless of
    /// how long it has sat idle (decision #9, database-tools-plan.md §3)
    /// — `run`'s own loop is what actually enforces that, not this
    /// constructor.
    pub fn spawn_with_idle_close(
        session: Session,
        tunnel: Option<Box<dyn Tunnel>>,
        idle_close_minutes: u32,
        reconnect: Option<Reconnect>,
        on_event: impl Fn(SessionEvent) + Send + 'static,
    ) -> Self {
        let idle_timeout =
            (idle_close_minutes > 0).then(|| Duration::from_secs(idle_close_minutes as u64 * 60));
        Self::spawn_with_idle_timeout(session, tunnel, idle_timeout, reconnect, on_event)
    }

    /// [`Self::spawn_with_idle_close`], but taking the idle timeout as a
    /// `Duration` directly rather than whole minutes — the seam the tests
    /// below use to exercise a sub-minute timeout without waiting a real
    /// minute for it; production code always goes through
    /// [`Self::spawn_with_idle_close`].
    fn spawn_with_idle_timeout(
        session: Session,
        tunnel: Option<Box<dyn Tunnel>>,
        idle_timeout: Option<Duration>,
        reconnect: Option<Reconnect>,
        on_event: impl Fn(SessionEvent) + Send + 'static,
    ) -> Self {
        let generation = session.generation_handle();
        let cancel_handle = Arc::new(Mutex::new(session.cancel_handle()));
        let cancel_handle_for_thread = Arc::clone(&cancel_handle);
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            run(
                session,
                tunnel,
                receiver,
                idle_timeout,
                reconnect,
                cancel_handle_for_thread,
                on_event,
            );
        });
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
#[path = "sessions_tests.rs"]
mod tests;
