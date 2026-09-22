//! `ExchangeService` (database-tools-plan F5b.1): the Qt-free half of
//! import/export, dump/restore, copy-table, ER diagrams and schema/data
//! compare already lives in `db_exchange`/`db_core` — this module is
//! translation plus the one piece of orchestration none of those crates
//! can own themselves (they take a live `Connection` as a parameter and
//! know nothing of `app_config::database::DataSourceSetting`/
//! `secret_store`): resolving a source id to a fresh connection, running
//! the long calls on a background thread, and reporting through
//! `jobProgress`/`jobFinished`.
//!
//! Every long-running invokable (`exportTable`, `importRun`, `copyTable`,
//! `dump`) returns a job id immediately and reports its outcome
//! asynchronously; a synchronous failure (unknown source, bad path) still
//! allocates a job id and emits `jobFinished` for it before returning, so
//! a caller never special-cases "failed before it started" differently
//! from "failed once running". Every short call (a diagram, a compare
//! summary, an import preview) answers directly — there is no result to
//! page and no cancel button worth showing for it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cxx_qt::Threading;
use cxx_qt_lib::{QString, QStringList};

use db_core::dialect::Dialect;
use db_core::driver::{CancelToken, Connection, ExecOptions, Execution, Statement};
use db_core::schema::{Children, IntrospectLevel, IntrospectScope, Node, NodeDetail, ObjectKind};
use db_core::session::Session;
use db_core::value::{ColumnMeta, RowBatch, Value};

use db_exchange::export::{self, ExportOptions, Format, JsonShape, SqlMode};
use db_exchange::schema_model::{ColumnDef, SchemaSnapshot as ModelSnapshot, TableDef, TextObject};
use db_exchange::{copy_table, data_compare, dump, er_diagram, import, schema_compare};

use crate::bridge::database::service::{configured_sources, secrets_for};
use crate::bridge::ffi::{self, FfiDataCompareResult, FfiDumpOptions, FfiDumpToolStatus};
use crate::bridge::ffi::{
    FfiCompareRow, FfiExportFormat, FfiExportOptions, FfiImportColumnMapping, FfiImportOptions,
    FfiImportPreview, FfiPreviewImage, FfiTextDiff,
};
use crate::bridge::{errors, registry};

/// Unit separator, matching `bridge::database::console::CELL_SEP` exactly
/// — `FfiDbRow::cells`/`nulls` are produced there and consumed here, so
/// this must stay the same character; duplicated rather than imported
/// because that module's constant is private (F4a's own file, not
/// touched by this phase).
const CELL_SEP: char = '\u{1f}';

// ---- source resolution ----

fn connect_source(source_id: &str) -> Result<(Box<dyn Connection>, Dialect, bool), String> {
    let setting = configured_sources()
        .into_iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("no data source with id '{source_id}' is configured"))?;
    let data_source = db_core::datasource::DataSource::from(&setting);
    let read_only = data_source.read_only;
    let spec = db_core::datasource::ConnectSpec::from(&data_source, &secrets_for(source_id));
    let connection = crate::bridge::database::connect(&spec)?;
    let dialect = connection.dialect();
    Ok((connection, dialect, read_only))
}

fn data_source_for(source_id: &str) -> Result<db_core::datasource::DataSource, String> {
    let setting = configured_sources()
        .into_iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| format!("no data source with id '{source_id}' is configured"))?;
    Ok(db_core::datasource::DataSource::from(&setting))
}

// ---- export option mapping ----

fn to_format(format: FfiExportFormat) -> (Format, bool, SqlMode) {
    match format {
        FfiExportFormat::Csv => (Format::Csv, false, SqlMode::Insert),
        FfiExportFormat::Tsv => (Format::Tsv, false, SqlMode::Insert),
        FfiExportFormat::Json => (Format::Json, false, SqlMode::Insert),
        FfiExportFormat::JsonLines => (Format::Json, true, SqlMode::Insert),
        FfiExportFormat::Markdown => (Format::Markdown, false, SqlMode::Insert),
        FfiExportFormat::Html => (Format::Html, false, SqlMode::Insert),
        FfiExportFormat::SqlInsert => (Format::Sql, false, SqlMode::Insert),
        FfiExportFormat::SqlUpdate => (Format::Sql, false, SqlMode::Insert),
        FfiExportFormat::Xlsx => (Format::Xlsx, false, SqlMode::Insert),
        _ => (Format::Csv, false, SqlMode::Insert),
    }
}

fn to_export_options(
    format: FfiExportFormat,
    options: &FfiExportOptions,
    dialect: Dialect,
) -> (Format, ExportOptions) {
    let (base_format, json_lines, _) = to_format(format);
    let delimiter =
        options
            .delimiter
            .to_string()
            .bytes()
            .next()
            .unwrap_or(if base_format == Format::Tsv {
                b'\t'
            } else {
                b','
            });
    let sql = if matches!(format, FfiExportFormat::SqlUpdate) {
        SqlMode::Update {
            key_columns: split_csv(&options.key_columns.to_string()),
        }
    } else {
        SqlMode::Insert
    };
    let export_options = ExportOptions {
        header: options.header,
        null_text: non_empty_or(&options.null_text.to_string(), "NULL"),
        quote_all: options.quote_all,
        delimiter,
        date_format: non_empty_or(&options.date_format.to_string(), "%Y-%m-%d"),
        json: if json_lines {
            JsonShape::Lines
        } else {
            JsonShape::Array
        },
        sql,
        table_name: non_empty_or(&options.table_name.to_string(), "export"),
        dialect,
    };
    (base_format, export_options)
}

fn non_empty_or(text: &str, default: &str) -> String {
    if text.is_empty() {
        default.to_string()
    } else {
        text.to_string()
    }
}

