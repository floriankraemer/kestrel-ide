//! SQL assistance in the editor (database-tools plan F3.7,
//! `database-tools.md` §4/§5): schema-aware completion, lint-style
//! inspections, and two intentions ("Format SQL", "Go to DDL") for a tab
//! attached to a data source — a console file under
//! `db_core::console::consoles_dir`, or a project `.sql` file named in
//! `[database] file_sources`. Injected before the language-server gate the
//! same way `containers.rs` injects image-name completion: SQL has no
//! language server in this codebase, so every one of these would
//! otherwise answer nothing.
//!
//! ponytail: schema introspection here opens its own short-lived
//! connection per source id and caches the snapshot, rather than reusing
//! `bridge::database::console`'s already-open `SessionWorker`. That
//! worker's state (`console::Shared`) is private to `console.rs`, owned by
//! a parallel task in this plan wave (F3e) — reaching into it would mean
//! editing a file this task does not own. Upgrade path: once F3e lands,
//! expose a read accessor on `console::Shared` keyed by tab id and switch
//! this module to it, dropping the second connection. Until then, a
//! completion/inspection request answers with keywords only until this
//! module's own background introspection completes, cached per source id
//! for the rest of the session.

use core::pin::Pin;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use app_config::database::DataSourceSetting;
use db_core::datasource::{ConnectSpec, DataSource, Secrets};
use db_core::dialect::Dialect;
use db_core::driver::Connection;
use db_core::schema::{IntrospectLevel, IntrospectScope, ObjectRef, SchemaSnapshot};
use diagnostics_core::Severity;

use crate::bridge::ffi;

/// The shared diagnostics store's key for this module's rows (F3.7),
/// distinct from `lsp_source_key`'s per-language ones and from a build
/// tool's — a bare `.sql` file has neither an LSP source nor a build tool
/// behind it.
const DATABASE_INSPECTIONS_SOURCE: &str = "database:inspections";

/// How long a burst of keystrokes in an attached SQL file is allowed to
/// settle before inspections re-run — the same 500ms window
/// `mod.rs`'s `WATCHED_FILES_DEBOUNCE` doc comment reasons about for a
/// different burst, chosen here so a fast typist never re-parses on every
/// keystroke.
const DATABASE_INSPECTION_DEBOUNCE: Duration = Duration::from_millis(500);

const SECRET_SERVICE: &str = "ide.database";

/// Per-source cached state once a background introspection has answered:
/// the schema at `Columns` level, and the dialect the live connection
/// actually reported (authoritative — a static id-to-dialect guess would
/// only ever be a fallback, so this module keeps none and instead answers
/// with keywords alone until the real one lands).
#[derive(Clone)]
struct SourceInfo {
    dialect: Dialect,
    schema: SchemaSnapshot,
}

/// Everything this module owns across calls: which path attaches to which
/// source, each source's cached schema/dialect, which sources currently
/// have a background introspection in flight, and the debounce generation
/// per path (mirrors `mod.rs`'s `watch_flush_pending`, one counter per
/// path instead of one global flag since two attached files can be
/// mid-edit at once).
#[derive(Default)]
pub(crate) struct DatabaseAssist {
    attachments: RefCell<HashMap<String, String>>,
    sources: RefCell<HashMap<String, SourceInfo>>,
    introspecting: RefCell<std::collections::HashSet<String>>,
    debounce: RefCell<HashMap<String, u64>>,
}

impl DatabaseAssist {
    /// The source id `path` attaches to, resolving and caching it on first
    /// ask — a console file names its own source
    /// (`db_core::console::source_of`); a plain project file only does
    /// through `[database] file_sources`, which needs the project root and
    /// settings this free function reads fresh each time a path is not yet
    /// cached.
    fn attachment_for(&self, path: &str) -> Option<String> {
        if let Some(cached) = self.attachments.borrow().get(path) {
            return Some(cached.clone());
        }
        let source_id = resolve_attachment(Path::new(path))?;
        self.attachments
            .borrow_mut()
            .insert(path.to_string(), source_id.clone());
        Some(source_id)
    }

