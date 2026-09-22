//! `ConsoleService`/`ResultProvider` (database-tools-plan F3.1/F3.3-F3.5):
//! a console tab attaches to a connected data source, runs a
//! statement/selection/script against its own dedicated `SessionWorker`
//! (never the Database dock's tree worker — a long-running grid page and a
//! tree expand must not queue behind one another on the same connection),
//! and the grid pages the parked `RowStream` through the result it started.
//!
//! `Shared` is the one piece of state both QObjects need — see
//! `bridge::registry::shared_database_consoles`'s doc comment for why it
//! lives behind a thread-local rather than a constructor argument.

use std::collections::HashMap;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Instant;

use cxx_qt::Threading;
use cxx_qt_lib::{QString, QStringList};

use db_core::dialect::Dialect;
use db_core::driver::{ExecOptions, Statement as DbStatement};
use db_core::error::DbError;
use db_core::readonly::Guard;
use db_core::result::{MemoryCap, ResultSet};
use db_core::session::Session;
use db_core::value::{ColumnMeta, Value};
use db_sql::classify::FamilyClassifier;

use crate::bridge::errors;
use crate::bridge::ffi::{
    self, FfiDbColumn, FfiDbError, FfiDbExecWhat, FfiDbRow, FfiDbScriptPolicy, FfiDbTxMode,
    FfiResult,
};

use super::edit::{apply_edit_lookup, apply_submit_outcome, EditState};
use super::sessions::{BatchOutcome, SessionCommand, SessionEvent, SessionWorker};

const CELL_SEP: char = '\u{1f}'; // unit separator — no `Value::display` text contains it.

/// `rowPage`'s own clamp — the same "a caller cannot ask for the whole
/// gigabyte in one FFI call" rule `MAX_HEX_ROWS_PER_REQUEST` already
/// applies to the hex viewer (`bridge::convert`'s own doc comment).
const MAX_ROWS_PER_REQUEST: u64 = 4096;

fn app_settings() -> app_config::database::DatabaseSettings {
    crate::bridge::convert::load_settings().database
}

/// `app_config::database::DataSourceSetting::script_policy`'s free-form
/// string vocabulary, both directions — see that field's own doc comment
/// on why it is a string, not this enum, in persistence.
fn policy_from_setting(text: &str) -> FfiDbScriptPolicy {
    match text {
        "continue" => FfiDbScriptPolicy::Continue,
        "ask" => FfiDbScriptPolicy::Ask,
        _ => FfiDbScriptPolicy::StopOnError,
    }
}

fn policy_to_setting(policy: FfiDbScriptPolicy) -> &'static str {
    match policy {
        FfiDbScriptPolicy::Continue => "continue",
        FfiDbScriptPolicy::Ask => "ask",
        _ => "stop_on_error",
    }
}

/// One console tab's live state.
///
/// `pub(crate)` throughout: `bridge::database::edit` (F4.1/F4.2, split out
/// once this module hit the file-size ceiling) reads/mutates this from a
/// sibling file, the same "internal to the crate, not to this one module"
/// visibility `Shared` below already needed for `ConsoleService`/
/// `ResultProvider` to share it.
pub(crate) struct ConsoleState {
    pub(crate) source_id: String,
    pub(crate) worker: SessionWorker,
    pub(crate) dialect: Dialect,
    pub(crate) guard: Guard,
    pub(crate) tx_mode: FfiDbTxMode,
    pub(crate) script_policy: FfiDbScriptPolicy,
    pub(crate) page_size: u32,
    pub(crate) history: bool,
    /// The result this console's *last* execute started, if it is still
    /// the active one — a fresh `execute` on the same console drops
    /// whatever the previous one had parked (this module's own doc
    /// comment's "closes it first" rule).
    pub(crate) current_result: Option<u64>,
    /// A multi-statement run still in progress: the statements not yet
    /// dispatched, plus the policy governing what happens on a failure.
    pub(crate) pending: Option<PendingScript>,
    /// Schema names from the source's own `Names`-level snapshot, for the
    /// console bar's schema picker (database-tools-plan F3e) — empty
    /// until the background `Introspect` this console kicks off at
    /// `attach` time lands, and always empty for a dialect with no schema
    /// concept (SQLite).
    pub(crate) schemas: Vec<String>,
    /// Every `Introspect` this console has dispatched and not yet been
    /// answered for, oldest first — the worker is one thread processing
    /// one command at a time over an ordered channel, so a reply always
    /// answers the front of this queue, regardless of whether it succeeds
    /// or fails. Two different call sites dispatch `Introspect` (the
    /// schema picker at `attach` time, F4.1's per-result editability
    /// lookup) and a `DbError` reply carries no level/scope to tell them
    /// apart by itself — this queue is what does, rather than the
    /// `Ok(snapshot) => snapshot.level == Full` guess an error reply
    /// cannot make (a real bug this queue replaces: an unrelated schema-
    /// picker failure could otherwise resolve a pending edit-lookup with
    /// the wrong error, or vice versa).
    pub(crate) pending_introspects: std::collections::VecDeque<IntrospectPurpose>,
    /// The one result a `submit` (F4.2) is waiting on an `Applied` reply
    /// for — `None` once answered. A console only ever has one submit in
    /// flight at a time (the grid disables Submit while one is pending),
    /// so a single slot (not a queue) is enough.
    pub(crate) submitting: Option<u64>,
}

/// What a dispatched `Introspect` command answers — see
/// `ConsoleState::pending_introspects`'s own doc comment.
pub(crate) enum IntrospectPurpose {
    /// The console bar's schema picker (`ConsoleState::schemas`).
    SchemaPicker,
    /// F4.1's editability lookup for one result, over the given table.
    EditLookup(u64, db_sql::single_table::TableRef),
}

pub(crate) struct PendingScript {
    remaining: std::collections::VecDeque<String>,
    index: u32,
    total: u32,
    policy: FfiDbScriptPolicy,
    /// Set while `FfiDbScriptPolicy::Ask` is waiting on `resume` — the
    /// still-undispatched remainder is kept in `remaining` either way.
    awaiting_resume: bool,
}