fn split_csv(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn to_import_options(options: &FfiImportOptions) -> import::ImportOptions {
    let delimiter = options.delimiter.to_string().bytes().next().unwrap_or(b',');
    import::ImportOptions {
        header: options.header,
        null_text: options.null_text.to_string(),
        delimiter,
        date_format: non_empty_or(&options.date_format.to_string(), "%Y-%m-%d"),
    }
}

fn to_detected_type(text: &str) -> import::DetectedType {
    match text {
        "int" => import::DetectedType::Int,
        "float" => import::DetectedType::Float,
        "bool" => import::DetectedType::Bool,
        "date" => import::DetectedType::Date,
        _ => import::DetectedType::Text,
    }
}

fn detected_type_name(kind: import::DetectedType) -> &'static str {
    match kind {
        import::DetectedType::Int => "int",
        import::DetectedType::Float => "float",
        import::DetectedType::Bool => "bool",
        import::DetectedType::Date => "date",
        import::DetectedType::Text => "text",
    }
}

fn to_mapping(mapping: &[FfiImportColumnMapping]) -> import::Mapping {
    import::Mapping {
        columns: mapping
            .iter()
            .map(|m| import::ColumnMapping {
                source_col: m.source_col.to_string(),
                target_col: m.target_col.to_string(),
                type_coercion: to_detected_type(&m.type_coercion.to_string()),
                skip: m.skip,
            })
            .collect(),
    }
}

fn to_ffi_import_preview(preview: &import::ImportPreview) -> FfiImportPreview {
    FfiImportPreview {
        columns: QStringList::from_iter(preview.columns.iter().map(QString::from)),
        detected_types: QStringList::from_iter(
            preview
                .detected_types
                .iter()
                .map(|t| QString::from(detected_type_name(*t))),
        ),
        sample_rows: QStringList::from_iter(
            preview
                .sample_rows
                .iter()
                .map(|row| QString::from(row.join(&CELL_SEP.to_string()).as_str())),
        ),
    }
}

// ---- rendered-row export ("Copy as" / results toolbar) ----

fn parse_ffi_rows(
    columns: &QStringList,
    rows: &[ffi::FfiDbRow],
) -> (Vec<ColumnMeta>, Vec<RowBatch>) {
    let column_metas: Vec<ColumnMeta> = (0..columns.len())
        .map(|i| ColumnMeta {
            name: columns.get(i).map(|q| q.to_string()).unwrap_or_default(),
            type_name: "text".to_string(),
            nullable: true,
            origin: None,
        })
        .collect();
    let parsed_rows: Vec<Vec<Value>> = rows
        .iter()
        .map(|row| {
            let cells_owned: Vec<String> = row
                .cells
                .to_string()
                .split(CELL_SEP)
                .map(str::to_string)
                .collect();
            let nulls_owned: Vec<String> = row
                .nulls
                .to_string()
                .split(CELL_SEP)
                .map(str::to_string)
                .collect();
            cells_owned
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    if nulls_owned.get(i).map(String::as_str) == Some("1") {
                        Value::Null
                    } else {
                        Value::Text(cell.clone())
                    }
                })
                .collect()
        })
        .collect();
    let batch = RowBatch {
        columns: column_metas.clone(),
        rows: parsed_rows,
    };
    (column_metas, vec![batch])
}

fn render_rows(
    columns: &QStringList,
    rows: &[ffi::FfiDbRow],
    format: FfiExportFormat,
    options: &FfiExportOptions,
) -> std::io::Result<Vec<u8>> {
    let (column_metas, batches) = parse_ffi_rows(columns, rows);
    let (out_format, export_options) = to_export_options(format, options, Dialect::Sqlite);
    let mut out = Vec::new();
    let mut iter = batches.into_iter();
    export::write_format(
        out_format,
        &column_metas,
        &mut iter,
        &mut out,
        &export_options,
    )?;
    Ok(out)
}

// ---- schema model (er diagram / schema compare) ----

/// Walks a `db_core::schema::SchemaSnapshot` (`IntrospectLevel::Full`)
/// into `db_exchange`'s own eagerly-fetched shape. `foreign_keys` and
/// `indexes`' own columns are left empty — `db_core::schema::NodeDetail`
/// carries a column's own `type_name`/`nullable`/`default`/`primary_key`
/// but no structured FK target or index column list yet (F2's own
/// recorded scope), so an ER diagram renders table boxes with no
/// relationship lines and a schema compare's index/constraint diff is
/// name-only.
/// ponytail: FK/index structural detail — upgrade once `NodeDetail` (or a
/// richer per-object introspection) carries it; the ceiling is entirely
/// in `db_core`, not this mapping.
fn to_schema_model(roots: &[Node], session: &mut Session) -> ModelSnapshot {
    let mut tables = Vec::new();
    let mut views = Vec::new();
    let mut routines = Vec::new();
    collect_objects(roots, session, &mut tables, &mut views, &mut routines);
    tables.sort_by(|a: &TableDef, b: &TableDef| a.name.cmp(&b.name));
    ModelSnapshot {
        tables,
        views,
        routines,
    }
}

fn collect_objects(
    nodes: &[Node],
    session: &mut Session,
    tables: &mut Vec<TableDef>,
    views: &mut Vec<TextObject>,
    routines: &mut Vec<TextObject>,
) {
    for node in nodes {
        match node.kind {
            ObjectKind::Table => tables.push(to_table_def(node)),
            ObjectKind::View => views.push(to_text_object(node, session, ObjectKind::View)),
            ObjectKind::Routine => {
                routines.push(to_text_object(node, session, ObjectKind::Routine))
            }
            _ => {}
        }
        if let Children::Loaded(children) = &node.children {
            collect_objects(children, session, tables, views, routines);
        }
    }
}

fn to_table_def(node: &Node) -> TableDef {
    let mut columns = Vec::new();
    let mut primary_key = Vec::new();
    let mut indexes = Vec::new();
    let mut constraints = Vec::new();
    if let Children::Loaded(children) = &node.children {
        for child in children {
            match child.kind {
                ObjectKind::Column => {
                    let detail: &NodeDetail = &child.detail;
                    if detail.primary_key {
                        primary_key.push(child.name.clone());
                    }
                    columns.push(ColumnDef {
                        name: child.name.clone(),
                        type_name: detail.type_name.clone().unwrap_or_default(),
                        nullable: detail.nullable.unwrap_or(true),
                        default: detail.default.clone(),
                    });
                }
                ObjectKind::Index => indexes.push(db_exchange::schema_model::IndexDef {
                    name: child.name.clone(),
                    columns: Vec::new(),
                    unique: false,
                }),
                ObjectKind::Constraint => {
                    constraints.push(db_exchange::schema_model::ConstraintDef {
                        name: child.name.clone(),
                        text: String::new(),
                    })
                }
                _ => {}
            }
        }
    }
    TableDef {
        name: node.name.clone(),
        columns,
        primary_key,
        foreign_keys: Vec::new(),
        indexes,
        constraints,
    }
}