    fn forget(&self, path: &str) {
        self.attachments.borrow_mut().remove(path);
    }

    fn cached_schema(&self, source_id: &str) -> Option<(Dialect, SchemaSnapshot)> {
        self.sources
            .borrow()
            .get(source_id)
            .map(|info| (info.dialect, info.schema.clone()))
    }

    /// `true` the first time this is called for `source_id` since its last
    /// answer — the caller spawns a background fetch only then, so a burst
    /// of keystrokes before the first snapshot lands starts one connection,
    /// not one per keystroke.
    fn begin_introspecting(&self, source_id: &str) -> bool {
        self.introspecting
            .borrow_mut()
            .insert(source_id.to_string())
    }

    fn finish_introspecting(&self, source_id: &str, outcome: Option<(Dialect, SchemaSnapshot)>) {
        self.introspecting.borrow_mut().remove(source_id);
        if let Some((dialect, schema)) = outcome {
            self.sources
                .borrow_mut()
                .insert(source_id.to_string(), SourceInfo { dialect, schema });
        }
    }

    /// Bumps and returns this path's debounce generation — the counter a
    /// later, still-pending timer compares itself against
    /// ([`is_current_debounce`](Self::is_current_debounce)) to tell a
    /// superseded run from the current one.
    fn bump_debounce(&self, path: &str) -> u64 {
        let mut debounce = self.debounce.borrow_mut();
        let generation = debounce.entry(path.to_string()).or_insert(0);
        *generation += 1;
        *generation
    }

    fn is_current_debounce(&self, path: &str, generation: u64) -> bool {
        self.debounce.borrow().get(path) == Some(&generation)
    }
}

/// `path`'s default data source, if any — `None` for a file this module
/// gives no assistance to. Reads `app_core::resolve_config_dir` and the
/// shared session/project settings fresh, since this only runs on a
/// cache miss (see [`DatabaseAssist::attachment_for`]).
fn resolve_attachment(path: &Path) -> Option<String> {
    let config_dir = app_core::resolve_config_dir();
    if let Some(source_id) = db_core::console::source_of(path, &config_dir) {
        return Some(source_id);
    }
    let session = crate::bridge::registry::shared_session();
    let root = session.borrow().root_path()?.to_path_buf();
    let project = crate::bridge::convert::load_project_settings();
    let file_sources = &project.database.as_ref()?.file_sources;
    attachment_from_file_sources(path, &root, file_sources)
}

/// The `[database] file_sources` half of [`resolve_attachment`], pulled
/// out as a pure function so it is testable without the shared session or
/// a project settings file on disk.
fn attachment_from_file_sources(
    path: &Path,
    root: &Path,
    file_sources: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let relative = relative.to_str()?.replace('\\', "/");
    file_sources.get(&relative).cloned()
}

/// Every configured data source, global and project merged — a private
/// copy of `bridge::database::service::configured_sources`'s own body:
/// that one is `pub(super)` to `bridge::database`, a different module
/// tree this file must not reach into (see this module's doc comment).
fn configured_sources() -> Vec<DataSourceSetting> {
    let global = crate::bridge::convert::load_settings();
    let project = crate::bridge::convert::load_project_settings();
    settings_model::scope::resolve_database_sources(&global, &project)
        .into_iter()
        .map(|(setting, _)| setting)
        .collect()
}

/// A private copy of `bridge::database::service::secrets_for`, for the
/// same reason [`configured_sources`] is one.
fn secrets_for(id: &str) -> Secrets {
    let store = secret_store::SecretStore::new(SECRET_SERVICE);
    Secrets {
        password: store.load(id).ok().flatten(),
        ..Default::default()
    }
}

