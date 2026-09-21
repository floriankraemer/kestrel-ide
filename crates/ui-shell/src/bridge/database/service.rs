//! `DatabaseService` (database-tools-plan F2.5): the Database dock's
//! adapter. Translation only — one [`super::sessions::SessionWorker`] per
//! connected data source, `db_core::tree::flatten` re-run whenever
//! `rows()` is asked for. Every rule (introspection scope/level, grouping,
//! filters, the action matrix, DDL/SQL generation) lives in `db_core`;
//! this file only tracks which source is connected, what schema its
//! worker has delivered so far, and which of that schema an in-flight
//! request will land in once its reply arrives.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::rc::Rc;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use app_config::database::DataSourceSetting;
use db_core::datasource::{ConnectSpec, DataSource, Secrets};
use db_core::ddl;
use db_core::dialect::Dialect;
use db_core::driver::Statement;
use db_core::error::DbError;
use db_core::schema::{Children, IntrospectLevel, IntrospectScope, Node, ObjectKind, ObjectRef};
use db_core::session::Session;
use db_core::tree::{
    self, FlattenOptions, GroupMode, ObjectTypeFilter, PatternFilter, RowKind, SortOrder,
    SourceCapabilities, TreeRow,
};

use crate::bridge::errors;
use crate::bridge::ffi::{
    self, FfiDbConnectionState, FfiDbRowActions, FfiDbSourceRow, FfiDbTreeRow, FfiResult,
};

use super::sessions::{SessionCommand, SessionEvent, SessionWorker};
use super::tree::{parse_node_id, to_ffi_row};

/// `secret-store`'s service name for data-source credentials — the exact
/// constant `bridge::database::settings` already uses (ADR-0061 §1);
/// duplicated here rather than made `pub(crate)` there, since `settings.rs`
/// is F1's own file and this module reads no other private item of it.
const SECRET_SERVICE: &str = "ide.database";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

fn to_ffi_state(state: ConnState) -> FfiDbConnectionState {
    match state {
        ConnState::Disconnected => FfiDbConnectionState::Disconnected,
        ConnState::Connecting => FfiDbConnectionState::Connecting,
        ConnState::Connected => FfiDbConnectionState::Connected,
        ConnState::Error => FfiDbConnectionState::Error,
    }
}

/// What an in-flight request will do with its reply once it arrives —
/// pushed the instant a command is sent, popped in the same order once
/// its `SessionEvent` comes back (both sides of one worker's single mpsc
/// channel are strictly FIFO, so this never needs its own correlation
/// id).
enum Pending {
    /// The initial (or force-refreshed) root snapshot: replace the whole
    /// tree.
    Root,
    /// An `expand`: merge the reply into this exact object path.
    Node(Vec<String>),
    /// A `goToDdl`: the virtual-document key/title to open the text
    /// under.
    Ddl { key: String, title: String },
    /// A `runAction`: nothing further to do beyond reporting
    /// `actionFinished`.
    Run,
}

struct SourceState {
    read_only: bool,
    state: ConnState,
    message: String,
    roots: Vec<Node>,
    worker: Option<SessionWorker>,
    dialect: Option<Dialect>,
    pending: VecDeque<Pending>,
}

impl SourceState {
    fn caps(&self) -> SourceCapabilities {
        SourceCapabilities {
            read_only: self.read_only,
            supports_comment: self.dialect.map(ddl::supports_comment).unwrap_or(true),
        }
    }
}

pub struct DatabaseServiceRust {
    sources: RefCell<BTreeMap<String, SourceState>>,
    filter: RefCell<String>,
    flat: RefCell<bool>,
    session: Rc<RefCell<app_core::AppSession>>,
}

impl Default for DatabaseServiceRust {
    fn default() -> Self {
        Self {
            sources: RefCell::default(),
            filter: RefCell::default(),
            flat: RefCell::default(),
            session: crate::bridge::registry::shared_session(),
        }
    }
}

pub(super) fn configured_sources() -> Vec<DataSourceSetting> {
    let global = crate::bridge::convert::load_settings();
    let project = crate::bridge::convert::load_project_settings();
    settings_model::scope::resolve_database_sources(&global, &project)
        .into_iter()
        .map(|(setting, _)| setting)
        .collect()
}

pub(super) fn secrets_for(id: &str) -> Secrets {
    let store = secret_store::SecretStore::new(SECRET_SERVICE);
    Secrets {
        password: store.load(id).ok().flatten(),
        ..Default::default()
    }
}

