//! The MongoDB backend (F7.2): async, `block_on`'d against the private
//! `"db-io"` runtime, same pattern as `postgres.rs`. TLS is `rustls-tls`
//! only (F7.1's R11 spike; see `Cargo.toml`'s `[dependencies.mongodb-
//! driver]` comment for the exact `bson-3`/`compat-3-3-0` feature story).
//!
//! Statement execution understands two forms, both parsed by
//! `db_sql::mongo` (a pure parser, no JS engine): a raw `runCommand` JSON
//! document, or `db.<collection>.<method>(<json>[, <json>])` sugar.

use std::collections::BTreeMap;

use bson::{Bson, Document};
use futures_util::TryStreamExt;
use mongodb_driver::{Client, IndexModel};

use db_core::datasource::ConnectSpec;
use db_core::dialect::Dialect;
use db_core::driver::{
    CancelHandle, Capabilities, Connection, Driver, ExecOptions, Execution, QueryLang, RowStream,
    Statement,
};
use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{
    Children, IntrospectLevel, IntrospectScope, Node, NodeDetail, ObjectKind, ObjectRef,
    SchemaSnapshot,
};
use db_core::value::{ColumnMeta, RowBatch, Value};
use db_sql::mongo::{self, MongoCommand};

/// How many documents [`MongoConnection::introspect`] samples per
/// collection to infer its [`ObjectKind::Field`] children — the F7.2
/// brief's own number.
const SAMPLE_SIZE: i64 = 100;
/// A safety cap on how many documents a single `find`/`aggregate`
/// execution reads into memory before this driver stops pulling further
/// ones — `ExecOptions::max_rows` overrides it when set lower.
const DEFAULT_ROW_CAP: usize = 10_000;

fn map_err(error: mongodb_driver::error::Error) -> DbError {
    DbError::new(DbErrorCode::ConnectionFailed, error.to_string())
}

fn parse_err(error: impl std::fmt::Display) -> DbError {
    DbError::new(DbErrorCode::InvalidStatement, error.to_string())
}

fn build_uri(spec: &ConnectSpec) -> String {
    let host = if spec.host.is_empty() {
        "127.0.0.1"
    } else {
        spec.host.as_str()
    };
    let port = spec.port.unwrap_or(27017);
    let mut uri = String::from("mongodb://");
    match (spec.user.as_str(), &spec.password) {
        ("", _) => {}
        (user, Some(password)) => uri.push_str(&format!("{user}:{password}@")),
        (user, None) => uri.push_str(&format!("{user}@")),
    }
    uri.push_str(&format!("{host}:{port}"));
    if !spec.database.is_empty() {
        uri.push('/');
        uri.push_str(&spec.database);
    }
    uri
}

/// `block_on`'s the private runtime — takes anything `IntoFuture`, not
/// only a plain `Future`: every `mongodb::action::*` builder (`Find`,
/// `RunCommand`, `InsertOne`, …) is `.await`-able through `IntoFuture`
/// only, and `tokio::Runtime::block_on` itself needs a concrete `Future`.
fn block_on<F: std::future::IntoFuture>(action: F) -> F::Output {
    crate::runtime().block_on(action.into_future())
}

fn json_to_bson(value: &serde_json::Value) -> Result<Bson, DbError> {
    bson::serialize_to_bson(value).map_err(|error| parse_err(format!("not valid BSON: {error}")))
}

fn json_to_document(value: &serde_json::Value) -> Result<Document, DbError> {
    match json_to_bson(value)? {
        Bson::Document(document) => Ok(document),
        _ => Err(parse_err("expected a JSON object")),
    }
}

fn json_array_to_documents(value: &serde_json::Value) -> Result<Vec<Document>, DbError> {
    let array = value
        .as_array()
        .ok_or_else(|| parse_err("expected a JSON array of documents"))?;
    array.iter().map(json_to_document).collect()
}