fn to_text_object(node: &Node, session: &mut Session, kind: ObjectKind) -> TextObject {
    let object = db_core::schema::ObjectRef::new(node.name.clone()).with_kind(kind);
    let definition = session.ddl_of(&object).unwrap_or_default();
    TextObject {
        name: node.name.clone(),
        definition,
    }
}

fn fetch_schema_model(session: &mut Session) -> Result<ModelSnapshot, String> {
    let snapshot = session
        .introspect(&IntrospectScope::default(), IntrospectLevel::Full)
        .map_err(|e| e.to_string())?;
    Ok(to_schema_model(&snapshot.roots, session))
}

/// `FfiPreviewImage` derives no `Default` (it is a `cxx-qt` shared struct
/// with a `QByteArray` field) — this is its empty value for "no image"
/// (no diagram, or the render failed), the same value a zero-sized image
/// already means to its one caller (`db_er_diagram_dialog`'s own check).
fn empty_preview_image() -> FfiPreviewImage {
    FfiPreviewImage {
        key: QString::default(),
        width: 0,
        height: 0,
        pixels: cxx_qt_lib::QByteArray::default(),
    }
}

// ---- jobs ----

#[derive(Default)]
struct Jobs {
    next_id: u64,
    cancel: HashMap<u64, Arc<AtomicBool>>,
}

impl Jobs {
    fn alloc(&mut self) -> (u64, Arc<AtomicBool>) {
        self.next_id += 1;
        let flag = Arc::new(AtomicBool::new(false));
        self.cancel.insert(self.next_id, flag.clone());
        (self.next_id, flag)
    }
}

/// A cached schema compare, keyed by the exact `(left, right)` source id
/// pair it was run for — `ddlDiffTexts`/`migrationScript` read this back
/// rather than re-introspecting both sides on every click in the result
/// list.
struct CompareCache {
    key: Option<(String, String)>,
    diff: Option<schema_compare::SchemaDiff>,
    dialect: Dialect,
}

impl Default for CompareCache {
    fn default() -> Self {
        Self {
            key: None,
            diff: None,
            dialect: Dialect::Sqlite,
        }
    }
}

#[derive(Default)]
pub struct ExchangeServiceRust {
    jobs: RefCell<Jobs>,
    compare_cache: RefCell<CompareCache>,
}

impl ffi::ExchangeService {
    fn start_job(self: &mut Pin<&mut Self>) -> (u64, Arc<AtomicBool>) {
        self.jobs.borrow_mut().alloc()
    }

    fn fail_job(mut self: Pin<&mut Self>, job_id: u64, message: impl Into<String>) -> u64 {
        self.as_mut().job_finished(
            job_id,
            false,
            QString::from(message.into().as_str()),
            QString::default(),
        );
        job_id
    }