/// Replace the node at `path` (a real object-name ancestry,
/// `TreeRow::object_path`) with `children` — the merge [`Pending::Node`]
/// applies once an `expand`'s reply arrives. `false` when `path` no
/// longer exists (the schema changed underneath it, e.g. a concurrent
/// force refresh) — the caller drops the reply silently rather than
/// panicking on a shape mismatch.
fn replace_node_children(roots: &mut [Node], path: &[String], children: Children) -> bool {
    let Some((first, rest)) = path.split_first() else {
        return false;
    };
    for node in roots.iter_mut() {
        if &node.name == first {
            if rest.is_empty() {
                node.children = children;
                return true;
            }
            if let Children::Loaded(inner) = &mut node.children {
                return replace_node_children(inner, rest, children);
            }
            return false;
        }
    }
    false
}

/// The `IntrospectScope`/`IntrospectLevel` an `expand` of this row asks
/// for: a schema-like row lists its children's *names* only (cheap — the
/// NFR's fast path), a table/view/collection-like row fetches its own
/// *columns*. `object_path`'s last two segments become `(schema, object)`
/// when there are at least two — one segment alone is the whole scope
/// (SQLite has no schema level).
fn scope_for_expand(row: &TreeRow) -> (IntrospectScope, IntrospectLevel) {
    let path = &row.object_path;
    let is_schema_like = matches!(
        row.kind,
        RowKind::Object(ObjectKind::Schema | ObjectKind::Catalog | ObjectKind::Keyspace)
    );
    if is_schema_like {
        let scope = IntrospectScope {
            schema: path.last().cloned(),
            ..Default::default()
        };
        return (scope, IntrospectLevel::Names);
    }
    let object = path.last().cloned();
    let schema = if path.len() >= 2 {
        Some(path[path.len() - 2].clone())
    } else {
        None
    };
    (
        IntrospectScope {
            schema,
            object,
            ..Default::default()
        },
        IntrospectLevel::Columns,
    )
}

/// The `ObjectRef` a Go-to-DDL/rename/drop/truncate/comment action names,
/// from a row's own kind and ancestry.
fn object_ref_for(row: &TreeRow) -> Option<ObjectRef> {
    let RowKind::Object(kind) = row.kind else {
        return None;
    };
    let name = row.object_path.last()?.clone();
    let schema = if row.object_path.len() >= 2 {
        Some(row.object_path[row.object_path.len() - 2].clone())
    } else {
        None
    };
    let mut object_ref = ObjectRef::new(name).with_kind(kind);
    if let Some(schema) = schema {
        object_ref = object_ref.with_schema(schema);
    }
    Some(object_ref)
}

fn qualified_name(row: &TreeRow) -> String {
    row.object_path.join(".")
}

impl ffi::DatabaseService {
    pub fn sources(&self) -> Vec<FfiDbSourceRow> {
        let states = self.sources.borrow();
        configured_sources()
            .into_iter()
            .map(|setting| {
                let (state, message) = states
                    .get(&setting.id)
                    .map(|s| (s.state, s.message.clone()))
                    .unwrap_or((ConnState::Disconnected, String::new()));
                FfiDbSourceRow {
                    id: QString::from(setting.id.as_str()),
                    name: QString::from(setting.name.as_str()),
                    driver: QString::from(setting.driver.as_str()),
                    color: QString::from(setting.color.as_str()),
                    group: QString::from(setting.group.as_str()),
                    state: to_ffi_state(state),
                    message: QString::from(message.as_str()),
                }
            })
            .collect()
    }

    fn flatten_for(&self, source_id: &str) -> Vec<TreeRow> {
        let sources = self.sources.borrow();
        let Some(source) = sources.get(source_id) else {
            return Vec::new();
        };
        let options = FlattenOptions {
            group_mode: if *self.flat.borrow() {
                GroupMode::Flat
            } else {
                GroupMode::ByObjectType
            },
            separate_routines: false,
            object_types: ObjectTypeFilter::all(),
            pattern: PatternFilter::new(self.filter.borrow().clone()),
            sort: SortOrder::Natural,
            caps: source.caps(),
        };
        tree::flatten(&source.roots, &options)
    }