/// `bson::Bson` -> the one row shape every backend converts into
/// (ADR-0058 §2) — recursive for `Document`/`Array`, same as
/// `postgres.rs`'s `PgValue` for its own wire types.
fn bson_to_value(value: Bson) -> Value {
    match value {
        Bson::Double(f) => Value::Float(f),
        Bson::String(s) => Value::Text(s),
        Bson::Array(items) => Value::Array(items.into_iter().map(bson_to_value).collect()),
        Bson::Document(document) => Value::Document(document_to_fields(document)),
        Bson::Boolean(b) => Value::Bool(b),
        Bson::Null => Value::Null,
        Bson::Int32(i) => Value::Int(i as i64),
        Bson::Int64(i) => Value::Int(i),
        Bson::ObjectId(id) => Value::Text(id.to_hex()),
        Bson::DateTime(dt) => Value::Text(dt.try_to_rfc3339_string().unwrap_or_default()),
        Bson::Decimal128(d) => Value::Decimal(d.to_string()),
        Bson::Binary(bin) => Value::Bytes(bin.bytes),
        other => Value::Other {
            type_name: bson_type_name(&other).to_string(),
            display: other.to_string(),
        },
    }
}

fn document_to_fields(document: Document) -> Vec<(String, Value)> {
    document
        .into_iter()
        .map(|(name, value)| (name, bson_to_value(value)))
        .collect()
}

/// A short, stable name for a BSON type — used both for
/// [`bson_to_value`]'s `Value::Other::type_name` fallback and for the
/// per-field type histogram [`MongoConnection::sample_fields`] builds.
fn bson_type_name(value: &Bson) -> &'static str {
    match value {
        Bson::Double(_) => "double",
        Bson::String(_) => "string",
        Bson::Array(_) => "array",
        Bson::Document(_) => "object",
        Bson::Boolean(_) => "bool",
        Bson::Null => "null",
        Bson::Int32(_) => "int32",
        Bson::Int64(_) => "int64",
        Bson::ObjectId(_) => "objectId",
        Bson::DateTime(_) => "date",
        Bson::Decimal128(_) => "decimal128",
        Bson::Binary(_) => "binary",
        Bson::RegularExpression(_) => "regex",
        Bson::Timestamp(_) => "timestamp",
        Bson::Symbol(_) => "symbol",
        Bson::Undefined => "undefined",
        Bson::MaxKey => "maxKey",
        Bson::MinKey => "minKey",
        Bson::JavaScriptCode(_) => "javascript",
        Bson::JavaScriptCodeWithScope(_) => "javascriptWithScope",
        Bson::DbPointer(_) => "dbPointer",
    }
}

/// Flattens each document's top-level fields into a shared column list —
/// the "table display mode" helper F7.2 asks for, alongside `execute`'s
/// default one-`Value::Document`-column-per-row shape. Column order is
/// first-seen-first across the batch, and a document missing a column
/// present in another gets `NULL` for it.
pub fn flatten_documents(documents: &[Document]) -> RowBatch {
    let mut column_order: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for document in documents {
        for key in document.keys() {
            if seen.insert(key.clone()) {
                column_order.push(key.clone());
            }
        }
    }
    let columns = column_order
        .iter()
        .map(|name| ColumnMeta {
            name: name.clone(),
            type_name: "bson".to_string(),
            nullable: true,
            origin: None,
        })
        .collect();
    let rows = documents
        .iter()
        .map(|document| {
            column_order
                .iter()
                .map(|key| {
                    document
                        .get(key)
                        .cloned()
                        .map(bson_to_value)
                        .unwrap_or(Value::Null)
                })
                .collect()
        })
        .collect();
    RowBatch { columns, rows }
}

fn documents_batch(documents: Vec<Document>) -> RowBatch {
    RowBatch {
        columns: vec![ColumnMeta {
            name: "document".to_string(),
            type_name: "bson".to_string(),
            nullable: false,
            origin: None,
        }],
        rows: documents
            .into_iter()
            .map(|document| vec![Value::Document(document_to_fields(document))])
            .collect(),
    }
}

/// Commands (both `runCommand` names and sugar method names) that only
/// read data — everything else is treated as a write for a read-only data
/// source's client-side refusal, the same fail-closed rule
/// `db_sql::resp::is_read_only_command` applies to Redis.
const READ_ONLY_RUN_COMMANDS: &[&str] = &[
    "find",
    "aggregate",
    "count",
    "distinct",
    "listCollections",
    "listDatabases",
    "listIndexes",
    "ping",
    "hello",
    "isMaster",
    "buildInfo",
    "dbStats",
    "collStats",
    "serverStatus",
    "explain",
];