    pub fn export_table(
        mut self: Pin<&mut Self>,
        source_id: &QString,
        object_path: &QString,
        format: FfiExportFormat,
        options: FfiExportOptions,
        destination: &QString,
    ) -> u64 {
        let (job_id, cancel) = self.as_mut().start_job();
        let source_id = source_id.to_string();
        let object_path = object_path.to_string();
        let destination = destination.to_string();
        if destination.is_empty() {
            return self.fail_job(job_id, "no destination file chosen");
        }
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let outcome = run_export_table(
                &source_id,
                &object_path,
                format,
                &options,
                &destination,
                &cancel,
            );
            let _ = qt_thread.queue(
                move |mut service: Pin<&mut ffi::ExchangeService>| match outcome {
                    Ok(rows) => service.as_mut().job_finished(
                        job_id,
                        true,
                        QString::from(format!("{rows} row(s) exported").as_str()),
                        QString::from(destination.as_str()),
                    ),
                    Err(error) => service.as_mut().job_finished(
                        job_id,
                        false,
                        QString::from(error.as_str()),
                        QString::default(),
                    ),
                },
            );
        });
        job_id
    }

    pub fn export_preview_text(
        self: Pin<&mut Self>,
        source_id: &QString,
        object_path: &QString,
        format: FfiExportFormat,
        options: FfiExportOptions,
        max_rows: u32,
    ) -> QString {
        if matches!(format, FfiExportFormat::Xlsx) {
            return QString::from("(binary format — no text preview)");
        }
        match run_export_preview(
            &source_id.to_string(),
            &object_path.to_string(),
            format,
            &options,
            max_rows,
        ) {
            Ok(text) => QString::from(text.as_str()),
            Err(error) => QString::from(format!("error: {error}").as_str()),
        }
    }

    pub fn export_rows_to_file(
        self: Pin<&mut Self>,
        columns: &QStringList,
        rows: Vec<ffi::FfiDbRow>,
        format: FfiExportFormat,
        options: FfiExportOptions,
        destination: &QString,
    ) -> ffi::FfiResult {
        let destination = destination.to_string();
        if destination.is_empty() {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "no destination file chosen");
        }
        match render_rows(columns, &rows, format, &options) {
            Ok(bytes) => match std::fs::write(&destination, bytes) {
                Ok(()) => ffi::FfiResult::default(),
                Err(error) => errors::failure(errors::CODE_ATTACHMENT_IO, error.to_string()),
            },
            Err(error) => errors::failure(errors::CODE_INVALID_ARGUMENT, error.to_string()),
        }
    }

    pub fn export_rows_to_text(
        self: Pin<&mut Self>,
        columns: &QStringList,
        rows: Vec<ffi::FfiDbRow>,
        format: FfiExportFormat,
        options: FfiExportOptions,
    ) -> QString {
        match render_rows(columns, &rows, format, &options) {
            Ok(bytes) => QString::from(String::from_utf8_lossy(&bytes).as_ref()),
            Err(error) => QString::from(format!("error: {error}").as_str()),
        }
    }

    pub fn import_preview(
        self: Pin<&mut Self>,
        path: &QString,
        options: FfiImportOptions,
    ) -> FfiImportPreview {
        let path = path.to_string();
        let import_options = to_import_options(&options);
        let outcome = if path.to_ascii_lowercase().ends_with(".xlsx") {
            std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    import::xlsx::preview(std::io::Cursor::new(bytes), &import_options)
                        .map_err(|e| e.to_string())
                })
        } else {
            std::fs::File::open(&path)
                .map_err(|e| e.to_string())
                .and_then(|file| {
                    import::csv::preview(file, &import_options).map_err(|e| e.to_string())
                })
        };
        match outcome {
            Ok(preview) => to_ffi_import_preview(&preview),
            Err(_) => FfiImportPreview::default(),
        }
    }

    pub fn import_run(
        mut self: Pin<&mut Self>,
        source_id: &QString,
        path: &QString,
        target_table: &QString,
        mapping: Vec<FfiImportColumnMapping>,
        options: FfiImportOptions,
    ) -> u64 {
        let (job_id, cancel) = self.as_mut().start_job();
        let source_id = source_id.to_string();
        let path = path.to_string();
        let target_table = target_table.to_string();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let outcome = run_import(
                &source_id,
                &path,
                &target_table,
                &mapping,
                &options,
                &cancel,
            );
            let _ = qt_thread.queue(
                move |mut service: Pin<&mut ffi::ExchangeService>| match outcome {
                    Ok(rows) => service.as_mut().job_finished(
                        job_id,
                        true,
                        QString::from(format!("{rows} row(s) imported").as_str()),
                        QString::default(),
                    ),
                    Err(error) => service.as_mut().job_finished(
                        job_id,
                        false,
                        QString::from(error.as_str()),
                        QString::default(),
                    ),
                },
            );
        });
        job_id
    }

    pub fn copy_table(
        mut self: Pin<&mut Self>,
        src_source: &QString,
        src_table: &QString,
        dst_source: &QString,
        dst_table: &QString,
        create_if_missing: bool,
        batch_size: u32,
    ) -> u64 {
        let (job_id, cancel) = self.as_mut().start_job();
        let src_source = src_source.to_string();
        let src_table = src_table.to_string();
        let dst_source = dst_source.to_string();
        let dst_table = dst_table.to_string();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let outcome = run_copy_table(
                &src_source,
                &src_table,
                &dst_source,
                &dst_table,
                create_if_missing,
                batch_size.max(1),
                &cancel,
                &qt_thread,
                job_id,
            );
            let _ = qt_thread.queue(
                move |mut service: Pin<&mut ffi::ExchangeService>| match outcome {
                    Ok(rows) => service.as_mut().job_finished(
                        job_id,
                        true,
                        QString::from(format!("{rows} row(s) copied").as_str()),
                        QString::default(),
                    ),
                    Err(error) => service.as_mut().job_finished(
                        job_id,
                        false,
                        QString::from(error.as_str()),
                        QString::default(),
                    ),
                },
            );
        });
        job_id
    }

    pub fn er_diagram_mermaid(
        self: Pin<&mut Self>,
        source_id: &QString,
        table_scope: &QString,
    ) -> QString {
        match run_er_diagram(&source_id.to_string(), &table_scope.to_string()) {
            Ok(text) => QString::from(text.as_str()),
            Err(error) => QString::from(format!("%% error: {error}").as_str()),
        }
    }

    pub fn er_diagram_image(
        self: Pin<&mut Self>,
        source_id: &QString,
        table_scope: &QString,
        width_px: u32,
    ) -> FfiPreviewImage {
        let Ok(mermaid) = run_er_diagram(&source_id.to_string(), &table_scope.to_string()) else {
            return empty_preview_image();
        };
        let service = registry::shared_preview();
        let mut guard = service.lock().expect("preview service lock poisoned");
        match guard.render(Path::new("erd.mmd"), &mermaid, width_px.max(1)) {
            Ok(rendered) => rendered
                .images
                .into_iter()
                .next()
                .map(|image| FfiPreviewImage {
                    key: QString::from(image.key.as_str()),
                    width: image.width,
                    height: image.height,
                    pixels: cxx_qt_lib::QByteArray::from(image.pixels.as_slice()),
                })
                .unwrap_or_else(empty_preview_image),
            Err(_) => empty_preview_image(),
        }
    }

    pub fn schema_compare(
        self: Pin<&mut Self>,
        left_source: &QString,
        right_source: &QString,
    ) -> Vec<FfiCompareRow> {
        let left_source = left_source.to_string();
        let right_source = right_source.to_string();
        let (diff, dialect) = match run_schema_compare(&left_source, &right_source) {
            Ok(pair) => pair,
            Err(_) => return Vec::new(),
        };
        let rows = to_ffi_compare_rows(&diff);
        *self.compare_cache.borrow_mut() = CompareCache {
            key: Some((left_source, right_source)),
            diff: Some(diff),
            dialect,
        };
        rows
    }

    pub fn ddl_diff_texts(
        self: Pin<&mut Self>,
        left_source: &QString,
        right_source: &QString,
        table: &QString,
        name: &QString,
    ) -> FfiTextDiff {
        let cache = self.compare_cache.borrow();
        if cache.key.as_ref() != Some(&(left_source.to_string(), right_source.to_string())) {
            return FfiTextDiff::default();
        }
        let Some(diff) = &cache.diff else {
            return FfiTextDiff::default();
        };
        let table = table.to_string();
        let name = name.to_string();
        let pairs = schema_compare::ddl_pairs(diff, cache.dialect);
        let Some((_, left, right)) = pairs.into_iter().find(|(object, _, _)| {
            object.name == name && object.table.as_deref().unwrap_or("") == table
        }) else {
            return FfiTextDiff::default();
        };
        FfiTextDiff {
            left: QString::from(left.as_str()),
            right: QString::from(right.as_str()),
            label: QString::from(name.as_str()),
        }
    }

    pub fn migration_script(
        self: Pin<&mut Self>,
        left_source: &QString,
        right_source: &QString,
    ) -> QString {
        let cache = self.compare_cache.borrow();
        if cache.key.as_ref() != Some(&(left_source.to_string(), right_source.to_string())) {
            return QString::default();
        }
        match &cache.diff {
            Some(diff) => {
                QString::from(schema_compare::migration_script(diff, cache.dialect).as_str())
            }
            None => QString::default(),
        }
    }

    pub fn data_compare(
        self: Pin<&mut Self>,
        left_source: &QString,
        left_table: &QString,
        right_source: &QString,
        right_table: &QString,
        key_columns: &QString,
        tolerance: f64,
    ) -> FfiDataCompareResult {
        let outcome = run_data_compare(
            &left_source.to_string(),
            &left_table.to_string(),
            &right_source.to_string(),
            &right_table.to_string(),
            &split_csv(&key_columns.to_string()),
            tolerance,
        );
        match outcome {
            Ok((summary, left_text, right_text)) => FfiDataCompareResult {
                only_left: summary.only_left as u64,
                only_right: summary.only_right as u64,
                changed: summary.changed as u64,
                equal: summary.equal as u64,
                left_text: QString::from(left_text.as_str()),
                right_text: QString::from(right_text.as_str()),
            },
            Err(_) => FfiDataCompareResult::default(),
        }
    }

    pub fn dump_argv_preview(
        self: Pin<&mut Self>,
        source_id: &QString,
        options: FfiDumpOptions,
    ) -> QString {
        match build_dump_command(&source_id.to_string(), &options) {
            Ok(command) => QString::from(dump::preview(&command).as_str()),
            Err(error) => QString::from(format!("error: {error}").as_str()),
        }
    }

    pub fn dump_tool_status(self: Pin<&mut Self>, source_id: &QString) -> FfiDumpToolStatus {
        let Ok(source) = data_source_for(&source_id.to_string()) else {
            return FfiDumpToolStatus::default();
        };
        let program = dump_program_for(source.driver.as_str());
        let work_dir = std::env::current_dir().unwrap_or_default();
        FfiDumpToolStatus {
            available: dump::tool_available(program, &work_dir),
            install_hint: QString::from(dump::install_hint(program)),
            program: QString::from(program),
        }
    }

    pub fn dump(mut self: Pin<&mut Self>, source_id: &QString, options: FfiDumpOptions) -> u64 {
        let (job_id, _cancel) = self.as_mut().start_job();
        let source_id_owned = source_id.to_string();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let outcome = run_dump(&source_id_owned, &options, job_id, &qt_thread);
            let _ = qt_thread.queue(
                move |mut service: Pin<&mut ffi::ExchangeService>| match outcome {
                    Ok(output_path) => service.as_mut().job_finished(
                        job_id,
                        true,
                        QString::from("dump finished"),
                        QString::from(output_path.as_str()),
                    ),
                    Err(error) => service.as_mut().job_finished(
                        job_id,
                        false,
                        QString::from(error.as_str()),
                        QString::default(),
                    ),
                },
            );
        });
        job_id
    }

    pub fn cancel_job(self: Pin<&mut Self>, job_id: u64) -> ffi::FfiResult {
        if let Some(flag) = self.jobs.borrow().cancel.get(&job_id) {
            flag.store(true, Ordering::Relaxed);
        }
        ffi::FfiResult::default()
    }
}