/// One execution's accumulated rows and outcome — what `ResultProvider`
/// reads from, and what `rowsAppended`/`executionFinished` describe.
/// `pub(crate)`: see `ConsoleState`'s own doc comment.
pub(crate) struct ResultState {
    pub(crate) tab_id: u64,
    pub(crate) columns: Vec<ColumnMeta>,
    pub(crate) set: ResultSet,
    /// The underlying stream is exhausted (or the statement was not a
    /// `Rows` shape at all) — nothing further to page. Distinct from
    /// `reported`: a `Rows` result reports `executionFinished` on its
    /// *first* batch, long before this is ever `true`.
    pub(crate) done: bool,
    /// `executionFinished` has already fired for this result — guards
    /// [`ffi::ConsoleService::report_and_advance`] so a later
    /// `ResultProvider::fetchMore` page never re-reports or re-advances a
    /// script a second time.
    pub(crate) reported: bool,
    pub(crate) cap_reached: bool,
    pub(crate) affected: Option<u64>,
    pub(crate) error: Option<DbError>,
    pub(crate) started_at: Instant,
    pub(crate) elapsed_ms: Option<u64>,
    /// The statement text this result came from — `applyClauses` wraps
    /// this as a derived table rather than needing its own copy tracked
    /// by the caller.
    pub(crate) statement_text: String,
    /// The data editor's own decision for this result (F4.1) — `Unknown`
    /// until the first `Rows` batch's columns are in hand (and, for a
    /// candidate single-table result, until the `Full`-level introspect
    /// this triggers lands too).
    pub(crate) edit: EditState,
    /// The same `Full`-level table introspect [`EditState`] resolves
    /// editability from, kept a second time here as the constraint list
    /// alone (F4.3's FK navigation, `edit::apply_edit_lookup`'s own
    /// populate step) — empty until that introspect lands, or forever for
    /// a result that never became a data-editor candidate in the first
    /// place (not a single table, a read-only source, …).
    pub(crate) table_constraints: Vec<db_core::schema::ConstraintKind>,
}

/// State `ConsoleService` and `ResultProvider` both read/mutate — see this
/// module's doc comment.
#[derive(Default)]
pub struct Shared {
    pub(crate) consoles: HashMap<u64, ConsoleState>,
    pub(crate) results: HashMap<u64, ResultState>,
    next_result_id: u64,
}

impl Shared {
    pub(crate) fn alloc_result_id(&mut self) -> u64 {
        self.next_result_id += 1;
        self.next_result_id
    }
}

fn to_ffi_error(error: &DbError) -> FfiDbError {
    FfiDbError {
        code: error.code as u32 as i32,
        message: QString::from(error.message.as_str()),
        line: -1,
        col: -1,
    }
}

fn ok_error() -> FfiDbError {
    FfiDbError {
        code: 0,
        message: QString::default(),
        line: -1,
        col: -1,
    }
}

/// Splits `text` into individual statement texts, dropping empty/
/// whitespace-only ones — dialect-aware and quote/comment safe for the
/// SQL family (`db_sql::split`), and family-native for Mongo (top-level
/// `db.coll.method(…)`/JSON documents, `db_sql::mongo::split_commands`)
/// and Redis (one command per line, `db_sql::resp::split_lines`), so a
/// SQL-shaped splitter never runs over `db.coll.find(…)` sugar or a RESP
/// line and misreads its `;`/`{}` (F7b).
fn split_statements(text: &str, dialect: Dialect) -> Vec<String> {
    match dialect {
        Dialect::Mongo => db_sql::mongo::split_commands(text)
            .into_iter()
            .map(str::to_string)
            .collect(),
        Dialect::Redis => db_sql::resp::split_lines(text)
            .into_iter()
            .map(str::to_string)
            .collect(),
        _ => db_sql::split(text, dialect)
            .into_iter()
            .map(|statement| statement.text(text).trim().to_string())
            .filter(|text| !text.is_empty())
            .collect(),
    }
}

/// The one statement whose span contains `caret` (a byte offset into
/// `text`), or the last statement if the caret sits past every span (e.g.
/// at end of file right after the final `;`/newline).
fn statement_at_caret(text: &str, dialect: Dialect, caret: usize) -> Option<String> {
    match dialect {
        Dialect::Mongo => {
            let spans = db_sql::mongo::command_spans(text);
            let hit = spans
                .iter()
                .find(|(start, end)| caret >= *start && caret <= *end)
                .or_else(|| spans.last())?;
            let rendered = text[hit.0..hit.1].trim().to_string();
            (!rendered.is_empty()).then_some(rendered)
        }
        Dialect::Redis => {
            let mut offset = 0usize;
            for line in text.split_inclusive('\n') {
                let end = offset + line.len();
                if caret >= offset && caret <= end {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        return Some(trimmed.to_string());
                    }
                }
                offset = end;
            }
            db_sql::resp::split_lines(text)
                .last()
                .map(|s| s.to_string())
        }
        _ => {
            let statements = db_sql::split(text, dialect);
            let hit = statements
                .iter()
                .find(|statement| caret >= statement.start && caret <= statement.end)
                .or_else(|| statements.last())?;
            let rendered = hit.text(text).trim().to_string();
            (!rendered.is_empty()).then_some(rendered)
        }
    }
}

/// Every `ObjectKind::Schema` node's name anywhere in `roots`, depth-first
/// — a `Names`-level snapshot is small enough that a full walk costs
/// nothing, and this stays correct regardless of whether a backend nests
/// schemas under a catalog root or reports them as the roots themselves.
fn collect_schema_names(roots: &[db_core::schema::Node]) -> Vec<String> {
    let mut names = Vec::new();
    for node in roots {
        if node.kind == db_core::schema::ObjectKind::Schema {
            names.push(node.name.clone());
        }
        if let db_core::schema::Children::Loaded(children) = &node.children {
            names.extend(collect_schema_names(children));
        }
    }
    names
}

pub struct ConsoleServiceRust {
    pub(crate) shared: Rc<std::cell::RefCell<Shared>>,
    /// For `dmlPreview` (F4.2) only — opening a `db-dml` virtual document
    /// is `AppSession`'s own call, mirroring `DatabaseServiceRust::session`
    /// exactly (that module's own doc comment on why cxx-qt gives every
    /// QObject its own `Default`-constructed state rather than a shared
    /// instance passed in).
    pub(crate) session: Rc<std::cell::RefCell<app_core::AppSession>>,
}

impl Default for ConsoleServiceRust {
    fn default() -> Self {
        Self {
            shared: crate::bridge::registry::shared_database_consoles(),
            session: crate::bridge::registry::shared_session(),
        }
    }
}