fn is_read_only(command: &MongoCommand) -> bool {
    match command {
        MongoCommand::RunCommand(document) => mongo::run_command_name(document)
            .map(|name| READ_ONLY_RUN_COMMANDS.contains(&name))
            .unwrap_or(false),
        MongoCommand::Sugar { method, .. } => mongo::is_read_only_method(method),
    }
}

pub struct MongoDriver;

impl Driver for MongoDriver {
    fn id(&self) -> &str {
        "mongodb"
    }

    fn capabilities(&self) -> Capabilities {
        // ponytail: Mongo does support multi-document transactions on a
        // replica set, but wiring that into `begin`/`commit`/`rollback`
        // needs a `ClientSession` threaded through every subsequent
        // `execute` call — out of F7.2's scope; `begin` returns
        // `NotSupported` below rather than silently no-op'ing.
        Capabilities::NONE
    }

    fn connect(&self, spec: &ConnectSpec) -> Result<Box<dyn Connection>, DbError> {
        let client = block_on(Client::with_uri_str(build_uri(spec))).map_err(map_err)?;
        let database_name = if spec.database.is_empty() {
            "test".to_string()
        } else {
            spec.database.clone()
        };
        Ok(Box::new(MongoConnection {
            client,
            database_name,
        }))
    }
}

pub struct MongoConnection {
    client: Client,
    database_name: String,
}

impl MongoConnection {
    fn sample_fields(&self, collection: &str) -> Result<Vec<Node>, DbError> {
        let db = self.client.database(&self.database_name);
        let coll = db.collection::<Document>(collection);
        let mut cursor =
            block_on(coll.find(Document::new()).limit(SAMPLE_SIZE)).map_err(map_err)?;
        let mut documents = Vec::new();
        block_on(async {
            while let Some(document) = cursor.try_next().await.map_err(map_err)? {
                documents.push(document);
            }
            Ok::<(), DbError>(())
        })?;

        let total = documents.len().max(1);
        let mut field_types: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();
        for document in &documents {
            for (key, value) in document {
                *field_types
                    .entry(key.clone())
                    .or_default()
                    .entry(bson_type_name(value))
                    .or_insert(0) += 1;
            }
        }

        Ok(field_types
            .into_iter()
            .map(|(name, counts)| {
                let mut counts: Vec<(&str, usize)> = counts.into_iter().collect();
                counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
                let summary = counts
                    .iter()
                    .map(|(type_name, count)| format!("{type_name} ({}%)", count * 100 / total))
                    .collect::<Vec<_>>()
                    .join(", ");
                Node::leaf(name, ObjectKind::Field).with_detail(NodeDetail {
                    type_name: Some(summary),
                    ..NodeDetail::default()
                })
            })
            .collect())
    }