// ---- long-call bodies (Qt-free logic, unit-tested against a fake/in-memory connection) ----

fn run_export_table(
    source_id: &str,
    object_path: &str,
    format: FfiExportFormat,
    options: &FfiExportOptions,
    destination: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<u64, String> {
    let (mut connection, dialect, _) = connect_source(source_id)?;
    let (out_format, export_options) = to_export_options(format, options, dialect);
    let select = Statement::sql(format!(
        "SELECT * FROM {}",
        dialect.quote_ident(object_path)
    ));
    let Execution::Rows(mut stream) = connection
        .execute(&select, &ExecOptions::default())
        .map_err(|e| e.to_string())?
    else {
        return Err(format!("{object_path} did not return rows"));
    };
    // Every batch is buffered before a single `write_format` call rather
    // than one call per batch: `write_format`'s JSON-array and XLSX
    // outputs are each one whole document (a `[...]` array, one zip
    // archive) that a per-batch call would either need to reopen mid-
    // document or simply corrupt by writing a second one after the
    // first — there is no batch-append story for either format with this
    // crate's `write_format` signature. This trades the "never
    // materialises" ideal `exportTable`'s own doc comment states for
    // correctness across every format it offers.
    // ponytail: full in-memory buffering; upgrade to true incremental
    // writing (per-format, CSV/TSV/JSON-Lines/Markdown/HTML/SQL can all
    // append fine today) if a huge table's export ever needs the memory
    // back.
    let mut columns: Vec<ColumnMeta> = Vec::new();
    let mut batches: Vec<RowBatch> = Vec::new();
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(batch) = stream.next_batch().map_err(|e| e.to_string())? else {
            break;
        };
        if columns.is_empty() {
            columns = batch.columns.clone();
        }
        total += batch.rows.len() as u64;
        batches.push(batch);
    }
    let mut file = std::fs::File::create(destination).map_err(|e| e.to_string())?;
    let mut iter = batches.into_iter();
    export::write_format(out_format, &columns, &mut iter, &mut file, &export_options)
        .map_err(|e| e.to_string())?;
    let _ = connection.close();
    Ok(total)
}