impl ffi::ConsoleService {
    /// Attaches `tab_id` to `source_id`: connects a dedicated
    /// `SessionWorker` off the Qt thread (mirrors `DatabaseService::
    /// connect_source`'s own doc comment) and reports through
    /// `outputAppended` once the connect finishes either way.
    pub fn attach(mut self: Pin<&mut Self>, tab_id: u64, source_id: &QString) -> FfiResult {
        let source_id = source_id.to_string();
        let Some(setting) = super::service::configured_sources()
            .into_iter()
            .find(|s| s.id == source_id)
        else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("no data source with id '{source_id}' is configured"),
            );
        };
        let settings = app_settings();
        let page_size = settings.page_size_or_default();
        let history = setting.history;
        let initial_policy = policy_from_setting(setting.script_policy_or_default());
        let qt_thread = self.as_mut().qt_thread();
        let attach_source_id = source_id.clone();
        std::thread::spawn(move || {
            let secrets = super::service::secrets_for(&attach_source_id);
            let data_source = db_core::datasource::DataSource::from_setting(&setting, &secrets);
            let read_only = data_source.read_only;
            let outcome =
                super::open_session(&data_source, &secrets).map_err(|error| error.to_string());
            let _ = qt_thread.queue(
                move |mut service: Pin<&mut ffi::ConsoleService>| match outcome {
                    Ok((tunnel, mut connection)) => {
                        if read_only {
                            let _ = connection.set_read_only(true);
                        }
                        let dialect = connection.dialect();
                        let session = Session::new(connection);
                        let inner_qt_thread = service.as_mut().qt_thread();
                        let worker_tab_id = tab_id;
                        let worker = SessionWorker::spawn(session, tunnel, move |event| {
                            let _ = inner_qt_thread.queue(
                                move |service: Pin<&mut ffi::ConsoleService>| {
                                    apply_event(service, worker_tab_id, event);
                                },
                            );
                        });
                        service.shared.borrow_mut().consoles.insert(
                            tab_id,
                            ConsoleState {
                                source_id: attach_source_id.clone(),
                                worker,
                                dialect,
                                guard: Guard::new(
                                    read_only,
                                    Box::new(FamilyClassifier { dialect }),
                                ),
                                tx_mode: FfiDbTxMode::Auto,
                                script_policy: initial_policy,
                                page_size,
                                history,
                                current_result: None,
                                pending: None,
                                schemas: Vec::new(),
                                pending_introspects: std::collections::VecDeque::new(),
                                submitting: None,
                            },
                        );
                        // The schema picker's own list — a `Names`-level
                        // introspect, the same cheap level a tree's
                        // initial expand always uses (`schema.rs`'s own
                        // doc comment); its reply lands on
                        // `SessionEvent::Introspected` below.
                        if let Some(console) = service.shared.borrow_mut().consoles.get_mut(&tab_id)
                        {
                            if console
                                .worker
                                .send(SessionCommand::Introspect {
                                    scope: db_core::schema::IntrospectScope::default(),
                                    level: db_core::schema::IntrospectLevel::Names,
                                    force: false,
                                })
                                .is_ok()
                            {
                                console
                                    .pending_introspects
                                    .push_back(IntrospectPurpose::SchemaPicker);
                            }
                        }
                        service.as_mut().output_appended(
                            tab_id,
                            QString::from(format!("Attached to '{attach_source_id}'.").as_str()),
                        );
                    }
                    Err(message) => {
                        service.as_mut().output_appended(
                            tab_id,
                            QString::from(format!("Could not attach: {message}").as_str()),
                        );
                    }
                },
            );
        });
        FfiResult::default()
    }

    pub fn detach(self: Pin<&mut Self>, tab_id: u64) {
        self.shared.borrow_mut().consoles.remove(&tab_id);
    }

    pub fn set_tx_mode(self: Pin<&mut Self>, tab_id: u64, mode: FfiDbTxMode) -> FfiResult {
        let mut shared = self.shared.borrow_mut();
        let Some(console) = shared.consoles.get_mut(&tab_id) else {
            return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "no such console");
        };
        if console.tx_mode == mode {
            return FfiResult::default();
        }
        let command = match mode {
            FfiDbTxMode::Manual => SessionCommand::BeginManual,
            _ => SessionCommand::Commit,
        };
        console.tx_mode = mode;
        match console.worker.send(command) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }

    /// Sets `tab_id`'s in-memory policy and persists it as this console's
    /// source's own default (database-tools-plan F3e), so the next
    /// console attached to that source starts with the same choice.
    /// `tab_id`'s current policy — `StopOnError` for a tab this object has
    /// never attached (the same default `attach` falls back to when a
    /// source has no `script_policy` of its own).
    pub fn script_policy(self: Pin<&mut Self>, tab_id: u64) -> FfiDbScriptPolicy {
        self.shared
            .borrow()
            .consoles
            .get(&tab_id)
            .map(|console| console.script_policy)
            .unwrap_or(FfiDbScriptPolicy::StopOnError)
    }

    pub fn set_script_policy(self: Pin<&mut Self>, tab_id: u64, policy: FfiDbScriptPolicy) {
        let source_id = {
            let mut shared = self.shared.borrow_mut();
            let Some(console) = shared.consoles.get_mut(&tab_id) else {
                return;
            };
            console.script_policy = policy;
            console.source_id.clone()
        };
        super::settings::persist_script_policy(&source_id, policy_to_setting(policy));
    }

    /// Schema names from `tab_id`'s source's own `Names`-level snapshot —
    /// empty until `attach`'s own background introspect lands, or always
    /// for a dialect with no schema concept (`Dialect::
    /// set_schema_statement`'s own doc comment).
    pub fn schemas(self: Pin<&mut Self>, tab_id: u64) -> QStringList {
        self.shared
            .borrow()
            .consoles
            .get(&tab_id)
            .map(|console| console.schemas.iter().map(QString::from).collect())
            .unwrap_or_default()
    }

    /// Switches `tab_id`'s session to `schema`, running the dialect's own
    /// `SET`/`USE` statement like any other statement (it lands in the
    /// Output tab, the read-only guard sees it as session control, not a
    /// data touch — `db_sql::classify`'s own doc comment) — a no-op with
    /// `code == 0` for a dialect with none.
    pub fn set_schema(mut self: Pin<&mut Self>, tab_id: u64, schema: &QString) -> FfiResult {
        let schema = schema.to_string();
        let dialect = {
            let shared = self.shared.borrow();
            let Some(console) = shared.consoles.get(&tab_id) else {
                return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "no such console");
            };
            console.dialect
        };
        match dialect.set_schema_statement(&schema) {
            Some(statement) => self.as_mut().begin_run(tab_id, vec![statement]),
            None => FfiResult::default(),
        }
    }

    /// Runs `text` against `tab_id`'s attached source. `what` decides how
    /// `text`/`caret` are read: `Statement` splits the *whole* console
    /// buffer and runs only the one statement `caret` (a byte offset)
    /// falls inside; `Selection` splits `text` (already just the selected
    /// range) and runs every statement in it, in order; `File` is not
    /// valid here (use `executeFile`, which reads from disk itself).
    pub fn execute(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        text: &QString,
        what: FfiDbExecWhat,
        caret: u32,
    ) -> FfiResult {
        let text = text.to_string();
        let statements = {
            let shared = self.shared.borrow();
            let Some(console) = shared.consoles.get(&tab_id) else {
                return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "no such console");
            };
            match what {
                FfiDbExecWhat::Statement => {
                    match statement_at_caret(&text, console.dialect, caret as usize) {
                        Some(statement) => vec![statement],
                        None => return errors::failure(errors::CODE_REFUSED, "nothing to run"),
                    }
                }
                FfiDbExecWhat::Selection => split_statements(&text, console.dialect),
                FfiDbExecWhat::File => {
                    return errors::failure(
                        errors::CODE_INVALID_ARGUMENT,
                        "use executeFile to run a script from disk",
                    );
                }
                _ => return errors::failure(errors::CODE_INVALID_ARGUMENT, "unknown `what`"),
            }
        };
        if statements.is_empty() {
            return errors::failure(errors::CODE_REFUSED, "nothing to run");
        }
        self.as_mut().begin_run(tab_id, statements)
    }

    /// Runs `path`'s whole contents as a script against `source_id` — a
    /// run configuration's own entry point (F3.6), independent of any
    /// console tab (`tab_id` is `0`, the same "no tab" sentinel `openFile`
    /// uses, so `executionStarted`/`outputAppended` still have somewhere
    /// to report without pretending a console is open).
    pub fn execute_file(
        mut self: Pin<&mut Self>,
        tab_id: u64,
        path: &QString,
        source_id: &QString,
    ) -> FfiResult {
        let path = path.to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => return errors::failure(errors::CODE_ATTACHMENT_IO, error.to_string()),
        };
        {
            if !self.shared.borrow().consoles.contains_key(&tab_id) {
                let attach_result = self.as_mut().attach(tab_id, source_id);
                if attach_result.code != 0 {
                    return attach_result;
                }
            }
        }
        let dialect = {
            let shared = self.shared.borrow();
            match shared.consoles.get(&tab_id) {
                Some(console) => console.dialect,
                None => Dialect::Sqlite, // attach is async; split with a
                                         // reasonable default, statements
                                         // are re-validated once it lands.
            }
        };
        let statements = split_statements(&text, dialect);
        if statements.is_empty() {
            return errors::failure(errors::CODE_REFUSED, "the script is empty");
        }
        self.as_mut().begin_run(tab_id, statements)
    }

    /// Best-effort interrupt of whatever `tab_id`'s console is doing right
    /// now (`SessionWorker::cancel_now`'s own doc comment) — `Err` when
    /// the backend offers no server-side cancel at all.
    pub fn cancel(self: Pin<&mut Self>, tab_id: u64) -> FfiResult {
        let shared = self.shared.borrow();
        let Some(console) = shared.consoles.get(&tab_id) else {
            return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "no such console");
        };
        match console.worker.cancel_now() {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }

    pub fn commit(self: Pin<&mut Self>, tab_id: u64) -> FfiResult {
        self.send_tx(tab_id, SessionCommand::Commit)
    }

    pub fn rollback(self: Pin<&mut Self>, tab_id: u64) -> FfiResult {
        self.send_tx(tab_id, SessionCommand::Rollback)
    }

    /// Answers an `askContinue` this console raised: `proceed` runs the
    /// rest of the pending script, `false` abandons it (nothing further is
    /// dispatched).
    pub fn resume(mut self: Pin<&mut Self>, result_id: u64, proceed: bool) {
        let tab_id = {
            let shared = self.shared.borrow();
            shared.results.get(&result_id).map(|result| result.tab_id)
        };
        let Some(tab_id) = tab_id else { return };
        if !proceed {
            if let Some(console) = self.shared.borrow_mut().consoles.get_mut(&tab_id) {
                console.pending = None;
            }
            return;
        }
        self.as_mut().dispatch_next(tab_id);
    }

    /// A source's execution history (text only — see `db_core::history`'s
    /// own doc comment), newest last.
    pub fn history(self: Pin<&mut Self>, source_id: &QString) -> QStringList {
        let config_dir = app_core::resolve_config_dir();
        let path = db_core::history::history_file(&config_dir, &source_id.to_string());
        db_core::history::load(&path)
            .unwrap_or_default()
            .into_iter()
            .map(|entry| QString::from(entry.statement.as_str()))
            .collect()
    }

    pub fn clear_history(self: Pin<&mut Self>, source_id: &QString) -> FfiResult {
        let config_dir = app_core::resolve_config_dir();
        let path = db_core::history::history_file(&config_dir, &source_id.to_string());
        match db_core::history::clear(&path) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }

    /// Which source id `path` defaults to, empty when unknown — a console
    /// file under `db_core::console::consoles_dir` names its own source
    /// this way; a plain project `.sql` file has no default yet (the
    /// console bar's own source combo is how a user picks one for those),
    /// so this only ever resolves the first case.
    /// ponytail: a project's `[database] file_sources` map (a `.sql` file
    /// opened directly from the tree) is not consulted here yet — that
    /// needs the shared `AppSession` for the project root, which this
    /// QObject does not hold; add it if "open a plain project .sql file
    /// and have it pre-select a source" becomes a real ask.
    pub fn source_for_path(self: Pin<&mut Self>, path: &QString) -> QString {
        let config_dir = app_core::resolve_config_dir();
        match db_core::console::source_of(std::path::Path::new(&path.to_string()), &config_dir) {
            Some(id) => QString::from(id.as_str()),
            None => QString::default(),
        }
    }

    /// Every configured data source's id/name, for the console bar's own
    /// source combo — connection state is `DatabaseService`'s dock's
    /// concern, not this one's, so every row reports `Disconnected` here
    /// regardless of whether the Database dock has it open.
    pub fn available_sources(self: Pin<&mut Self>) -> Vec<ffi::FfiDbSourceRow> {
        super::service::configured_sources()
            .into_iter()
            .map(|setting| ffi::FfiDbSourceRow {
                id: QString::from(setting.id.as_str()),
                name: QString::from(setting.name.as_str()),
                driver: QString::from(setting.driver.as_str()),
                color: QString::from(setting.color.as_str()),
                group: QString::from(setting.group.as_str()),
                state: ffi::FfiDbConnectionState::Disconnected,
                message: QString::default(),
            })
            .collect()
    }
}