    pub fn rows(&self) -> Vec<FfiDbTreeRow> {
        let mut out = Vec::new();
        for setting in configured_sources() {
            let (state, message) = self
                .sources
                .borrow()
                .get(&setting.id)
                .map(|s| (Some(s.state), s.message.clone()))
                .unwrap_or((None, String::new()));
            out.push(FfiDbTreeRow {
                source_id: QString::from(setting.id.as_str()),
                node_id: QString::from(format!("{}:$root", setting.id)),
                depth: 0,
                kind: QString::from("source"),
                label: QString::from(setting.name.as_str()),
                detail: QString::from(message.as_str()),
                expandable: true,
                loaded: true,
                actions: FfiDbRowActions {
                    can_refresh: true,
                    ..Default::default()
                },
            });
            if state != Some(ConnState::Connected) {
                continue;
            }
            for row in self.flatten_for(&setting.id) {
                let mut ffi_row = to_ffi_row(&setting.id, &row);
                ffi_row.depth += 1;
                out.push(ffi_row);
            }
        }
        out
    }

    /// Connects off the Qt thread (the connect itself may be a slow
    /// network round trip — the same "always run off the UI thread"
    /// rule `DataSourceEditor::test_connection`'s doc comment states):
    /// this call only marks the source `Connecting` and returns; the
    /// spawned thread reports the outcome back through one `qt_thread()
    /// .queue` closure that builds the `SessionWorker` itself (spawning a
    /// worker thread is cheap, so it happens on the Qt thread rather than
    /// needing a second cross-thread hop).
    pub fn connect_source(mut self: Pin<&mut Self>, id: &QString) -> FfiResult {
        let id = id.to_string();
        let Some(setting) = configured_sources().into_iter().find(|s| s.id == id) else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                format!("no data source with id '{id}' is configured"),
            );
        };
        if self
            .sources
            .borrow()
            .get(&id)
            .is_some_and(|s| s.worker.is_some())
        {
            return errors::failure(
                errors::CODE_REFUSED,
                format!("'{}' is already connected", setting.name),
            );
        }

        self.sources.borrow_mut().insert(
            id.clone(),
            SourceState {
                read_only: false,
                state: ConnState::Connecting,
                message: String::new(),
                roots: Vec::new(),
                worker: None,
                dialect: None,
                pending: VecDeque::new(),
            },
        );
        self.as_mut().connection_state_changed(
            QString::from(id.as_str()),
            FfiDbConnectionState::Connecting,
            QString::default(),
        );
        self.as_mut().rows_changed();

        let qt_thread = self.as_mut().qt_thread();
        let thread_id = id.clone();
        std::thread::spawn(move || {
            let data_source = DataSource::from(&setting);
            let read_only = data_source.read_only;
            let spec = ConnectSpec::from(&data_source, &secrets_for(&thread_id));
            let outcome = super::connect(&spec);
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DatabaseService>| {
                match outcome {
                    Ok(connection) => {
                        let dialect = connection.dialect();
                        let session = Session::new(connection);
                        let inner_qt_thread = service.as_mut().qt_thread();
                        let worker_source_id = thread_id.clone();
                        let worker = SessionWorker::spawn(session, move |event| {
                            let source_id = worker_source_id.clone();
                            let _ = inner_qt_thread.queue(
                                move |service: Pin<&mut ffi::DatabaseService>| {
                                    apply_event(service, source_id, event);
                                },
                            );
                        });
                        let _ = worker.send(SessionCommand::Introspect {
                            scope: IntrospectScope::default(),
                            level: IntrospectLevel::Names,
                            force: false,
                        });
                        if let Some(source) = service.sources.borrow_mut().get_mut(&thread_id) {
                            source.read_only = read_only;
                            source.dialect = Some(dialect);
                            source.state = ConnState::Connected;
                            source.worker = Some(worker);
                            source.pending.push_back(Pending::Root);
                        }
                        service.as_mut().connection_state_changed(
                            QString::from(thread_id.as_str()),
                            FfiDbConnectionState::Connected,
                            QString::default(),
                        );
                    }
                    Err(message) => {
                        if let Some(source) = service.sources.borrow_mut().get_mut(&thread_id) {
                            source.state = ConnState::Error;
                            source.message = message.clone();
                        }
                        service.as_mut().connection_state_changed(
                            QString::from(thread_id.as_str()),
                            FfiDbConnectionState::Error,
                            QString::from(message.as_str()),
                        );
                    }
                }
                service.as_mut().rows_changed();
            });
        });
        FfiResult::default()
    }

    pub fn disconnect_source(mut self: Pin<&mut Self>, id: &QString) -> FfiResult {
        let id = id.to_string();
        let existed = self.sources.borrow_mut().remove(&id).is_some();
        if !existed {
            return errors::failure(errors::CODE_REFUSED, format!("'{id}' is not connected"));
        }
        self.as_mut().connection_state_changed(
            QString::from(id.as_str()),
            FfiDbConnectionState::Disconnected,
            QString::default(),
        );
        self.as_mut().rows_changed();
        FfiResult::default()
    }

    pub fn expand(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        let composite = node_id.to_string();
        let Some((source_id, path)) = parse_node_id(&composite) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "malformed node id");
        };
        if path == "$root" {
            return FfiResult::default();
        }
        let rows = self.flatten_for(source_id);
        let Some(row) = rows.iter().find(|r| r.node_id == path) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such node");
        };
        if row.loaded || !row.expandable {
            return FfiResult::default();
        }
        let (scope, level) = scope_for_expand(row);
        let object_path = row.object_path.clone();
        let source_id = source_id.to_string();
        let mut sources = self.sources.borrow_mut();
        let Some(source) = sources.get_mut(&source_id) else {
            return errors::failure(
                errors::CODE_REFUSED,
                format!("'{source_id}' is not connected"),
            );
        };
        let Some(worker) = source.worker.as_ref() else {
            return errors::failure(
                errors::CODE_REFUSED,
                format!("'{source_id}' is not connected"),
            );
        };
        if let Err(error) = worker.send(SessionCommand::Introspect {
            scope,
            level,
            force: false,
        }) {
            return errors::failure(errors::CODE_REFUSED, error.to_string());
        }
        source.pending.push_back(Pending::Node(object_path));
        FfiResult::default()
    }

    pub fn refresh(mut self: Pin<&mut Self>, node_id: &QString, force: bool) -> FfiResult {
        let composite = node_id.to_string();
        let Some((source_id, path)) = parse_node_id(&composite) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "malformed node id");
        };
        let source_id = source_id.to_string();
        if force {
            let mut sources = self.sources.borrow_mut();
            let Some(source) = sources.get_mut(&source_id) else {
                return errors::failure(
                    errors::CODE_REFUSED,
                    format!("'{source_id}' is not connected"),
                );
            };
            let Some(worker) = source.worker.as_ref() else {
                return errors::failure(
                    errors::CODE_REFUSED,
                    format!("'{source_id}' is not connected"),
                );
            };
            // Drop every reply already in flight — a force refresh means
            // "whatever this source answers next is authoritative", not
            // "queue behind whatever it was already about to say".
            worker.invalidate();
            source.pending.clear();
            if let Err(error) = worker.send(SessionCommand::Introspect {
                scope: IntrospectScope::default(),
                level: IntrospectLevel::Names,
                force: true,
            }) {
                return errors::failure(errors::CODE_REFUSED, error.to_string());
            }
            source.pending.push_back(Pending::Root);
            return FfiResult::default();
        }
        if path == "$root" {
            return self.as_mut().refresh(node_id, true);
        }
        let rows = self.flatten_for(&source_id);
        let Some(row) = rows.iter().find(|r| r.node_id == path) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such node");
        };
        let (scope, level) = scope_for_expand(row);
        let object_path = row.object_path.clone();
        let mut sources = self.sources.borrow_mut();
        let Some(source) = sources.get_mut(&source_id) else {
            return errors::failure(
                errors::CODE_REFUSED,
                format!("'{source_id}' is not connected"),
            );
        };
        let Some(worker) = source.worker.as_ref() else {
            return errors::failure(
                errors::CODE_REFUSED,
                format!("'{source_id}' is not connected"),
            );
        };
        if let Err(error) = worker.send(SessionCommand::DropCached {
            scope: scope.clone(),
        }) {
            return errors::failure(errors::CODE_REFUSED, error.to_string());
        }
        if let Err(error) = worker.send(SessionCommand::Introspect {
            scope,
            level,
            force: false,
        }) {
            return errors::failure(errors::CODE_REFUSED, error.to_string());
        }
        source.pending.push_back(Pending::Node(object_path));
        FfiResult::default()
    }

    pub fn set_filter(mut self: Pin<&mut Self>, text: &QString) {
        *self.filter.borrow_mut() = text.to_string();
        self.as_mut().rows_changed();
    }

    pub fn set_grouping(mut self: Pin<&mut Self>, flat: bool) {
        *self.flat.borrow_mut() = flat;
        self.as_mut().rows_changed();
    }

    pub fn run_action(self: Pin<&mut Self>, node_id: &QString, action_id: &QString) -> FfiResult {
        let composite = node_id.to_string();
        let action_id = action_id.to_string();
        let Some((source_id, path)) = parse_node_id(&composite) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "malformed node id");
        };
        let source_id = source_id.to_string();
        let rows = self.flatten_for(&source_id);
        let Some(row) = rows.iter().find(|r| r.node_id == path) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such node");
        };
        let Some(object_ref) = object_ref_for(row) else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                "this row has no runnable action",
            );
        };
        let dialect = {
            let sources = self.sources.borrow();
            let Some(source) = sources.get(&source_id) else {
                return errors::failure(
                    errors::CODE_REFUSED,
                    format!("'{source_id}' is not connected"),
                );
            };
            let Some(dialect) = source.dialect else {
                return errors::failure(
                    errors::CODE_REFUSED,
                    format!("'{source_id}' is not connected"),
                );
            };
            dialect
        };
        let generated = match action_id.split_once(':') {
            Some(("rename", new_name)) => ddl::rename_statement(dialect, &object_ref, new_name),
            Some(("comment", text)) => ddl::comment_statement(dialect, &object_ref, text),
            _ => match action_id.as_str() {
                "drop" => ddl::drop_statement(dialect, &object_ref),
                "truncate" => ddl::truncate_statement(dialect, &object_ref),
                _ => Err(DbError::new(
                    db_core::error::DbErrorCode::NotSupported,
                    format!("unknown action '{action_id}'"),
                )),
            },
        };
        let statement_text = match generated {
            Ok(text) => text,
            Err(error) => return errors::failure(errors::CODE_INVALID_ARGUMENT, error.to_string()),
        };
        let mut sources = self.sources.borrow_mut();
        let Some(source) = sources.get_mut(&source_id) else {
            return errors::failure(
                errors::CODE_REFUSED,
                format!("'{source_id}' is not connected"),
            );
        };
        let Some(worker) = source.worker.as_ref() else {
            return errors::failure(
                errors::CODE_REFUSED,
                format!("'{source_id}' is not connected"),
            );
        };
        if let Err(error) = worker.send(SessionCommand::RunStatement {
            statement: Statement::sql(statement_text),
        }) {
            return errors::failure(errors::CODE_REFUSED, error.to_string());
        }
        source.pending.push_back(Pending::Run);
        FfiResult::default()
    }

    pub fn go_to_ddl(self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        let composite = node_id.to_string();
        let Some((source_id, path)) = parse_node_id(&composite) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "malformed node id");
        };
        let source_id = source_id.to_string();
        let rows = self.flatten_for(&source_id);
        let Some(row) = rows.iter().find(|r| r.node_id == path) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such node");
        };
        let Some(object_ref) = object_ref_for(row) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "this row has no DDL");
        };
        let key = format!("{source_id}/{}.sql", qualified_name(row));
        let title = format!("{} DDL", row.label);
        let mut sources = self.sources.borrow_mut();
        let Some(source) = sources.get_mut(&source_id) else {
            return errors::failure(
                errors::CODE_REFUSED,
                format!("'{source_id}' is not connected"),
            );
        };
        let Some(worker) = source.worker.as_ref() else {
            return errors::failure(
                errors::CODE_REFUSED,
                format!("'{source_id}' is not connected"),
            );
        };
        if let Err(error) = worker.send(SessionCommand::DdlOf { object: object_ref }) {
            return errors::failure(errors::CODE_REFUSED, error.to_string());
        }
        source.pending.push_back(Pending::Ddl { key, title });
        FfiResult::default()
    }

    /// "Open Console"/"Jump to console" (F3.1/F3.3) — see this slot's own
    /// doc comment in `ffi.rs`.
    pub fn open_console(mut self: Pin<&mut Self>, node_id: &QString) -> FfiResult {
        let composite = node_id.to_string();
        let Some((source_id, _path)) = parse_node_id(&composite) else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "malformed node id");
        };
        let source_id = source_id.to_string();
        let config_dir = app_core::resolve_config_dir();
        let dir = db_core::console::console_dir(&config_dir, &source_id);
        if let Err(error) = std::fs::create_dir_all(&dir) {
            return errors::failure(errors::CODE_SETTINGS_IO, error.to_string());
        }
        let mut existing: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
                    .collect()
            })
            .unwrap_or_default();
        existing.sort();
        let path = match existing.into_iter().next() {
            Some(path) => path,
            None => {
                let path = db_core::console::console_file(&config_dir, &source_id, 1);
                if let Err(error) = std::fs::write(&path, "") {
                    return errors::failure(errors::CODE_SETTINGS_IO, error.to_string());
                }
                path
            }
        };
        let path_string = path.to_string_lossy().to_string();
        self.as_mut().console_file_ready(
            QString::from(path_string.as_str()),
            QString::from(source_id.as_str()),
        );
        FfiResult::default()
    }
}