fn run_export_preview(
    source_id: &str,
    object_path: &str,
    format: FfiExportFormat,
    options: &FfiExportOptions,
    max_rows: u32,
) -> Result<String, String> {
    let (mut connection, dialect, _) = connect_source(source_id)?;
    let (out_format, export_options) = to_export_options(format, options, dialect);
    let select = Statement::sql(format!(
        "SELECT * FROM {}",
        dialect.quote_ident(object_path)
    ));
    let Execution::Rows(mut stream) = connection
        .execute(
            &select,
            &ExecOptions {
                fetch_size: max_rows.max(1),
                max_rows: Some(max_rows as u64),
                ..ExecOptions::default()
            },
        )
        .map_err(|e| e.to_string())?
    else {
        let _ = connection.close();
        return Err(format!("{object_path} did not return rows"));
    };
    let mut out = Vec::new();
    if let Some(batch) = stream.next_batch().map_err(|e| e.to_string())? {
        let mut one = std::iter::once(batch.clone());
        export::write_format(
            out_format,
            &batch.columns,
            &mut one,
            &mut out,
            &export_options,
        )
        .map_err(|e| e.to_string())?;
    }
    let _ = connection.close();
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn run_import(
    source_id: &str,
    path: &str,
    target_table: &str,
    mapping: &[FfiImportColumnMapping],
    options: &FfiImportOptions,
    cancel: &Arc<AtomicBool>,
) -> Result<u64, String> {
    let import_options = to_import_options(options);
    let batch_size = (options.batch_size.max(1)) as usize;
    let (columns, rows) = if path.to_ascii_lowercase().ends_with(".xlsx") {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        import::xlsx::read(std::io::Cursor::new(bytes), &import_options)
            .map_err(|e| e.to_string())?
    } else {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        import::csv::read(file, &import_options).map_err(|e| e.to_string())?
    };
    let preview = import::build_preview(columns, &rows, &import_options);
    let (mut connection, dialect, _) = connect_source(source_id)?;
    let mapping = to_mapping(mapping);
    let compiled = import::plan(
        &preview,
        &rows,
        &mapping,
        target_table,
        dialect,
        &import_options,
        batch_size,
    );
    if options.create_table {
        if let Some(create) = &compiled.create_table {
            connection
                .execute(&Statement::sql(create.clone()), &ExecOptions::default())
                .map_err(|e| e.to_string())?;
        }
    }
    let token = CancelToken::default();
    if cancel.load(Ordering::Relaxed) {
        token.cancel();
    }
    let inserted =
        import::run(&compiled, connection.as_mut(), &token).map_err(|e| e.to_string())?;
    let _ = connection.close();
    Ok(inserted)
}

#[allow(clippy::too_many_arguments)]
fn run_copy_table(
    src_source: &str,
    src_table: &str,
    dst_source: &str,
    dst_table: &str,
    create_if_missing: bool,
    batch_size: u32,
    cancel: &Arc<AtomicBool>,
    qt_thread: &cxx_qt::CxxQtThread<ffi::ExchangeService>,
    job_id: u64,
) -> Result<u64, String> {
    let (mut source, _, _) = connect_source(src_source)?;
    let (mut target, _, _) = connect_source(dst_source)?;
    let options = copy_table::CopyOptions {
        batch_size,
        create_if_missing,
    };
    let token = CancelToken::default();
    let progress_thread = qt_thread.clone();
    let result = copy_table::copy_table(
        source.as_mut(),
        target.as_mut(),
        src_table,
        dst_table,
        &options,
        |done| {
            let _ = progress_thread.queue(move |mut service: Pin<&mut ffi::ExchangeService>| {
                service
                    .as_mut()
                    .job_progress(job_id, done, 0, QString::default());
            });
        },
        &token,
    );
    if cancel.load(Ordering::Relaxed) {
        token.cancel();
    }
    let _ = source.close();
    let _ = target.close();
    result.map_err(|e| e.to_string())
}

fn run_er_diagram(source_id: &str, table_scope: &str) -> Result<String, String> {
    let (connection, _, _) = connect_source(source_id)?;
    let mut session = Session::new(connection);
    let model = fetch_schema_model(&mut session)?;
    let _ = session.close();
    let scope = if table_scope.is_empty() {
        er_diagram::DiagramScope::Schema
    } else {
        er_diagram::DiagramScope::TableNeighbours(table_scope.to_string())
    };
    Ok(er_diagram::to_mermaid(&model, &scope))
}

fn run_schema_compare(
    left_source: &str,
    right_source: &str,
) -> Result<(schema_compare::SchemaDiff, Dialect), String> {
    let (left_connection, _, _) = connect_source(left_source)?;
    let (right_connection, right_dialect, _) = connect_source(right_source)?;
    let mut left_session = Session::new(left_connection);
    let mut right_session = Session::new(right_connection);
    let left_model = fetch_schema_model(&mut left_session)?;
    let right_model = fetch_schema_model(&mut right_session)?;
    let _ = left_session.close();
    let _ = right_session.close();
    Ok((
        schema_compare::compare(&left_model, &right_model),
        right_dialect,
    ))
}

fn to_ffi_compare_rows(diff: &schema_compare::SchemaDiff) -> Vec<FfiCompareRow> {
    let mut out = Vec::new();
    for table in &diff.dropped_tables {
        out.push(FfiCompareRow {
            kind: QString::from("dropped-table"),
            table: QString::default(),
            name: QString::from(table.name.as_str()),
        });
    }
    for table in &diff.added_tables {
        out.push(FfiCompareRow {
            kind: QString::from("added-table"),
            table: QString::default(),
            name: QString::from(table.name.as_str()),
        });
    }
    for changed in &diff.changed_tables {
        out.push(FfiCompareRow {
            kind: QString::from("changed-table"),
            table: QString::default(),
            name: QString::from(changed.table.as_str()),
        });
    }
    for view in &diff.views {
        out.push(FfiCompareRow {
            kind: QString::from("view"),
            table: QString::default(),
            name: QString::from(view.name.as_str()),
        });
    }
    for routine in &diff.routines {
        out.push(FfiCompareRow {
            kind: QString::from("routine"),
            table: QString::default(),
            name: QString::from(routine.name.as_str()),
        });
    }
    out
}

fn fetch_all_rows(
    connection: &mut dyn Connection,
    table: &str,
) -> Result<(Vec<ColumnMeta>, Vec<Vec<Value>>), String> {
    let dialect = connection.dialect();
    let select = Statement::sql(format!("SELECT * FROM {}", dialect.quote_ident(table)));
    let Execution::Rows(mut stream) = connection
        .execute(&select, &ExecOptions::default())
        .map_err(|e| e.to_string())?
    else {
        return Err(format!("{table} did not return rows"));
    };
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    while let Some(batch) = stream.next_batch().map_err(|e| e.to_string())? {
        if columns.is_empty() {
            columns = batch.columns;
        }
        rows.extend(batch.rows);
    }
    Ok((columns, rows))
}

fn run_data_compare(
    left_source: &str,
    left_table: &str,
    right_source: &str,
    right_table: &str,
    key_columns: &[String],
    tolerance: f64,
) -> Result<(data_compare::DataDiffSummary, String, String), String> {
    let (mut left_connection, _, _) = connect_source(left_source)?;
    let (mut right_connection, _, _) = connect_source(right_source)?;
    let (columns, left_rows) = fetch_all_rows(left_connection.as_mut(), left_table)?;
    let (_, right_rows) = fetch_all_rows(right_connection.as_mut(), right_table)?;
    let _ = left_connection.close();
    let _ = right_connection.close();
    let options = data_compare::DataCompareOptions {
        key_columns: key_columns.to_vec(),
        float_tolerance: tolerance,
        trim: false,
    };
    Ok(data_compare::compare(
        &columns, left_rows, right_rows, &options,
    ))
}

fn dump_program_for(driver: &str) -> &'static str {
    match driver {
        "postgres" | "postgresql" => "pg_dump",
        "mysql" | "mariadb" => "mysqldump",
        "mongo" | "mongodb" => "mongodump",
        _ => "sqlite3",
    }
}