/// The parts of `ConsoleService`'s slots too large to sit inline above.
impl ffi::ConsoleService {
    fn send_tx(self: Pin<&mut Self>, tab_id: u64, command: SessionCommand) -> FfiResult {
        let shared = self.shared.borrow();
        let Some(console) = shared.consoles.get(&tab_id) else {
            return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "no such console");
        };
        match console.worker.send(command) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }

    /// Starts a fresh multi-statement run: refuses the whole batch up
    /// front if any statement fails the read-only guard (a console cannot
    /// be hidden the way the data editor is — `db_core::readonly`'s own
    /// doc comment — so this is the one place that check actually runs),
    /// logs the batch to history, then dispatches the first statement.
    fn begin_run(mut self: Pin<&mut Self>, tab_id: u64, statements: Vec<String>) -> FfiResult {
        let (history, source_id) = {
            let mut shared = self.shared.borrow_mut();
            let Some(console) = shared.consoles.get_mut(&tab_id) else {
                return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "no such console");
            };
            for statement in &statements {
                if let Err(error) = console.guard.check(statement) {
                    return errors::failure(errors::CODE_REFUSED, error.message);
                }
            }
            console.pending = Some(PendingScript {
                remaining: statements.iter().cloned().collect(),
                index: 0,
                total: statements.len() as u32,
                policy: console.script_policy,
                awaiting_resume: false,
            });
            (console.history, console.source_id.clone())
        };
        if history {
            let config_dir = app_core::resolve_config_dir();
            let path = db_core::history::history_file(&config_dir, &source_id);
            let cap = app_settings().history_cap_or_default();
            for statement in &statements {
                let _ = db_core::history::append(
                    &path,
                    &db_core::history::HistoryEntry {
                        timestamp: chrono::Utc::now(),
                        statement: statement.clone(),
                    },
                    cap,
                );
            }
        }
        self.as_mut().dispatch_next(tab_id);
        FfiResult::default()
    }

    /// Pops the next pending statement (if any) and sends it to the
    /// console's worker, allocating a fresh result id and announcing it
    /// through `executionStarted`.
    fn dispatch_next(mut self: Pin<&mut Self>, tab_id: u64) {
        let dispatch = {
            let mut shared = self.shared.borrow_mut();
            let Some(console) = shared.consoles.get_mut(&tab_id) else {
                return;
            };
            let Some(pending) = console.pending.as_mut() else {
                return;
            };
            pending.awaiting_resume = false;
            let Some(statement) = pending.remaining.pop_front() else {
                console.pending = None;
                return;
            };
            pending.index += 1;
            let result_id = shared.alloc_result_id();
            let (index, total) = {
                let pending = shared.consoles[&tab_id].pending.as_ref().unwrap();
                (pending.index, pending.total)
            };
            let page_size = shared.consoles[&tab_id].page_size;
            let dialect = shared.consoles[&tab_id].dialect;
            shared.consoles.get_mut(&tab_id).unwrap().current_result = Some(result_id);
            shared.results.insert(
                result_id,
                ResultState {
                    tab_id,
                    columns: Vec::new(),
                    set: ResultSet::new(),
                    done: false,
                    reported: false,
                    cap_reached: false,
                    affected: None,
                    error: None,
                    started_at: Instant::now(),
                    elapsed_ms: None,
                    statement_text: statement.clone(),
                    edit: EditState::Unknown,
                    table_constraints: Vec::new(),
                },
            );
            let worker_send = shared.consoles[&tab_id]
                .worker
                .send(SessionCommand::Execute {
                    statement: DbStatement::for_dialect(dialect, statement),
                    options: ExecOptions {
                        fetch_size: page_size,
                        max_rows: None,
                        ..ExecOptions::default()
                    },
                });
            Some((result_id, index, total, worker_send))
        };
        let Some((result_id, index, total, worker_send)) = dispatch else {
            return;
        };
        self.as_mut()
            .execution_started(tab_id, result_id, index, total);
        if let Err(error) = worker_send {
            self.as_mut().finish_result(result_id, Err(error));
        }
    }

    /// A non-`Rows` statement's whole outcome: marks the result fully
    /// `done` (there is nothing further to page — see [`Self::
    /// finish_result`]'s doc comment on the distinction `Rows` needs) and
    /// reports/advances exactly like it.
    fn finish_result(mut self: Pin<&mut Self>, result_id: u64, outcome: Result<u64, DbError>) {
        if let Some(result) = self.shared.borrow_mut().results.get_mut(&result_id) {
            result.done = true;
        }
        self.as_mut().report_and_advance(result_id, outcome);
    }

    /// Emits `executionFinished` and advances the console's pending
    /// script per its policy — called exactly once per result, the
    /// moment its *first* batch/outcome arrives. For a `Rows` shape that
    /// is deliberately not the same moment as "every row has been
    /// fetched": the statement itself has already succeeded or failed by
    /// the time the first page is in hand (`db_core::driver`'s own
    /// "blocking" doc comment — `execute` does not return until then), so
    /// a script's next statement (or this result's own `ok`/`elapsed`)
    /// must not wait on however many `ResultProvider::fetchMore` calls a
    /// user makes afterwards.
    fn report_and_advance(mut self: Pin<&mut Self>, result_id: u64, outcome: Result<u64, DbError>) {
        let already_reported = {
            let mut shared = self.shared.borrow_mut();
            let Some(result) = shared.results.get_mut(&result_id) else {
                return;
            };
            let was_reported = result.reported;
            result.reported = true;
            was_reported
        };
        if already_reported {
            return;
        }
        let (tab_id, affected, elapsed_ms, ffi_error, ok, error_text) = {
            let mut shared = self.shared.borrow_mut();
            let Some(result) = shared.results.get_mut(&result_id) else {
                return;
            };
            let elapsed_ms = result.started_at.elapsed().as_millis() as u64;
            result.elapsed_ms = Some(elapsed_ms);
            let (affected, ffi_error, ok, error_text) = match &outcome {
                Ok(n) => {
                    result.affected = Some(*n);
                    (*n, ok_error(), true, String::new())
                }
                Err(error) => {
                    result.error = Some(error.clone());
                    (0, to_ffi_error(error), false, error.message.clone())
                }
            };
            (
                result.tab_id,
                affected,
                elapsed_ms,
                ffi_error,
                ok,
                error_text,
            )
        };
        self.as_mut()
            .execution_finished(result_id, ok, affected, elapsed_ms, ffi_error);
        let mut shared = self.shared.borrow_mut();
        let Some(console) = shared.consoles.get_mut(&tab_id) else {
            return;
        };
        let Some(pending) = console.pending.as_mut() else {
            return;
        };
        let more_left = !pending.remaining.is_empty();
        let stop = !ok
            && matches!(
                pending.policy,
                FfiDbScriptPolicy::StopOnError | FfiDbScriptPolicy::Ask
            );
        if !more_left || (stop && pending.policy == FfiDbScriptPolicy::StopOnError) {
            console.pending = None;
            return;
        }
        if stop {
            pending.awaiting_resume = true;
            drop(shared);
            self.as_mut()
                .ask_continue(result_id, QString::from(error_text.as_str()));
            return;
        }
        drop(shared);
        self.as_mut().dispatch_next(tab_id);
    }
}