/// Connects to `source_id` fresh — never the console's or the Database
/// dock's already-open connection (this module's doc comment). Blocking;
/// every caller runs it off the Qt thread.
fn connect_source(source_id: &str) -> Result<Box<dyn Connection>, String> {
    let setting = configured_sources()
        .into_iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("no data source with id '{source_id}' is configured"))?;
    let data_source = DataSource::from(&setting);
    let read_only = data_source.read_only;
    let spec = ConnectSpec::from(&data_source, &secrets_for(source_id));
    let registry = db_drivers::DriverRegistry::builtin();
    let mut connection = registry
        .get(&spec.driver)
        .ok_or_else(|| format!("no driver registered for '{}'", spec.driver))?
        .connect(&spec)
        .map_err(|error| error.to_string())?;
    if read_only {
        let _ = connection.set_read_only(true);
    }
    Ok(connection)
}

/// A UTF-16 `(line, character)` caret converted to a byte offset — the
/// same helper `build_files.rs`'s `caret_byte_offset` is, duplicated
/// rather than shared across a module boundary this task does not own
/// (see this module's doc comment).
fn caret_byte_offset(text: &str, line: u32, character: u32) -> usize {
    let starts = editor_core::offsets::line_starts(text);
    let line_range = editor_core::offsets::line_range(text, &starts, line as usize);
    let line_text = &text[line_range.clone()];
    line_range.start + editor_core::offsets::byte_offset(line_text, character as usize)
}

fn byte_range_to_text_range(text: &str, range: std::ops::Range<usize>) -> lsp_core::TextRange {
    let starts = editor_core::offsets::utf16_line_starts(text);
    let start = editor_core::offsets::utf16_offset(text, range.start);
    let end = editor_core::offsets::utf16_offset(text, range.end);
    let (start_line, start_character) = editor_core::offsets::utf16_position_at(&starts, start);
    let (end_line, end_character) = editor_core::offsets::utf16_position_at(&starts, end);
    lsp_core::TextRange {
        start_line,
        start_character,
        end_line,
        end_character,
    }
}

/// `db_sql::dialects::sqlparser_dialect`'s own `GenericDialect` arm,
/// mirrored rather than imported (that function returns the grammar
/// object itself, which `inspections`/`parse` already apply — this only
/// needs the yes/no it was built from) — a parse error against a dialect
/// this crate cannot really parse (T-SQL, CQL, or a document store with
/// no SQL grammar at all) is a much weaker signal than one against a
/// dialect `sqlparser` actually models, so it is published as a warning,
/// not an error.
fn is_generic_dialect(dialect: Dialect) -> bool {
    matches!(
        dialect,
        Dialect::SqlServer | Dialect::Cassandra | Dialect::Mongo | Dialect::Redis
    )
}

fn to_lsp_completion_item(item: db_sql::CompletionItem) -> lsp_core::CompletionItem {
    // LSP `CompletionItemKind`: Struct (22) for a table, Interface (8) for
    // a view, Field (5) for a column, Keyword (14) — the same code
    // `fallback_completion`'s own keywords already use, so a database
    // file's keyword icon matches every other file's.
    let kind = match item.kind {
        db_sql::CompletionKind::Table => 22,
        db_sql::CompletionKind::View => 8,
        db_sql::CompletionKind::Column => 5,
        db_sql::CompletionKind::Keyword => 14,
    };
    lsp_core::CompletionItem {
        insert: item.label.clone(),
        label: item.label,
        kind: Some(kind),
        detail: item.detail.unwrap_or_default(),
        documentation: String::new(),
        sort_text: None,
        filter_text: None,
        is_snippet: false,
        deprecated: false,
        range: None,
        raw: serde_json::Value::Null,
    }
}