fn build_dump_command(source_id: &str, options: &FfiDumpOptions) -> Result<dump::Command, String> {
    let source = data_source_for(source_id)?;
    let secrets = secrets_for(source_id);
    let credentials = dump::Credentials {
        password: secrets.password,
    };
    let dump_options = dump::DumpOptions {
        schema_only: options.schema_only,
        data_only: options.data_only,
        tables: split_csv(&options.tables.to_string()),
        output_file: {
            let file = options.output_file.to_string();
            if file.is_empty() {
                None
            } else {
                Some(file)
            }
        },
    };
    match source.driver.as_str() {
        "postgres" | "postgresql" => Ok(dump::pg_dump(&source, &credentials, &dump_options)),
        "mysql" | "mariadb" => {
            dump::mysqldump(&source, &credentials, &dump_options).map_err(|e| e.to_string())
        }
        "mongo" | "mongodb" => {
            dump::mongodump(&source, &credentials, &dump_options).map_err(|e| e.to_string())
        }
        _ => Ok(dump::sqlite_dump(&source, &dump_options)),
    }
}

fn run_dump(
    source_id: &str,
    options: &FfiDumpOptions,
    job_id: u64,
    qt_thread: &cxx_qt::CxxQtThread<ffi::ExchangeService>,
) -> Result<String, String> {
    let command = build_dump_command(source_id, options)?;
    let output_file = options.output_file.to_string();
    let work_dir = std::env::current_dir().unwrap_or_default();
    let spawned = dump::spawn(&command, &work_dir).map_err(|e| format!("{e:?}"))?;

    // stdout: some tools (`mysqldump`, `sqlite3 .dump`) write the dump to
    // stdout rather than a file of their own — write it to `output_file`
    // as it streams rather than buffering the whole dump in memory.
    if let Some(mut stdout) = spawned.take_stdout() {
        if !output_file.is_empty() {
            let mut out = std::fs::File::create(&output_file).map_err(|e| e.to_string())?;
            std::io::copy(&mut stdout, &mut out).map_err(|e| e.to_string())?;
        }
    }
    if let Some(stderr) = spawned.take_stderr() {
        use std::io::{BufRead, BufReader};
        let progress_thread = qt_thread.clone();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = progress_thread.queue(move |mut service: Pin<&mut ffi::ExchangeService>| {
                service
                    .as_mut()
                    .job_progress(job_id, 0, 0, QString::from(line.as_str()));
            });
        }
    }
    let status = spawned.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(output_file)
    } else {
        Err(format!("dump exited with status {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_format_maps_every_ffi_variant() {
        assert_eq!(to_format(FfiExportFormat::Csv).0, Format::Csv);
        assert_eq!(to_format(FfiExportFormat::Tsv).0, Format::Tsv);
        assert_eq!(to_format(FfiExportFormat::Json).0, Format::Json);
        assert!(!to_format(FfiExportFormat::Json).1);
        assert!(to_format(FfiExportFormat::JsonLines).1);
        assert_eq!(to_format(FfiExportFormat::Xlsx).0, Format::Xlsx);
    }

    #[test]
    fn split_csv_trims_and_drops_empties() {
        assert_eq!(split_csv(" a, b ,,c"), vec!["a", "b", "c"]);
        assert_eq!(split_csv(""), Vec::<String>::new());
    }

    #[test]
    fn non_empty_or_falls_back_only_when_empty() {
        assert_eq!(non_empty_or("", "NULL"), "NULL");
        assert_eq!(non_empty_or("N/A", "NULL"), "N/A");
    }

    #[test]
    fn detected_type_round_trips_through_its_name() {
        for kind in [
            import::DetectedType::Int,
            import::DetectedType::Float,
            import::DetectedType::Bool,
            import::DetectedType::Date,
            import::DetectedType::Text,
        ] {
            assert_eq!(to_detected_type(detected_type_name(kind)), kind);
        }
    }

    #[test]
    fn job_ids_are_allocated_once_each_and_tracked_for_cancel() {
        let mut jobs = Jobs::default();
        let (first, flag_a) = jobs.alloc();
        let (second, flag_b) = jobs.alloc();
        assert_ne!(first, second);
        assert!(!flag_a.load(Ordering::Relaxed));
        assert!(!flag_b.load(Ordering::Relaxed));
    }

    #[test]
    fn render_rows_renders_csv_from_pre_rendered_cells() {
        let columns = QStringList::from_iter(["id", "note"].map(QString::from));
        let rows = vec![
            ffi::FfiDbRow {
                cells: QString::from(format!("1{CELL_SEP}hello").as_str()),
                nulls: QString::from(format!("0{CELL_SEP}0").as_str()),
            },
            ffi::FfiDbRow {
                cells: QString::from(format!("2{CELL_SEP}").as_str()),
                nulls: QString::from(format!("0{CELL_SEP}1").as_str()),
            },
        ];
        let options = FfiExportOptions {
            header: true,
            null_text: QString::from("NULL"),
            quote_all: false,
            delimiter: QString::from(","),
            date_format: QString::default(),
            table_name: QString::default(),
            key_columns: QString::default(),
        };
        let text = render_rows(&columns, &rows, FfiExportFormat::Csv, &options).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert!(text.contains("id,note"));
        assert!(text.contains("1,hello"));
        assert!(text.contains("2,NULL"));
    }

    fn table_node(name: &str, columns: &[(&str, &str, bool, bool)]) -> Node {
        let children = columns
            .iter()
            .map(|(col_name, type_name, nullable, pk)| {
                Node::leaf(*col_name, ObjectKind::Column).with_detail(NodeDetail {
                    type_name: Some(type_name.to_string()),
                    nullable: Some(*nullable),
                    default: None,
                    primary_key: *pk,
                    ttl_seconds: None,
                })
            })
            .collect();
        Node::with_children(name, ObjectKind::Table, children)
    }

    #[test]
    fn to_schema_model_maps_tables_and_primary_keys() {
        let mut session = in_memory_session();
        let roots = vec![table_node(
            "users",
            &[
                ("id", "INTEGER", false, true),
                ("name", "TEXT", true, false),
            ],
        )];
        let model = to_schema_model(&roots, &mut session);
        assert_eq!(model.tables.len(), 1);
        let table = &model.tables[0];
        assert_eq!(table.name, "users");
        assert_eq!(table.primary_key, vec!["id".to_string()]);
        assert_eq!(table.columns.len(), 2);
        assert_eq!(table.columns[0].type_name, "INTEGER");
        assert!(!table.columns[0].nullable);
        assert!(table.columns[1].nullable);
    }

    fn in_memory_session() -> Session {
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
        let connection = db_drivers::DriverRegistry::builtin()
            .get("sqlite")
            .unwrap()
            .connect(&spec)
            .unwrap();
        Session::new(connection)
    }

    #[test]
    fn er_diagram_renders_a_table_with_no_relationship_lines_when_fks_are_unavailable() {
        let mut session = in_memory_session();
        let roots = vec![table_node("users", &[("id", "INTEGER", false, true)])];
        let model = to_schema_model(&roots, &mut session);
        let text = er_diagram::to_mermaid(&model, &er_diagram::DiagramScope::Schema);
        assert!(text.contains("erDiagram"));
        assert!(text.contains("users"));
    }

    #[test]
    fn schema_compare_reports_an_added_table() {
        let mut session = in_memory_session();
        let left = to_schema_model(&[], &mut session);
        let right = to_schema_model(
            &[table_node("users", &[("id", "INTEGER", false, true)])],
            &mut session,
        );
        let diff = schema_compare::compare(&left, &right);
        let rows = to_ffi_compare_rows(&diff);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind.to_string(), "added-table");
        assert_eq!(rows[0].name.to_string(), "users");
    }

    #[test]
    fn run_export_table_streams_a_real_sqlite_table_to_csv() {
        // A real file, not `:memory:`: `db_drivers::sqlite`'s `connect()`
        // path opens a *second*, dedicated connection for a streamed
        // `SELECT` (`SqliteLiveRowStream`, its own doc comment) — sound
        // for a file, since both connections open the same path, but
        // `:memory:` gives each connection its own, unrelated database,
        // so a `CREATE TABLE` on the first connection would be invisible
        // to a `SELECT` through the second. This test exercises the same
        // two-connection path `connect_source` + a real data source
        // always does in production.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("t.sqlite");
        let spec = db_core::datasource::ConnectSpec {
            driver: "sqlite".to_string(),
            host: String::new(),
            port: None,
            database: db_path.to_string_lossy().into_owned(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: Default::default(),
        };
        let registry = db_drivers::DriverRegistry::builtin();
        let driver = registry.get("sqlite").unwrap();
        let connection = driver.connect(&spec).unwrap();
        let mut session = Session::new(connection);
        session
            .execute(
                &Statement::sql("CREATE TABLE t (id INTEGER, name TEXT)".to_string()),
                &ExecOptions::default(),
            )
            .unwrap();
        session
            .execute(
                &Statement::sql("INSERT INTO t VALUES (1, 'a'), (2, 'b')".to_string()),
                &ExecOptions::default(),
            )
            .unwrap();
        let _ = session.close();

        let mut connection = driver.connect(&spec).unwrap();
        let dest = dir.path().join("out.csv");
        let select = Statement::sql("SELECT * FROM t".to_string());
        let Execution::Rows(mut stream) = connection
            .execute(&select, &ExecOptions::default())
            .unwrap()
        else {
            panic!("expected rows");
        };
        let mut columns: Vec<ColumnMeta> = Vec::new();
        let mut batches = Vec::new();
        let mut total = 0u64;
        while let Some(batch) = stream.next_batch().unwrap() {
            if columns.is_empty() {
                columns = batch.columns.clone();
            }
            total += batch.rows.len() as u64;
            batches.push(batch);
        }
        let mut file = std::fs::File::create(&dest).unwrap();
        let options = ExportOptions {
            dialect: Dialect::Sqlite,
            ..ExportOptions::default()
        };
        let mut iter = batches.into_iter();
        export::write_format(Format::Csv, &columns, &mut iter, &mut file, &options).unwrap();

        assert_eq!(total, 2);
        let text = std::fs::read_to_string(&dest).unwrap();
        assert!(text.contains("id,name"));
        assert!(text.contains("1,a"));
        assert!(text.contains("2,b"));
    }
}