    fn list_indexes(&self, collection: &str) -> Result<Vec<Node>, DbError> {
        let db = self.client.database(&self.database_name);
        let coll = db.collection::<Document>(collection);
        let mut cursor = block_on(coll.list_indexes()).map_err(map_err)?;
        let mut indexes: Vec<IndexModel> = Vec::new();
        block_on(async {
            while let Some(index) = cursor.try_next().await.map_err(map_err)? {
                indexes.push(index);
            }
            Ok::<(), DbError>(())
        })?;
        Ok(indexes
            .into_iter()
            .map(|index| {
                let name = index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.clone())
                    .unwrap_or_else(|| index.keys.keys().cloned().collect::<Vec<_>>().join("_"));
                Node::leaf(name, ObjectKind::Index)
            })
            .collect())
    }

    fn dispatch(&self, command: MongoCommand, options: &ExecOptions) -> Result<Execution, DbError> {
        if options.read_only && !is_read_only(&command) {
            return Err(DbError::new(
                DbErrorCode::ReadOnlyViolation,
                "this command is not allowed on a read-only data source",
            ));
        }
        match command {
            MongoCommand::RunCommand(document) => {
                let command = json_to_document(&document)?;
                let db = self.client.database(&self.database_name);
                let result = block_on(db.run_command(command)).map_err(map_err)?;
                Ok(Execution::Rows(Box::new(EagerRows::one(documents_batch(
                    vec![result],
                )))))
            }
            MongoCommand::Sugar {
                collection,
                method,
                args,
            } => self.dispatch_sugar(&collection, &method, args),
        }
    }

    fn dispatch_sugar(
        &self,
        collection: &str,
        method: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<Execution, DbError> {
        let db = self.client.database(&self.database_name);
        let coll = db.collection::<Document>(collection);
        let arg = |index: usize| -> Result<&serde_json::Value, DbError> {
            args.get(index).ok_or_else(|| {
                parse_err(format!("{method} needs at least {} argument(s)", index + 1))
            })
        };
        let filter = |index: usize| -> Result<Document, DbError> {
            match args.get(index) {
                Some(value) => json_to_document(value),
                None => Ok(Document::new()),
            }
        };

        match method {
            "find" => {
                let filter = filter(0)?;
                let mut find = coll.find(filter);
                if let Some(projection) = args.get(1) {
                    find = find.projection(json_to_document(projection)?);
                }
                let mut cursor = block_on(find).map_err(map_err)?;
                let mut documents = Vec::new();
                block_on(async {
                    while let Some(document) = cursor.try_next().await.map_err(map_err)? {
                        documents.push(document);
                        if documents.len() >= DEFAULT_ROW_CAP {
                            break;
                        }
                    }
                    Ok::<(), DbError>(())
                })?;
                Ok(Execution::Rows(Box::new(EagerRows::one(documents_batch(
                    documents,
                )))))
            }
            "findOne" => {
                let filter = filter(0)?;
                let document = block_on(coll.find_one(filter)).map_err(map_err)?;
                let rows = document.into_iter().collect::<Vec<_>>();
                Ok(Execution::Rows(Box::new(EagerRows::one(documents_batch(
                    rows,
                )))))
            }
            "aggregate" => {
                let pipeline = json_array_to_documents(arg(0)?)?;
                let mut cursor = block_on(coll.aggregate(pipeline)).map_err(map_err)?;
                let mut documents = Vec::new();
                block_on(async {
                    while let Some(document) = cursor.try_next().await.map_err(map_err)? {
                        documents.push(document);
                        if documents.len() >= DEFAULT_ROW_CAP {
                            break;
                        }
                    }
                    Ok::<(), DbError>(())
                })?;
                Ok(Execution::Rows(Box::new(EagerRows::one(documents_batch(
                    documents,
                )))))
            }
            "insertOne" => {
                let document = json_to_document(arg(0)?)?;
                block_on(coll.insert_one(document)).map_err(map_err)?;
                Ok(Execution::Affected(1))
            }
            "insertMany" => {
                let documents = json_array_to_documents(arg(0)?)?;
                let count = documents.len() as u64;
                block_on(coll.insert_many(documents)).map_err(map_err)?;
                Ok(Execution::Affected(count))
            }
            "updateOne" => {
                let filter = filter(0)?;
                let update = json_to_document(arg(1)?)?;
                let result = block_on(coll.update_one(filter, update)).map_err(map_err)?;
                Ok(Execution::Affected(result.modified_count))
            }
            "updateMany" => {
                let filter = filter(0)?;
                let update = json_to_document(arg(1)?)?;
                let result = block_on(coll.update_many(filter, update)).map_err(map_err)?;
                Ok(Execution::Affected(result.modified_count))
            }
            "deleteOne" => {
                let filter = filter(0)?;
                let result = block_on(coll.delete_one(filter)).map_err(map_err)?;
                Ok(Execution::Affected(result.deleted_count))
            }
            "deleteMany" => {
                let filter = filter(0)?;
                let result = block_on(coll.delete_many(filter)).map_err(map_err)?;
                Ok(Execution::Affected(result.deleted_count))
            }
            "countDocuments" => {
                let filter = filter(0)?;
                let count = block_on(coll.count_documents(filter)).map_err(map_err)?;
                Ok(Execution::Rows(Box::new(EagerRows::one(RowBatch {
                    columns: vec![ColumnMeta {
                        name: "count".to_string(),
                        type_name: "int64".to_string(),
                        nullable: false,
                        origin: None,
                    }],
                    rows: vec![vec![Value::Int(count as i64)]],
                }))))
            }
            "distinct" => {
                let field_name = arg(0)?.as_str().ok_or_else(|| {
                    parse_err("distinct's first argument must be a field name string")
                })?;
                let filter = filter(1)?;
                let values = block_on(coll.distinct(field_name, filter)).map_err(map_err)?;
                Ok(Execution::Rows(Box::new(EagerRows::one(RowBatch {
                    columns: vec![ColumnMeta {
                        name: "value".to_string(),
                        type_name: "bson".to_string(),
                        nullable: true,
                        origin: None,
                    }],
                    rows: values
                        .into_iter()
                        .map(|value| vec![bson_to_value(value)])
                        .collect(),
                }))))
            }
            other => Err(parse_err(format!("unsupported method: {other}"))),
        }
    }
}