/// Applies one `SessionEvent` from a console's own worker — always
/// queued onto the Qt thread already (see `ConsoleService::attach`'s own
/// closure).
/// `true` when `generation` is still the console's *current* generation —
/// a reply behind that (a detach/reconnect already bumped it, or the
/// console is gone entirely) is stale and must be dropped, not applied
/// (this module's own doc comment, and `sessions.rs`'s cancel-race guard
/// it reuses).
fn is_current(service: &Pin<&mut ffi::ConsoleService>, tab_id: u64, generation: u64) -> bool {
    service
        .shared
        .borrow()
        .consoles
        .get(&tab_id)
        .is_some_and(|console| console.worker.generation() == generation)
}

fn apply_event(mut service: Pin<&mut ffi::ConsoleService>, tab_id: u64, event: SessionEvent) {
    match event {
        SessionEvent::Batch {
            generation,
            outcome,
        } => {
            if is_current(&service, tab_id, generation) {
                apply_batch(service, tab_id, outcome);
            }
        }
        SessionEvent::TxChanged { generation, result } => {
            if !is_current(&service, tab_id, generation) {
                return;
            }
            if let Err(error) = result {
                service.as_mut().output_appended(
                    tab_id,
                    QString::from(format!("Transaction error: {error}").as_str()),
                );
            }
        }
        SessionEvent::Introspected { generation, result } => {
            if !is_current(&service, tab_id, generation) {
                return;
            }
            let purpose = {
                let mut shared = service.shared.borrow_mut();
                shared
                    .consoles
                    .get_mut(&tab_id)
                    .and_then(|console| console.pending_introspects.pop_front())
            };
            match purpose {
                Some(IntrospectPurpose::EditLookup(result_id, table)) => {
                    apply_edit_lookup(service, tab_id, result_id, table, result);
                }
                Some(IntrospectPurpose::SchemaPicker) | None => {
                    if let Ok(snapshot) = result {
                        let names = collect_schema_names(&snapshot.roots);
                        if let Some(console) = service.shared.borrow_mut().consoles.get_mut(&tab_id)
                        {
                            console.schemas = names;
                        }
                    }
                }
            }
        }
        SessionEvent::Applied { generation, result } => {
            if is_current(&service, tab_id, generation) {
                apply_submit_outcome(service, tab_id, result);
            }
        }
        SessionEvent::Ddl { .. } | SessionEvent::Ran { .. } => {}
    }
}