/// Applies one `SessionEvent` for `source_id` — always runs on the Qt
/// thread (queued there by [`ffi::DatabaseService::connect_source`]'s own
/// `on_event` closure).
fn apply_event(
    mut service: Pin<&mut ffi::DatabaseService>,
    source_id: String,
    event: SessionEvent,
) {
    match event {
        SessionEvent::Introspected { generation, result } => {
            let outcome = {
                let mut sources = service.sources.borrow_mut();
                let Some(source) = sources.get_mut(&source_id) else {
                    return;
                };
                let current = source.worker.as_ref().map(|w| w.generation());
                if current != Some(generation) {
                    // Stale: a disconnect/force-refresh already invalidated
                    // whatever this reply answers.
                    return;
                }
                let pending = source.pending.pop_front();
                match result {
                    Ok(snapshot) => {
                        match pending {
                            Some(Pending::Node(path)) => {
                                if let Some(node) = snapshot.roots.into_iter().next() {
                                    replace_node_children(&mut source.roots, &path, node.children);
                                }
                            }
                            _ => source.roots = snapshot.roots,
                        }
                        source.state = ConnState::Connected;
                        source.message.clear();
                        Ok(())
                    }
                    Err(error) => {
                        source.state = ConnState::Error;
                        source.message = error.to_string();
                        Err(error)
                    }
                }
            };
            if let Err(error) = outcome {
                service.as_mut().connection_state_changed(
                    QString::from(source_id.as_str()),
                    FfiDbConnectionState::Error,
                    QString::from(error.to_string().as_str()),
                );
            }
            service.as_mut().rows_changed();
        }
        SessionEvent::Ddl { generation, result } => {
            let pending = {
                let mut sources = service.sources.borrow_mut();
                let Some(source) = sources.get_mut(&source_id) else {
                    return;
                };
                let current = source.worker.as_ref().map(|w| w.generation());
                if current != Some(generation) {
                    return;
                }
                source.pending.pop_front()
            };
            let Some(Pending::Ddl { key, title: _title }) = pending else {
                return;
            };
            match result {
                Ok(text) => {
                    let opened = service
                        .session
                        .borrow_mut()
                        .open_virtual_document("db-ddl", &key, &text);
                    service.as_mut().virtual_document_opened(
                        opened.id.raw(),
                        QString::from(opened.title.as_str()),
                        opened.newly_opened,
                    );
                }
                Err(error) => {
                    service
                        .as_mut()
                        .action_finished(false, QString::from(error.to_string().as_str()));
                }
            }
        }
        SessionEvent::Ran { generation, result } => {
            let is_current = {
                let mut sources = service.sources.borrow_mut();
                let Some(source) = sources.get_mut(&source_id) else {
                    return;
                };
                let current = source.worker.as_ref().map(|w| w.generation());
                let is_current = current == Some(generation);
                if is_current {
                    source.pending.pop_front();
                }
                is_current
            };
            if !is_current {
                return;
            }
            match result {
                Ok(()) => service.as_mut().action_finished(true, QString::default()),
                Err(error) => service
                    .as_mut()
                    .action_finished(false, QString::from(error.to_string().as_str())),
            }
        }
        // The tree's own worker never sends `Execute`/`FetchMore`/tx
        // commands — those are `ConsoleService`'s (F3.3), which runs each
        // console on its own `SessionWorker` rather than this one.
        SessionEvent::Batch { .. } | SessionEvent::TxChanged { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::tree::{flatten, FlattenOptions};

    #[test]
    fn replace_node_children_finds_a_nested_table_by_its_real_ancestry() {
        let table = Node::with_children(
            "users",
            ObjectKind::Table,
            vec![Node::leaf("id", ObjectKind::Column)],
        );
        let mut roots = vec![Node::with_children(
            "public",
            ObjectKind::Schema,
            vec![table],
        )];
        let new_children = Children::Loaded(vec![
            Node::leaf("id", ObjectKind::Column),
            Node::leaf("name", ObjectKind::Column),
        ]);
        let found = replace_node_children(
            &mut roots,
            &["public".to_string(), "users".to_string()],
            new_children,
        );
        assert!(found);
        let Children::Loaded(schema_children) = &roots[0].children else {
            panic!("expected loaded schema");
        };
        let Children::Loaded(table_children) = &schema_children[0].children else {
            panic!("expected loaded table");
        };
        assert_eq!(table_children.len(), 2);
    }

    #[test]
    fn replace_node_children_is_a_no_op_for_a_path_that_no_longer_exists() {
        let mut roots = vec![Node::leaf("users", ObjectKind::Table)];
        let found =
            replace_node_children(&mut roots, &["gone".to_string()], Children::Loaded(vec![]));
        assert!(!found);
    }

    #[test]
    fn scope_for_expand_uses_names_level_for_a_schema_like_row() {
        let node = Node::leaf("public", ObjectKind::Schema);
        let rows = flatten(&[node], &FlattenOptions::default());
        let row = rows.iter().find(|r| r.label == "public").unwrap();
        let (scope, level) = scope_for_expand(row);
        assert_eq!(level, IntrospectLevel::Names);
        assert_eq!(scope.schema.as_deref(), Some("public"));
        assert_eq!(scope.object, None);
    }

    #[test]
    fn scope_for_expand_uses_columns_level_and_the_object_name_for_a_table() {
        let node = Node::leaf("users", ObjectKind::Table);
        let rows = flatten(&[node], &FlattenOptions::default());
        let row = rows.iter().find(|r| r.label == "users").unwrap();
        let (scope, level) = scope_for_expand(row);
        assert_eq!(level, IntrospectLevel::Columns);
        assert_eq!(scope.object.as_deref(), Some("users"));
        assert_eq!(scope.schema, None);
    }

    #[test]
    fn scope_for_expand_derives_the_schema_from_a_two_segment_path() {
        let table = Node::leaf("users", ObjectKind::Table);
        let roots = vec![Node::with_children(
            "public",
            ObjectKind::Schema,
            vec![table],
        )];
        let options = FlattenOptions {
            group_mode: db_core::tree::GroupMode::Flat,
            ..FlattenOptions::default()
        };
        let rows = flatten(&roots, &options);
        let row = rows.iter().find(|r| r.label == "users").unwrap();
        let (scope, level) = scope_for_expand(row);
        assert_eq!(level, IntrospectLevel::Columns);
        assert_eq!(scope.schema.as_deref(), Some("public"));
        assert_eq!(scope.object.as_deref(), Some("users"));
    }

    #[test]
    fn object_ref_for_a_folder_row_is_none() {
        let node = Node::leaf("users", ObjectKind::Table);
        let rows = flatten(&[node], &FlattenOptions::default());
        let folder_row = rows.iter().find(|r| r.label == "Tables").unwrap();
        assert!(object_ref_for(folder_row).is_none());
    }

    #[test]
    fn object_ref_for_a_table_carries_its_kind_and_name() {
        let node = Node::leaf("users", ObjectKind::Table);
        let rows = flatten(&[node], &FlattenOptions::default());
        let row = rows.iter().find(|r| r.label == "users").unwrap();
        let object_ref = object_ref_for(row).unwrap();
        assert_eq!(object_ref.name, "users");
        assert_eq!(object_ref.kind, Some(ObjectKind::Table));
        assert_eq!(object_ref.schema, None);
    }

    #[test]
    fn qualified_name_joins_the_real_ancestry_with_dots() {
        let table = Node::leaf("users", ObjectKind::Table);
        let roots = vec![Node::with_children(
            "public",
            ObjectKind::Schema,
            vec![table],
        )];
        let rows = flatten(&roots, &FlattenOptions::default());
        let row = rows.iter().find(|r| r.label == "users").unwrap();
        assert_eq!(qualified_name(row), "public.users");
    }
}