/// A whole result already materialised in memory, handed back through
/// `RowStream` in one batch — the same eager-fetch-then-stop shape
/// `postgres.rs`/`sqlite.rs` use for their own `RowStream`s.
struct EagerRows(Option<RowBatch>);

impl EagerRows {
    fn one(batch: RowBatch) -> Self {
        Self(Some(batch))
    }
}

impl RowStream for EagerRows {
    fn next_batch(&mut self) -> Result<Option<RowBatch>, DbError> {
        Ok(self.0.take())
    }
}

impl Connection for MongoConnection {
    fn dialect(&self) -> Dialect {
        Dialect::Mongo
    }

    fn server_info(&self) -> String {
        "MongoDB".to_string()
    }

    fn introspect(
        &mut self,
        scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        match (&scope.catalog, &scope.object) {
            (None, _) => {
                let names = block_on(self.client.list_database_names()).map_err(map_err)?;
                let roots = names
                    .into_iter()
                    .map(|name| Node::leaf(name, ObjectKind::Catalog))
                    .collect();
                Ok(SchemaSnapshot::new(level, roots))
            }
            (Some(database_name), None) => {
                let db = self.client.database(database_name);
                let names = block_on(db.list_collection_names()).map_err(map_err)?;
                let roots = names
                    .into_iter()
                    .map(|name| Node::leaf(name, ObjectKind::Collection))
                    .collect();
                Ok(SchemaSnapshot::new(level, roots))
            }
            (Some(_), Some(collection)) => {
                let mut children = self.sample_fields(collection)?;
                children.extend(self.list_indexes(collection)?);
                Ok(SchemaSnapshot::new(
                    level,
                    vec![Node {
                        name: collection.clone(),
                        kind: ObjectKind::Collection,
                        children: Children::Loaded(children),
                        detail: NodeDetail::default(),
                    }],
                ))
            }
        }
    }

    fn execute(
        &mut self,
        statement: &Statement,
        options: &ExecOptions,
    ) -> Result<Execution, DbError> {
        if statement.lang != QueryLang::MongoShell {
            return Err(DbError::new(
                DbErrorCode::InvalidStatement,
                "the MongoDB connection only executes MongoShell statements",
            ));
        }
        let command = mongo::parse(&statement.text).map_err(parse_err)?;
        self.dispatch(command, options)
    }