/// The "Format SQL" intention's item, or `None` when there is no statement
/// under the caret, or it is already exactly what `db_sql::format` would
/// produce (nothing to offer). Pure, so it is unit-testable without a Qt
/// runtime.
fn database_format_item(
    content: &str,
    dialect: Dialect,
    caret: usize,
    path: &str,
) -> Option<lsp_core::CodeActionItem> {
    let statements = db_sql::split(content, dialect);
    let statement = statements
        .iter()
        .find(|s| caret >= s.start && caret <= s.end)
        .or_else(|| statements.last())?;
    let original = statement.text(content);
    if original.trim().is_empty() {
        return None;
    }
    let formatted = db_sql::format(original, dialect);
    if formatted.trim() == original.trim() {
        return None;
    }
    let uri = lsp_core::uri_from_path(path);
    let range = byte_range_to_text_range(content, statement.start..statement.end);
    let edit = serde_json::json!({
        "changes": {
            uri: [{
                "range": {
                    "start": { "line": range.start_line, "character": range.start_character },
                    "end": { "line": range.end_line, "character": range.end_character },
                },
                "newText": formatted,
            }]
        }
    });
    Some(lsp_core::CodeActionItem {
        title: "Format SQL".to_string(),
        kind: Some(DATABASE_FORMAT_INTENTION_KIND.to_string()),
        edit: Some(edit),
        command: None,
        disabled: None,
        raw: serde_json::json!({}),
    })
}

/// The "Go to DDL" intention's item, or `None` when the caret is not on an
/// identifier at all. `source_id` travels in the item's `raw` payload
/// (there is nowhere else to carry it to `apply_database_intention`) —
/// `object.kind`, resolved against `schema` when one is already cached, is
/// only ever used for the title; `db_core::session::Session::ddl_of`
/// itself does not need it.
fn database_goto_ddl_item(
    content: &str,
    caret: usize,
    schema: Option<&SchemaSnapshot>,
    source_id: &str,
) -> Option<lsp_core::CodeActionItem> {
    let object = db_sql::navigate(content, caret, schema)?;
    Some(lsp_core::CodeActionItem {
        title: format!("Go to DDL of {}", object.name),
        kind: Some(DATABASE_GOTO_DDL_INTENTION_KIND.to_string()),
        edit: None,
        command: None,
        disabled: None,
        raw: serde_json::json!({
            "sourceId": source_id,
            "name": object.name,
            "schema": object.schema,
            "catalog": object.catalog,
        }),
    })
}

/// The intention kinds [`ffi::LanguageService::apply_intention`] routes:
/// `database.format` carries its own `edit` and needs no special apply
/// branch (it flows through `run_action` exactly like D7's build-file
/// quick fix); `database.goToDdl` is intercepted by
/// `apply_database_intention`, the same short-circuit
/// `apply_container_intention` gives `container.pull`.
const DATABASE_FORMAT_INTENTION_KIND: &str = "database.format";
const DATABASE_GOTO_DDL_INTENTION_KIND: &str = "database.goToDdl";

impl ffi::LanguageService {
    /// The database-assisted branch of `completionAt` (F3.7). `true` when
    /// `path` attaches to a data source and the popup has been filled —
    /// the caller stops there, exactly like `container_completion`.
    pub(crate) fn database_completion(
        mut self: Pin<&mut Self>,
        path: &str,
        line: u32,
        character: u32,
        _text_before_cursor: &str,
    ) -> bool {
        let Some(source_id) = self.database.attachment_for(path) else {
            return false;
        };
        let content = self
            .session
            .borrow()
            .content_for_path(Path::new(path))
            .unwrap_or_default();
        let offset = caret_byte_offset(&content, line, character);
        let (dialect, schema, ready) = match self.database.cached_schema(&source_id) {
            Some((dialect, schema)) => (dialect, schema, true),
            None => {
                self.as_mut().trigger_database_introspection(source_id);
                (
                    Dialect::Postgres,
                    SchemaSnapshot::new(IntrospectLevel::Names, Vec::new()),
                    false,
                )
            }
        };
        *self.completion_language.borrow_mut() = None;
        self.completion
            .borrow_mut()
            .begin(lsp_core::completion_prefix(_text_before_cursor));
        let items = db_sql::completion(&content, offset, &schema, dialect)
            .into_iter()
            .map(to_lsp_completion_item)
            .collect();
        *self.completions.borrow_mut() = lsp_core::CompletionList {
            items,
            is_incomplete: !ready,
        };
        self.as_mut().completion_ready();
        true
    }