fn apply_batch(mut service: Pin<&mut ffi::ConsoleService>, tab_id: u64, outcome: BatchOutcome) {
    let result_id = {
        let shared = service.shared.borrow();
        match shared.consoles.get(&tab_id).and_then(|c| c.current_result) {
            Some(id) => id,
            None => return,
        }
    };
    match outcome {
        BatchOutcome::Rows {
            columns,
            rows,
            done,
        } => {
            let (first, count, cap_reached) = {
                let mut shared = service.shared.borrow_mut();
                let Some(result) = shared.results.get_mut(&result_id) else {
                    return;
                };
                if result.columns.is_empty() && !columns.is_empty() {
                    result.columns = columns;
                }
                let first = result.set.row_count() as u64;
                let count = rows.len() as u64;
                let cap_reached = if !rows.is_empty() {
                    let batch = db_core::value::RowBatch {
                        columns: result.columns.clone(),
                        rows,
                    };
                    let cap = MemoryCap::from_mib(app_settings().memory_cap_mib_or_default());
                    result.set.append_batch(batch, cap).is_err()
                } else {
                    false
                };
                if cap_reached {
                    result.cap_reached = true;
                }
                if done {
                    result.done = true;
                }
                (first, count, cap_reached)
            };
            service.as_mut().start_editability_check(tab_id, result_id);
            if count > 0 {
                service.as_mut().rows_appended(result_id, first, count);
            }
            if cap_reached {
                service.as_mut().cap_reached_signal(result_id);
            }
            // The statement itself has already succeeded by the time any
            // batch (the first one, or a later `fetchMore` page) arrives —
            // `report_and_advance`'s own doc comment on why this fires
            // once, right here, rather than waiting for `done`.
            service
                .as_mut()
                .report_and_advance(result_id, Ok(count + first));
        }
        BatchOutcome::Affected(n) => {
            service.as_mut().finish_result(result_id, Ok(n));
        }
        BatchOutcome::Ok => {
            service.as_mut().finish_result(result_id, Ok(0));
        }
        BatchOutcome::Error(error) => {
            service.as_mut().finish_result(result_id, Err(error));
        }
    }
}

// ---- ResultProvider ----

pub struct ResultProviderRust {
    pub(crate) shared: Rc<std::cell::RefCell<Shared>>,
    page_sizes: std::cell::RefCell<HashMap<u64, u32>>,
}

impl Default for ResultProviderRust {
    fn default() -> Self {
        Self {
            shared: crate::bridge::registry::shared_database_consoles(),
            page_sizes: std::cell::RefCell::default(),
        }
    }
}

fn row_to_ffi(row: &[Value], format: &db_core::value::FormatRules, flags: u8) -> FfiDbRow {
    let mut cells = String::new();
    let mut nulls = String::new();
    for (i, value) in row.iter().enumerate() {
        if i > 0 {
            cells.push(CELL_SEP);
            nulls.push(CELL_SEP);
        }
        cells.push_str(&value.display(format));
        nulls.push(if matches!(value, Value::Null) {
            '1'
        } else {
            '0'
        });
    }
    FfiDbRow {
        cells: QString::from(cells.as_str()),
        nulls: QString::from(nulls.as_str()),
        flags,
    }
}