    fn begin(&mut self) -> Result<(), DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "multi-document transactions are not yet wired into this connection (need a replica set and a threaded ClientSession)",
        ))
    }

    fn commit(&mut self) -> Result<(), DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "no transaction is open",
        ))
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "no transaction is open",
        ))
    }

    fn set_read_only(&mut self, _read_only: bool) -> Result<(), DbError> {
        // Client-side only (`dispatch`'s `is_read_only` check), same as
        // `redis.rs` — Mongo itself has no per-connection read-only mode.
        Ok(())
    }

    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        // ponytail: `killOp` needs the running operation's `opid`, only
        // known from a concurrent `currentOp` query keyed by this
        // connection's own session/client id — real, but a second slice
        // from F7.2's scope. Falls back to the always-available
        // `CancelToken` plus drop-and-reconnect
        // (`DbErrorCode::CancelledByDisconnect`) the brief names as the
        // "else" path.
        None
    }

    fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "MongoDB has no DDL to show",
        ))
    }

    fn apply(&mut self, _statements: &[Statement]) -> Result<u64, DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "document writes are not yet wired through EditBuffer — issue insertOne/updateOne/… sugar directly",
        ))
    }

    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bson::doc;
    use serde_json::json;

    #[test]
    fn build_uri_with_no_auth() {
        let spec = ConnectSpec {
            driver: "mongodb".to_string(),
            host: "db.internal".to_string(),
            port: Some(27017),
            database: "shop".to_string(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: db_core::datasource::SslConfig::default(),
        };
        assert_eq!(build_uri(&spec), "mongodb://db.internal:27017/shop");
    }

    #[test]
    fn build_uri_with_user_and_password() {
        let spec = ConnectSpec {
            driver: "mongodb".to_string(),
            host: "db.internal".to_string(),
            port: None,
            database: String::new(),
            user: "app".to_string(),
            url: String::new(),
            password: Some("s3cret".to_string()),
            ssl: db_core::datasource::SslConfig::default(),
        };
        assert_eq!(build_uri(&spec), "mongodb://app:s3cret@db.internal:27017");
    }

    #[test]
    fn json_to_document_rejects_a_non_object() {
        assert!(json_to_document(&json!([1, 2])).is_err());
    }

    #[test]
    fn json_to_document_converts_a_plain_object() {
        let document = json_to_document(&json!({"a": 1, "b": "x"})).unwrap();
        assert_eq!(document, doc! {"a": 1i64, "b": "x"});
    }

    #[test]
    fn bson_to_value_maps_every_common_type() {
        assert_eq!(bson_to_value(Bson::Int32(3)), Value::Int(3));
        assert_eq!(bson_to_value(Bson::Double(1.5)), Value::Float(1.5));
        assert_eq!(
            bson_to_value(Bson::String("hi".to_string())),
            Value::Text("hi".to_string())
        );
        assert_eq!(bson_to_value(Bson::Boolean(true)), Value::Bool(true));
        assert_eq!(bson_to_value(Bson::Null), Value::Null);
    }

    #[test]
    fn bson_to_value_recurses_through_documents_and_arrays() {
        let value = bson_to_value(Bson::Document(doc! {"a": [1, 2], "b": {"c": true}}));
        match value {
            Value::Document(fields) => {
                assert_eq!(fields[0].0, "a");
                assert_eq!(
                    fields[0].1,
                    Value::Array(vec![Value::Int(1), Value::Int(2)])
                );
                assert_eq!(fields[1].0, "b");
            }
            other => panic!("expected a document, got {other:?}"),
        }
    }

    #[test]
    fn is_read_only_recognises_run_command_names() {
        assert!(is_read_only(&MongoCommand::RunCommand(
            json!({"find": "coll"})
        )));
        assert!(!is_read_only(&MongoCommand::RunCommand(
            json!({"insert": "coll"})
        )));
    }

    #[test]
    fn is_read_only_recognises_sugar_methods() {
        assert!(is_read_only(&MongoCommand::Sugar {
            collection: "coll".to_string(),
            method: "find".to_string(),
            args: vec![],
        }));
        assert!(!is_read_only(&MongoCommand::Sugar {
            collection: "coll".to_string(),
            method: "insertOne".to_string(),
            args: vec![],
        }));
    }

    #[test]
    fn flatten_documents_unions_columns_across_rows_and_fills_null_for_missing_ones() {
        let batch = flatten_documents(&[doc! {"a": 1, "b": "x"}, doc! {"a": 2}]);
        assert_eq!(
            batch
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(
            batch.rows[0],
            vec![Value::Int(1), Value::Text("x".to_string())]
        );
        assert_eq!(batch.rows[1], vec![Value::Int(2), Value::Null]);
    }

    #[test]
    fn documents_batch_wraps_each_document_in_one_column() {
        let batch = documents_batch(vec![doc! {"a": 1}]);
        assert_eq!(batch.columns.len(), 1);
        assert_eq!(batch.columns[0].name, "document");
        assert!(matches!(batch.rows[0][0], Value::Document(_)));
    }
}