    /// The database-assisted branch of `requestIntentions` (F3.7):
    /// "Format SQL" over the statement at the caret, and "Go to DDL" for
    /// the identifier under it. Short-circuits before the language-server
    /// path exactly like `container_intentions` — SQL has no server in
    /// this codebase.
    pub(crate) fn database_intentions(
        mut self: Pin<&mut Self>,
        path: &str,
        line: u32,
        character: u32,
    ) -> bool {
        let Some(source_id) = self.database.attachment_for(path) else {
            return false;
        };
        let content = self
            .session
            .borrow()
            .content_for_path(Path::new(path))
            .unwrap_or_default();
        let caret = caret_byte_offset(&content, line, character);
        let (dialect, schema) = self.database.cached_schema(&source_id).unwrap_or((
            Dialect::Postgres,
            SchemaSnapshot::new(IntrospectLevel::Names, Vec::new()),
        ));

        let mut intentions = Vec::new();
        if let Some(item) = database_format_item(&content, dialect, caret, path) {
            intentions.push(lsp_core::Intention {
                item,
                group: lsp_core::IntentionGroup::Other,
                preferred: false,
            });
        }
        if let Some(item) = database_goto_ddl_item(&content, caret, Some(&schema), &source_id) {
            intentions.push(lsp_core::Intention {
                item,
                group: lsp_core::IntentionGroup::Other,
                preferred: false,
            });
        }
        self.intentions_tracker.borrow_mut().begin();
        *self.intentions.borrow_mut() = intentions;
        self.intentions_language.borrow_mut().clear();
        self.as_mut().intentions_ready();
        true
    }