impl ffi::ResultProvider {
    pub fn columns(self: Pin<&mut Self>, result_id: u64) -> Vec<FfiDbColumn> {
        let shared = self.shared.borrow();
        shared
            .results
            .get(&result_id)
            .map(|result| {
                result
                    .columns
                    .iter()
                    .map(|c| FfiDbColumn {
                        name: QString::from(c.name.as_str()),
                        type_name: QString::from(c.type_name.as_str()),
                        nullable: c.nullable,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn row_count(self: Pin<&mut Self>, result_id: u64) -> u64 {
        let shared = self.shared.borrow();
        shared
            .results
            .get(&result_id)
            .map(|result| {
                let fetched = result.set.row_count() as u64;
                let inserted = match &result.edit {
                    EditState::Editable(buffer) => buffer.inserted_row_count() as u64,
                    _ => 0,
                };
                fetched + inserted
            })
            .unwrap_or(0)
    }

    /// Reads `count` (clamped) rows starting at `first`, in FFI time
    /// bounded by the batches actually touched rather than the whole
    /// result — walking `result.set.batches()` in order and skipping
    /// whole untouched batches by their row count, never flattening the
    /// full result into one `Vec` first (the ≤16ms-per-page NFR,
    /// `database-tools.md` §8).
    /// ponytail: still a linear scan of the batches *before* the page
    /// (cheap — an integer add per batch, not a row copy), not a
    /// prefix-sum index; revisit if a profile ever shows this mattering
    /// at the batch counts a page near the far end of a huge result
    /// produces.
    pub fn row_page(self: Pin<&mut Self>, result_id: u64, first: u64, count: u64) -> Vec<FfiDbRow> {
        let shared = self.shared.borrow();
        let Some(result) = shared.results.get(&result_id) else {
            return Vec::new();
        };
        let format = db_core::value::FormatRules::default();
        let count = count.min(MAX_ROWS_PER_REQUEST);
        let fetched_count = result.set.row_count() as u64;
        let mut out = Vec::with_capacity(count as usize);
        let mut skipped = 0u64;
        let mut grid_row = first;
        if first < fetched_count {
            'batches: for batch in result.set.batches() {
                let batch_len = batch.rows.len() as u64;
                if skipped + batch_len <= first {
                    skipped += batch_len;
                    continue;
                }
                let start_in_batch = first.saturating_sub(skipped) as usize;
                for row in &batch.rows[start_in_batch..] {
                    if out.len() as u64 >= count {
                        break 'batches;
                    }
                    let flags = match &result.edit {
                        EditState::Editable(buffer) => buffer.row_flags(grid_row as usize),
                        _ => 0,
                    };
                    out.push(row_to_ffi(row, &format, flags));
                    grid_row += 1;
                }
                skipped += batch_len;
            }
        }
        // Rows staged past the fetched count (`addRow`/`cloneRow`) — read
        // straight from the buffer, one column at a time, since they were
        // never fetched from a driver at all.
        if let EditState::Editable(buffer) = &result.edit {
            let total = fetched_count + buffer.inserted_row_count() as u64;
            while (out.len() as u64) < count && grid_row < total {
                let values: Vec<Value> = result
                    .columns
                    .iter()
                    .map(|c| {
                        buffer
                            .current_value(grid_row as usize, &c.name)
                            .unwrap_or(Value::Null)
                    })
                    .collect();
                out.push(row_to_ffi(
                    &values,
                    &format,
                    buffer.row_flags(grid_row as usize),
                ));
                grid_row += 1;
            }
        }
        out
    }

    pub fn fetch_more(self: Pin<&mut Self>, result_id: u64) -> FfiResult {
        let shared = self.shared.borrow();
        let Some(result) = shared.results.get(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        if result.done {
            return FfiResult::default();
        }
        let Some(console) = shared.consoles.get(&result.tab_id) else {
            return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "console detached");
        };
        match console.worker.send(SessionCommand::FetchMore) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }

    /// `true` when a further `fetchMore` could return more rows — the
    /// grid's own `canFetchMore` (`ResultTableModel`'s doc comment).
    pub fn fetch_more_available(self: Pin<&mut Self>, result_id: u64) -> bool {
        let shared = self.shared.borrow();
        shared
            .results
            .get(&result_id)
            .is_some_and(|result| !result.done)
    }

    pub fn set_page_size(self: Pin<&mut Self>, result_id: u64, size: u32) {
        self.page_sizes.borrow_mut().insert(result_id, size.max(1));
    }

    pub fn text_view(
        self: Pin<&mut Self>,
        result_id: u64,
        format: ffi::FfiDbTextFormat,
    ) -> QString {
        let shared = self.shared.borrow();
        let Some(result) = shared.results.get(&result_id) else {
            return QString::default();
        };
        let rules = db_core::value::FormatRules::default();
        let rows: Vec<&Vec<Value>> = result
            .set
            .batches()
            .iter()
            .flat_map(|batch| batch.rows.iter())
            .collect();
        let text = match format {
            ffi::FfiDbTextFormat::Json => {
                let objects: Vec<String> = rows
                    .iter()
                    .map(|row| {
                        let fields: Vec<String> = result
                            .columns
                            .iter()
                            .zip(row.iter())
                            .map(|(col, value)| {
                                format!("{:?}:{:?}", col.name, value.display(&rules))
                            })
                            .collect();
                        format!("{{{}}}", fields.join(","))
                    })
                    .collect();
                format!("[{}]", objects.join(","))
            }
            ffi::FfiDbTextFormat::Tsv => render_delimited(&result.columns, &rows, &rules, '\t'),
            _ => render_delimited(&result.columns, &rows, &rules, ','),
        };
        QString::from(text.as_str())
    }

    /// A best-effort aggregate over the rows fetched *so far* — not the
    /// whole result if paging has not reached the end yet.
    /// ponytail: client-side over the current page cache rather than a
    /// re-executed `SELECT agg(col) FROM (...)`; upgrade once a console
    /// needs an exact aggregate over an unfetched tail.
    pub fn aggregate(
        self: Pin<&mut Self>,
        result_id: u64,
        column: &QString,
        op: ffi::FfiDbAggOp,
    ) -> QString {
        let shared = self.shared.borrow();
        let Some(result) = shared.results.get(&result_id) else {
            return QString::default();
        };
        let column = column.to_string();
        let Some(index) = result.columns.iter().position(|c| c.name == column) else {
            return QString::default();
        };
        let numbers: Vec<f64> = result
            .set
            .batches()
            .iter()
            .flat_map(|batch| batch.rows.iter())
            .filter_map(|row| match row.get(index) {
                Some(Value::Int(n)) => Some((*n) as f64),
                Some(Value::Float(n)) => Some(*n),
                Some(Value::Decimal(text)) => text.parse::<f64>().ok(),
                _ => None,
            })
            .collect();
        let text = match op {
            ffi::FfiDbAggOp::Sum => numbers.iter().sum::<f64>().to_string(),
            ffi::FfiDbAggOp::Avg => {
                if numbers.is_empty() {
                    "0".to_string()
                } else {
                    (numbers.iter().sum::<f64>() / numbers.len() as f64).to_string()
                }
            }
            ffi::FfiDbAggOp::Min => numbers
                .iter()
                .cloned()
                .fold(f64::INFINITY, f64::min)
                .to_string(),
            ffi::FfiDbAggOp::Max => numbers
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max)
                .to_string(),
            _ => result.set.row_count().to_string(),
        };
        QString::from(text.as_str())
    }

    /// Re-executes `resultId`'s original statement with `WHERE`/`ORDER BY`
    /// applied through `db_sql::apply_clauses` — a plain `SELECT` gets its
    /// own clauses extended in place; anything else falls back to that
    /// module's derived-table wrapper (its own doc comment says which is
    /// which), on the same console worker that ran the original statement.
    /// `ResultProvider` cannot call `ConsoleService::execute` directly
    /// (they are two separate QObjects — see this module's own doc
    /// comment on why `Shared` exists), so this sends the `Execute`
    /// command straight to the worker; the worker's replies still land on
    /// `ConsoleService::apply_event` regardless of who sent the command
    /// (its `on_event` closure was bound to that QObject once, at
    /// `attach` time), so `rowsAppended`/`executionFinished` still fire
    /// exactly as they do for a plain "Run".
    /// ponytail: does not validate `where_clause`/`order_by` beyond what
    /// the database itself rejects; the read-only guard still runs below,
    /// so this cannot smuggle a write.
    pub fn apply_clauses(
        self: Pin<&mut Self>,
        result_id: u64,
        where_clause: &QString,
        order_by: &QString,
    ) -> FfiResult {
        let where_clause = where_clause.to_string();
        let order_by = order_by.to_string();
        let mut shared = self.shared.borrow_mut();
        let Some(existing) = shared.results.get(&result_id) else {
            return errors::failure(errors::CODE_UNKNOWN_RESULT, "no such result");
        };
        let tab_id = existing.tab_id;
        let Some(console) = shared.consoles.get(&tab_id) else {
            return errors::failure(errors::CODE_UNKNOWN_DB_CONSOLE, "console detached");
        };
        let statement = db_sql::apply_clauses(
            &existing.statement_text,
            &where_clause,
            &order_by,
            console.dialect,
        );
        if let Err(error) = console.guard.check(&statement) {
            return errors::failure(errors::CODE_REFUSED, error.message);
        }
        let page_size = console.page_size;
        let dialect = console.dialect;
        let new_id = shared.alloc_result_id();
        shared.results.insert(
            new_id,
            ResultState {
                tab_id,
                columns: Vec::new(),
                set: ResultSet::new(),
                done: false,
                reported: false,
                cap_reached: false,
                affected: None,
                error: None,
                started_at: Instant::now(),
                elapsed_ms: None,
                statement_text: statement.clone(),
                edit: EditState::Unknown,
                table_constraints: Vec::new(),
            },
        );
        shared.consoles.get_mut(&tab_id).unwrap().current_result = Some(new_id);
        match shared.consoles[&tab_id]
            .worker
            .send(SessionCommand::Execute {
                statement: DbStatement::for_dialect(dialect, statement),
                options: ExecOptions {
                    fetch_size: page_size,
                    max_rows: None,
                    ..ExecOptions::default()
                },
            }) {
            // The new result id, in `message` — `ResultProvider` has no
            // `executionStarted` signal of its own to announce it with
            // (this module's own doc comment on why not), so the grid
            // view reads it from here instead.
            // ponytail: a string-encoded id in an otherwise-human-message
            // field is a crutch; upgrade to a small `{code, message,
            // resultId}` struct if `ResultProvider` ever needs to return
            // one from more than this one call.
            Ok(()) => FfiResult {
                code: 0,
                message: QString::from(new_id.to_string().as_str()),
            },
            Err(error) => errors::failure(errors::CODE_REFUSED, error.to_string()),
        }
    }
}

fn render_delimited(
    columns: &[ColumnMeta],
    rows: &[&Vec<Value>],
    rules: &db_core::value::FormatRules,
    sep: char,
) -> String {
    let mut out = String::new();
    out.push_str(
        &columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(&sep.to_string()),
    );
    for row in rows {
        out.push('\n');
        let cells: Vec<String> = row.iter().map(|v| v.display(rules)).collect();
        out.push_str(&cells.join(&sep.to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_statements_drops_blank_ones_between_semicolons() {
        let statements = split_statements("SELECT 1; ;  SELECT 2;", Dialect::Sqlite);
        assert_eq!(statements, vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn split_statements_is_quote_aware() {
        let statements = split_statements("SELECT ';'; SELECT 2", Dialect::Sqlite);
        assert_eq!(statements, vec!["SELECT ';'", "SELECT 2"]);
    }

    #[test]
    fn statement_at_caret_picks_the_statement_the_caret_sits_inside() {
        let text = "SELECT 1;\nSELECT 2;\nSELECT 3;";
        // Caret inside the second statement's text.
        let caret = text.find("SELECT 2").unwrap() + 3;
        assert_eq!(
            statement_at_caret(text, Dialect::Sqlite, caret),
            Some("SELECT 2".to_string())
        );
    }

    #[test]
    fn statement_at_caret_falls_back_to_the_last_statement_past_every_span() {
        let text = "SELECT 1;\nSELECT 2;";
        assert_eq!(
            statement_at_caret(text, Dialect::Sqlite, text.len() + 5),
            Some("SELECT 2".to_string())
        );
    }

    #[test]
    fn statement_at_caret_on_blank_text_is_none() {
        assert_eq!(statement_at_caret("   ", Dialect::Sqlite, 0), None);
    }
}