    /// The `database.goToDdl` branch of `applyIntention`: fetches the
    /// object's DDL off the Qt thread and opens it as a read-only virtual
    /// document, the same `db-ddl` scheme
    /// `bridge::database::service::DatabaseServiceRust::go_to_ddl` uses
    /// for the Database dock's own "Go to DDL", so re-navigating to the
    /// same object from either surface focuses the same tab.
    /// `database.format` is not handled here — it carries its own `edit`
    /// and returns `false` so `apply_intention` falls through to
    /// `run_action`, exactly like D7's build-file quick fix.
    pub(crate) fn apply_database_intention(
        mut self: Pin<&mut Self>,
        action: &lsp_core::CodeActionItem,
    ) -> bool {
        if action.kind.as_deref() != Some(DATABASE_GOTO_DDL_INTENTION_KIND) {
            return false;
        }
        let raw = &action.raw;
        let Some(source_id) = raw.get("sourceId").and_then(|v| v.as_str()) else {
            return true;
        };
        let Some(name) = raw.get("name").and_then(|v| v.as_str()) else {
            return true;
        };
        let mut object = ObjectRef::new(name);
        if let Some(schema) = raw.get("schema").and_then(|v| v.as_str()) {
            object = object.with_schema(schema);
        }
        if let Some(catalog) = raw.get("catalog").and_then(|v| v.as_str()) {
            object = object.with_catalog(catalog);
        }
        let key = format!(
            "{source_id}/{}.sql",
            object
                .schema
                .as_deref()
                .map(|schema| format!("{schema}.{}", object.name))
                .unwrap_or_else(|| object.name.clone())
        );
        let source_id = source_id.to_string();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let outcome = connect_source(&source_id).and_then(|mut connection| {
                let result = connection
                    .ddl_of(&object)
                    .map_err(|error| error.to_string());
                let _ = connection.close();
                result
            });
            let _ = qt_thread.queue(move |service: Pin<&mut ffi::LanguageService>| {
                service.database_ddl_ready(key, outcome);
            });
        });
        true
    }

    /// The background fetch [`apply_database_intention`] started has
    /// answered: open it as a virtual document, or report the refusal
    /// through `definitionUnavailable` — the same signal C12-followup's
    /// decompiled-metadata fetch uses for its own failure, so a database
    /// DDL fetch failing and a metadata fetch failing surface the same way.
    fn database_ddl_ready(mut self: Pin<&mut Self>, key: String, outcome: Result<String, String>) {
        match outcome {
            Ok(text) => {
                let opened = self
                    .session
                    .borrow_mut()
                    .open_virtual_document("db-ddl", &key, &text);
                self.as_mut().virtual_document_opened(
                    opened.id.raw(),
                    QString::from(opened.title.as_str()),
                    opened.newly_opened,
                );
            }
            Err(message) => {
                self.as_mut().definition_unavailable(QString::from(
                    format!("Could not fetch this object's DDL: {message}").as_str(),
                ));
            }
        }
    }

    /// D0's counterpart for a database-attached `.sql` file: registers it
    /// in `open_docs` (with no server, and no `did_open` — nothing here
    /// speaks LSP) purely so `document_changed`/`document_saved`/
    /// `document_closed` already gate on that map run this module's own
    /// inspections lifecycle for it. Runs inspections immediately, the
    /// same "don't wait for the first debounce" reasoning
    /// `open_build_file_document` gives version hints.
    pub(crate) fn open_database_document(
        mut self: Pin<&mut Self>,
        path_str: &str,
        _text: &QString,
    ) {
        if self.database.attachment_for(path_str).is_none() {
            return;
        }
        self.open_docs
            .borrow_mut()
            .insert(path_str.to_string(), "sql".to_string());
        self.as_mut().refresh_database_inspections_now(path_str);
    }

    /// The caret's tab changed text: (re)arm this path's debounce. A cheap
    /// no-op for a path with no attachment — every `document_changed` call
    /// reaches this, database-attached or not.
    pub(crate) fn schedule_database_inspection(mut self: Pin<&mut Self>, path: &str) {
        if self.database.attachment_for(path).is_none() {
            return;
        }
        let generation = self.database.bump_debounce(path);
        let qt_thread = self.as_mut().qt_thread();
        let path_owned = path.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(DATABASE_INSPECTION_DEBOUNCE);
            let _ = qt_thread.queue(move |service: Pin<&mut ffi::LanguageService>| {
                service.run_debounced_database_inspection(path_owned, generation);
            });
        });
    }

    /// The debounce window lapsed: run inspections unless a later
    /// keystroke already rearmed it (`is_current_debounce`'s job — see
    /// `DatabaseAssist`'s own doc comment).
    fn run_debounced_database_inspection(mut self: Pin<&mut Self>, path: String, generation: u64) {
        if !self.database.is_current_debounce(&path, generation) {
            return;
        }
        self.as_mut().refresh_database_inspections_now(&path);
    }

    /// Runs `db_sql::inspections` plus `db_sql::parse` over every
    /// statement in `path`'s current buffer and publishes the result under
    /// [`DATABASE_INSPECTIONS_SOURCE`] — on save
    /// (`document_saved`) and once the idle debounce lapses
    /// (`run_debounced_database_inspection`). A path with no attachment
    /// clears whatever this source may still be publishing for it (a file
    /// can detach without closing, e.g. the console bar's source combo
    /// changing) rather than leaving a stale squiggle behind.
    pub(crate) fn refresh_database_inspections_now(mut self: Pin<&mut Self>, path: &str) {
        let uri = lsp_core::uri_from_path(path);
        let Some(source_id) = self.database.attachment_for(path) else {
            self.store
                .borrow_mut()
                .remove(DATABASE_INSPECTIONS_SOURCE, &uri);
            self.as_mut().diagnostics_changed();
            return;
        };
        let content = self
            .session
            .borrow()
            .content_for_path(Path::new(path))
            .unwrap_or_default();
        let (dialect, schema) = match self.database.cached_schema(&source_id) {
            Some(pair) => pair,
            None => {
                self.as_mut().trigger_database_introspection(source_id);
                (
                    Dialect::Postgres,
                    SchemaSnapshot::new(IntrospectLevel::Names, Vec::new()),
                )
            }
        };
        let mut diagnostics = db_sql::inspections(&content, dialect, &schema);
        for statement in db_sql::split(&content, dialect) {
            let text = statement.text(&content);
            if text.trim().is_empty() {
                continue;
            }
            if let Err(diagnostic) = db_sql::parse(text, dialect, statement.start_pos) {
                let mut diagnostic = *diagnostic;
                if is_generic_dialect(dialect) {
                    diagnostic.severity = Severity::Warning;
                }
                diagnostics.push(diagnostic);
            }
        }
        self.store
            .borrow_mut()
            .replace(DATABASE_INSPECTIONS_SOURCE, &uri, diagnostics);
        self.as_mut().diagnostics_changed();
    }

    /// Forgets `path`'s cached attachment and clears whatever this source
    /// last published for it — `document_closed`'s counterpart, mirroring
    /// `clear_version_hints`.
    pub(crate) fn clear_database_inspections(mut self: Pin<&mut Self>, path: &str) {
        self.database.forget(path);
        let uri = lsp_core::uri_from_path(path);
        self.store
            .borrow_mut()
            .remove(DATABASE_INSPECTIONS_SOURCE, &uri);
        self.as_mut().diagnostics_changed();
    }

    /// Connects to `source_id` and introspects it at `Columns` level in
    /// the background, caching the answer for every attached file that
    /// shares this source — a no-op if one is already in flight (see
    /// `DatabaseAssist::begin_introspecting`).
    fn trigger_database_introspection(mut self: Pin<&mut Self>, source_id: String) {
        if !self.database.begin_introspecting(&source_id) {
            return;
        }
        let qt_thread = self.as_mut().qt_thread();
        let thread_source_id = source_id.clone();
        std::thread::spawn(move || {
            let outcome = connect_source(&thread_source_id).and_then(|mut connection| {
                let dialect = connection.dialect();
                let result = connection
                    .introspect(&IntrospectScope::default(), IntrospectLevel::Columns)
                    .map_err(|error| error.to_string());
                let _ = connection.close();
                result.map(|schema| (dialect, schema))
            });
            let _ = qt_thread.queue(move |service: Pin<&mut ffi::LanguageService>| {
                service.database_schema_ready(thread_source_id, outcome);
            });
        });
    }

    fn database_schema_ready(
        self: Pin<&mut Self>,
        source_id: String,
        outcome: Result<(Dialect, SchemaSnapshot), String>,
    ) {
        self.database.finish_introspecting(&source_id, outcome.ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::schema::{Children, Node, ObjectKind};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from("/project")
    }

    #[test]
    fn a_relative_path_named_in_file_sources_resolves() {
        let file_sources = BTreeMap::from([("sql/reports.sql".to_string(), "abc123".to_string())]);
        let path = root().join("sql/reports.sql");
        assert_eq!(
            attachment_from_file_sources(&path, &root(), &file_sources),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn a_path_outside_the_project_root_does_not_resolve() {
        let file_sources = BTreeMap::from([("sql/reports.sql".to_string(), "abc123".to_string())]);
        assert_eq!(
            attachment_from_file_sources(
                Path::new("/elsewhere/sql/reports.sql"),
                &root(),
                &file_sources
            ),
            None
        );
    }

    #[test]
    fn a_path_not_named_in_file_sources_does_not_resolve() {
        let file_sources = BTreeMap::new();
        let path = root().join("sql/reports.sql");
        assert_eq!(
            attachment_from_file_sources(&path, &root(), &file_sources),
            None
        );
    }

    #[test]
    fn table_view_column_and_keyword_map_to_the_popups_icons() {
        let table = to_lsp_completion_item(db_sql::CompletionItem {
            label: "users".to_string(),
            kind: db_sql::CompletionKind::Table,
            detail: None,
        });
        assert_eq!(table.kind, Some(22));
        assert_eq!(table.insert, "users");

        let view = to_lsp_completion_item(db_sql::CompletionItem {
            label: "active_users".to_string(),
            kind: db_sql::CompletionKind::View,
            detail: None,
        });
        assert_eq!(view.kind, Some(8));

        let column = to_lsp_completion_item(db_sql::CompletionItem {
            label: "id".to_string(),
            kind: db_sql::CompletionKind::Column,
            detail: Some("users".to_string()),
        });
        assert_eq!(column.kind, Some(5));
        assert_eq!(column.detail, "users");

        let keyword = to_lsp_completion_item(db_sql::CompletionItem {
            label: "SELECT".to_string(),
            kind: db_sql::CompletionKind::Keyword,
            detail: None,
        });
        assert_eq!(keyword.kind, Some(14));
    }

    #[test]
    fn sql_server_cassandra_mongo_and_redis_are_the_generic_arm() {
        assert!(is_generic_dialect(Dialect::SqlServer));
        assert!(is_generic_dialect(Dialect::Cassandra));
        assert!(is_generic_dialect(Dialect::Mongo));
        assert!(is_generic_dialect(Dialect::Redis));
        assert!(!is_generic_dialect(Dialect::Postgres));
        assert!(!is_generic_dialect(Dialect::MySql));
        assert!(!is_generic_dialect(Dialect::Sqlite));
    }

    #[test]
    fn format_item_offers_an_edit_for_a_misindented_statement() {
        let item = database_format_item(
            "select a, b from t where a = 1",
            Dialect::Postgres,
            0,
            "/tmp/console1.sql",
        )
        .expect("an unformatted statement offers a fix");
        assert_eq!(item.kind.as_deref(), Some(DATABASE_FORMAT_INTENTION_KIND));
        assert!(item.edit.is_some());
    }

    #[test]
    fn format_item_is_none_once_already_formatted() {
        let formatted = db_sql::format("select 1", Dialect::Sqlite);
        assert_eq!(
            database_format_item(&formatted, Dialect::Sqlite, 0, "/tmp/console1.sql"),
            None
        );
    }

    #[test]
    fn format_item_is_none_for_an_empty_buffer() {
        assert_eq!(
            database_format_item("", Dialect::Postgres, 0, "/tmp/console1.sql"),
            None
        );
    }

    fn users_schema() -> SchemaSnapshot {
        SchemaSnapshot::new(
            IntrospectLevel::Names,
            vec![Node::with_children(
                "public",
                ObjectKind::Schema,
                vec![Node::leaf("users", ObjectKind::Table)],
            )],
        )
    }

    #[test]
    fn goto_ddl_item_carries_the_identifier_under_the_caret() {
        let sql = "SELECT * FROM users";
        let caret = sql.find("users").unwrap() + 2;
        let item = database_goto_ddl_item(sql, caret, Some(&users_schema()), "abc123")
            .expect("the caret sits on an identifier");
        assert_eq!(item.kind.as_deref(), Some(DATABASE_GOTO_DDL_INTENTION_KIND));
        assert_eq!(item.raw["sourceId"], "abc123");
        assert_eq!(item.raw["name"], "users");
    }

    #[test]
    fn goto_ddl_item_is_none_off_an_identifier() {
        let sql = "SELECT * FROM users";
        // Offset 8 sits in the whitespace between `*` and `FROM`.
        assert_eq!(database_goto_ddl_item(sql, 8, None, "abc123"), None);
    }

    #[test]
    fn a_children_loaded_marker_is_reachable_for_the_schema_fixture() {
        // Guards `users_schema`'s own shape rather than this module's
        // logic: a `Children::NotLoaded` schema still exercises
        // `database_goto_ddl_item`'s `find_kind` path in `db_sql::navigate`
        // (that crate's own tests cover it in depth) — this only proves
        // the fixture built above is a `Loaded` tree, not a degenerate one.
        assert!(matches!(
            users_schema().roots[0].children,
            Children::Loaded(_)
        ));
    }
}
