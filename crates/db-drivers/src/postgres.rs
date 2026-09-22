//! The PostgreSQL backend (ADR-0058): async, `block_on`'d against the
//! private `"db-io"` runtime (`crate::runtime`) from whichever `std::thread`
//! calls it (`ui-shell`'s `SessionWorker`, in F1.6) — never the ambient
//! runtime, and this connection's own methods stay ordinary blocking calls
//! from every other crate's point of view (ADR-0058 §1).
//!
//! ## TLS: plaintext only in F1
//!
//! `ConnectSpec::ssl.mode` other than [`db_core::datasource::SslMode::
//! Disable`] returns [`DbErrorCode::NotSupported`] rather than either
//! silently connecting in plaintext or building a `rustls::ClientConfig`
//! this crate cannot yet correctly verify: ADR-0061 §2 rules out a
//! "connect over TLS but don't verify" mode entirely, and building
//! `VerifyCa`/`VerifyFull` properly needs a PEM parser and a root store
//! this F1 slice does not yet carry (`rustls-pemfile` is not in the
//! approved F1 dependency list). `tokio-postgres-rustls` stays a declared
//! dependency for the phase that adds it, so the seam this driver
//! implements does not need a breaking change to grow it in.
//! ponytail: TLS wiring deferred; add `rustls-pemfile` (or a hand-rolled
//! PEM reader) and thread `ca_file` into a `rustls::RootCertStore` when a
//! real Postgres server is available to verify the handshake against
//! (`docs/architecture/db-integration.md`'s `db-integration` feature).

use std::error::Error as StdError;
use std::pin::Pin;

use bytes::BytesMut;
use futures_util::{Stream, StreamExt};
use tokio_postgres::types::{FromSql, IsNull, ToSql, Type as PgType};
use tokio_postgres::{Client, NoTls};

use db_core::datasource::{ConnectSpec, SslMode};
use db_core::dialect::Dialect;
use db_core::driver::{
    CancelHandle, Capabilities, Connection, Driver, ExecOptions, Execution, RowStream, Statement,
};
use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{
    ConstraintKind, IndexDetail, IntrospectLevel, IntrospectScope, Node, NodeDetail, ObjectKind,
    ObjectRef, SchemaSnapshot,
};
use db_core::value::{ColumnMeta, RowBatch, Value};

const NAMES_QUERY: &str = include_str!("introspect/postgres_names.sql");
// `introspect` only fetches `IntrospectLevel::Names` in this F1 slice (the
// eager, cheap query the 5 000-table NFR needs); a table's own columns are
// fetched lazily on expand, a follow-up this fixture already documents the
// shape of — kept here (and its own test) so that follow-up is one
// query away, not a new SQL text to design from scratch.
#[allow(dead_code)]
const COLUMNS_QUERY: &str = include_str!("introspect/postgres_columns.sql");
/// `IntrospectLevel::Full`: one table's own constraints (F6c) —
/// `pg_constraint`/`pg_get_constraintdef`, fixture-tested below (no live
/// server needed for the query text itself; `db-integration`'s gated test
/// exercises it against a real server).
const CONSTRAINTS_QUERY: &str = include_str!("introspect/postgres_constraints.sql");
/// `IntrospectLevel::Full`: one table's own indexes (F6c) — `pg_index`.
const INDEXES_QUERY: &str = include_str!("introspect/postgres_indexes.sql");

/// Postgres's `pg_constraint.confupdtype`/`confdeltype` one-character
/// codes -> the SQL keyword `db_core::schema::ConstraintKind::ForeignKey`
/// carries, `None` for `'a'` (`NO ACTION`, the default — worth omitting
/// rather than repeating on every foreign key that never named one).
fn referential_action(code: &str) -> Option<String> {
    match code {
        "a" | "" => None,
        "r" => Some("RESTRICT".to_string()),
        "c" => Some("CASCADE".to_string()),
        "n" => Some("SET NULL".to_string()),
        "d" => Some("SET DEFAULT".to_string()),
        other => Some(other.to_string()),
    }
}

fn io_err(message: impl std::fmt::Display) -> DbError {
    DbError::new(DbErrorCode::ConnectionFailed, message.to_string())
}

/// Postgres's own binary `numeric` wire format: `ndigits`/`weight` (i16),
/// `sign`/`dscale` (u16), then `ndigits` base-10000 digit groups (i16
/// each). Decoded here rather than through a decimal crate — the same
/// "preserve the server's own text exactly" rule `Value::Decimal`'s doc
/// comment states, applied to the one type Postgres never sends as plain
/// UTF-8 text over the binary protocol.
fn decode_numeric(raw: &[u8]) -> Result<String, String> {
    if raw.len() < 8 {
        return Err("truncated numeric header".to_string());
    }
    let ndigits = u16::from_be_bytes([raw[0], raw[1]]) as usize;
    let weight = i16::from_be_bytes([raw[2], raw[3]]) as i32;
    let sign = u16::from_be_bytes([raw[4], raw[5]]);
    let dscale = u16::from_be_bytes([raw[6], raw[7]]) as usize;
    if sign == 0xC000 {
        return Ok("NaN".to_string());
    }
    if raw.len() < 8 + ndigits * 2 {
        return Err("truncated numeric digits".to_string());
    }
    let mut groups = Vec::with_capacity(ndigits);
    for i in 0..ndigits {
        let offset = 8 + i * 2;
        groups.push(i16::from_be_bytes([raw[offset], raw[offset + 1]]));
    }

    let padded: String = groups.iter().map(|g| format!("{g:04}")).collect();
    let mut chars: Vec<char> = padded.chars().collect();
    let point = (weight + 1) * 4;
    if point < 0 {
        let mut zeros: Vec<char> = std::iter::repeat_n('0', (-point) as usize).collect();
        zeros.extend(chars);
        chars = zeros;
    }
    let point = point.max(0) as usize;
    if point > chars.len() {
        chars.extend(std::iter::repeat_n('0', point - chars.len()));
    }
    let (int_chars, frac_chars) = chars.split_at(point);
    let int_trimmed = int_chars
        .iter()
        .collect::<String>()
        .trim_start_matches('0')
        .to_string();
    let int_final = if int_trimmed.is_empty() {
        "0"
    } else {
        &int_trimmed
    };
    let mut frac: String = frac_chars.iter().collect();
    if frac.len() < dscale {
        frac.push_str(&"0".repeat(dscale - frac.len()));
    } else {
        frac.truncate(dscale);
    }
    let sign_str = if sign == 0x4000 { "-" } else { "" };
    if dscale == 0 {
        Ok(format!("{sign_str}{int_final}"))
    } else {
        Ok(format!("{sign_str}{int_final}.{frac}"))
    }
}

/// A `FromSql` wrapper decoding straight into [`Value`] — the driver
/// boundary `ADR-0058` §2 describes ("every backend converts its own wire
/// format into this once").
struct PgValue(Value);

impl<'a> FromSql<'a> for PgValue {
    fn from_sql(ty: &PgType, raw: &'a [u8]) -> Result<Self, Box<dyn StdError + Sync + Send>> {
        let value = match *ty {
            PgType::BOOL => Value::Bool(raw.first().copied().unwrap_or(0) != 0),
            PgType::INT2 => Value::Int(i16::from_be_bytes(raw.try_into()?) as i64),
            PgType::INT4 => Value::Int(i32::from_be_bytes(raw.try_into()?) as i64),
            PgType::INT8 => Value::Int(i64::from_be_bytes(raw.try_into()?)),
            PgType::FLOAT4 => Value::Float(f32::from_be_bytes(raw.try_into()?) as f64),
            PgType::FLOAT8 => Value::Float(f64::from_be_bytes(raw.try_into()?)),
            PgType::TEXT | PgType::VARCHAR | PgType::BPCHAR | PgType::NAME => {
                Value::Text(std::str::from_utf8(raw)?.to_string())
            }
            PgType::BYTEA => Value::Bytes(raw.to_vec()),
            PgType::UUID => Value::Uuid(uuid::Uuid::from_slice(raw)?),
            PgType::JSON => Value::Json(std::str::from_utf8(raw)?.to_string()),
            // jsonb's wire format carries a one-byte version prefix before the JSON text.
            PgType::JSONB => {
                Value::Json(std::str::from_utf8(&raw[1.min(raw.len())..])?.to_string())
            }
            PgType::NUMERIC => Value::Decimal(
                decode_numeric(raw).map_err(Box::<dyn StdError + Sync + Send>::from)?,
            ),
            _ => Value::Other {
                type_name: ty.name().to_string(),
                display: String::from_utf8_lossy(raw).into_owned(),
            },
        };
        Ok(PgValue(value))
    }

    fn from_sql_null(_ty: &PgType) -> Result<Self, Box<dyn StdError + Sync + Send>> {
        Ok(PgValue(Value::Null))
    }

    fn accepts(_ty: &PgType) -> bool {
        true
    }
}

/// `Value` -> a bound parameter. Every value crosses as a parameter, never
/// interpolated (ADR-0061 §1).
/// ponytail: a `NULL` parameter's own Postgres type is inferred from
/// context by the extended protocol in the common case; an untyped `NULL`
/// with no other type hint nearby can occasionally need an explicit cast
/// on the caller's SQL side — a real Postgres server is needed to harden
/// this further (`db-integration`, not run in this sandbox).
#[derive(Debug)]
struct PgParam<'a>(&'a Value);

impl ToSql for PgParam<'_> {
    fn to_sql(
        &self,
        ty: &PgType,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
        match self.0 {
            Value::Null => Ok(IsNull::Yes),
            Value::Bool(b) => b.to_sql(ty, out),
            Value::Int(i) => i.to_sql(ty, out),
            Value::Float(f) => f.to_sql(ty, out),
            Value::Decimal(text) | Value::Text(text) | Value::Json(text) => text.to_sql(ty, out),
            Value::Bytes(bytes) => bytes.to_sql(ty, out),
            Value::Uuid(id) => id.to_sql(ty, out),
            Value::Date(date) => date.to_sql(ty, out),
            Value::Time(time) => time.to_sql(ty, out),
            Value::DateTime(dt) => dt.to_sql(ty, out),
            Value::DateTimeTz(dt) => dt.to_sql(ty, out),
            other => other.display(&Default::default()).to_sql(ty, out),
        }
    }

    fn accepts(_ty: &PgType) -> bool {
        true
    }

    tokio_postgres::types::to_sql_checked!();
}

pub struct PostgresDriver;

impl Driver for PostgresDriver {
    fn id(&self) -> &str {
        "postgresql"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::TRANSACTIONS
            | Capabilities::RETURNING
            | Capabilities::SERVER_SIDE_CANCEL
            | Capabilities::SCHEMAS_WITHIN_CATALOG
            | Capabilities::SAVEPOINTS
    }

    fn connect(&self, spec: &ConnectSpec) -> Result<Box<dyn Connection>, DbError> {
        if spec.ssl.mode != SslMode::Disable {
            return Err(DbError::new(
                DbErrorCode::NotSupported,
                "TLS is not yet implemented for PostgreSQL connections (F1 foundation) — use SSL mode \"disable\" for now",
            ));
        }
        let mut config = tokio_postgres::Config::new();
        config.host(&spec.host);
        if let Some(port) = spec.port {
            config.port(port);
        }
        if !spec.database.is_empty() {
            config.dbname(&spec.database);
        }
        if !spec.user.is_empty() {
            config.user(&spec.user);
        }
        if let Some(password) = &spec.password {
            config.password(password);
        }

        let (client, connection) = crate::runtime()
            .block_on(config.connect(NoTls))
            .map_err(io_err)?;
        // The connection object drives the actual socket I/O; it must keep
        // running for as long as `client` is used, the same background
        // task every tokio-postgres consumer spawns.
        crate::runtime().spawn(async move {
            let _ = connection.await;
        });
        Ok(Box::new(PostgresConnection { client }))
    }
}

pub struct PostgresConnection {
    client: Client,
}

/// Wraps `tokio_postgres::CancelToken` (the server-side cancel request,
/// ADR-0058 §3's "PG `cancel_query`") — a different type from
/// `db_core::driver::CancelToken`, which is the consumer-side atomic flag.
struct PgServerCancel(tokio_postgres::CancelToken);

impl CancelHandle for PgServerCancel {
    fn cancel(&self) -> Result<(), DbError> {
        crate::runtime()
            .block_on(self.0.cancel_query(NoTls))
            .map_err(io_err)
    }
}

/// A live `tokio_postgres::RowStream`, pulling up to `fetch_size` rows per
/// `next_batch` straight off the wire rather than draining the whole
/// result upfront (database-tools-plan F3d: the same NFR-breaking defect
/// F3c fixed for SQLite — see `database-tools.md` §4/§11).
///
/// Unlike SQLite's cursor, this needs no self-referential borrow or second
/// connection: `Client::query_raw` returns a `'static`, owned stream that
/// pages rows off the connection's own background I/O task, and
/// tokio-postgres pipelines concurrent commands on one `Client`
/// transparently — a second `execute` on the same `PostgresConnection`
/// while this stream is still parked mid-result is fine protocol-wise
/// (proven by `a_second_execute_does_not_disturb_an_open_stream`, gated
/// behind `db-integration`).
///
/// The wire `Row` -> `Value` mapping happens once, at construction, via
/// `.map()` on the raw stream — so this struct (and its tests) only ever
/// see `Vec<Value>`, never a `tokio_postgres::Row`, which has no public
/// constructor and so cannot be faked in a unit test.
struct PostgresRowStream {
    columns: Vec<ColumnMeta>,
    fetch_size: usize,
    max_rows: Option<u64>,
    fetched: u64,
    stream: Pin<Box<dyn Stream<Item = Result<Vec<Value>, DbError>> + Send>>,
    done: bool,
}

impl PostgresRowStream {
    fn new(
        columns: Vec<ColumnMeta>,
        options: &ExecOptions,
        stream: Pin<Box<dyn Stream<Item = Result<Vec<Value>, DbError>> + Send>>,
    ) -> Self {
        Self {
            columns,
            fetch_size: options.fetch_size.max(1) as usize,
            max_rows: options.max_rows,
            fetched: 0,
            stream,
            done: false,
        }
    }

    /// Pulls at most `fetch_size` rows off the live stream, honouring
    /// `max_rows`. Returns the rows plus whether the stream (or the
    /// `max_rows` budget) is now exhausted.
    async fn pull_batch(&mut self) -> Result<(Vec<Vec<Value>>, bool), DbError> {
        let mut out = Vec::with_capacity(self.fetch_size);
        let mut exhausted = false;
        while out.len() < self.fetch_size {
            if let Some(max) = self.max_rows {
                if self.fetched >= max {
                    exhausted = true;
                    break;
                }
            }
            match self.stream.as_mut().next().await {
                Some(Ok(values)) => {
                    out.push(values);
                    self.fetched += 1;
                }
                Some(Err(error)) => return Err(error),
                None => {
                    exhausted = true;
                    break;
                }
            }
        }
        Ok((out, exhausted))
    }
}

impl RowStream for PostgresRowStream {
    fn next_batch(&mut self) -> Result<Option<RowBatch>, DbError> {
        if self.done {
            return Ok(None);
        }
        let (rows, exhausted) = crate::runtime().block_on(self.pull_batch())?;
        if exhausted {
            self.done = true;
        }
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(RowBatch {
            columns: self.columns.clone(),
            rows,
        }))
    }
}

impl Connection for PostgresConnection {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn server_info(&self) -> String {
        "PostgreSQL".to_string()
    }

    fn introspect(
        &mut self,
        scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        // `NAMES_QUERY` runs unconditionally at every level (F2.1): it is
        // the cheap, near-instant listing the 5 000-table NFR needs, and
        // `Columns`/`Full` only pay for a further per-table round trip
        // (`table_node`) for the objects the scope actually narrows to.
        let rows = crate::runtime()
            .block_on(self.client.query(NAMES_QUERY, &[]))
            .map_err(io_err)?;

        let mut by_schema: std::collections::BTreeMap<String, Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        for row in &rows {
            let schema: String = row.get(0);
            let name: String = row.get(1);
            let kind: i8 = row.get::<_, i8>(2);
            by_schema
                .entry(schema)
                .or_default()
                .push((name, (kind as u8 as char).to_string()));
        }

        let mut roots = Vec::with_capacity(by_schema.len());
        for (schema, objects) in by_schema {
            if let Some(filter) = &scope.schema {
                if filter != &schema {
                    continue;
                }
            }
            let mut children = Vec::with_capacity(objects.len());
            for (name, kind) in objects {
                if let Some(object_filter) = &scope.object {
                    if object_filter != &name {
                        continue;
                    }
                }
                let object_kind = match kind.as_str() {
                    "v" => ObjectKind::View,
                    "m" => ObjectKind::MaterializedView,
                    _ => ObjectKind::Table,
                };
                let node = if level == IntrospectLevel::Names {
                    Node::leaf(name, object_kind)
                } else {
                    self.table_node(&schema, &name, object_kind, level)?
                };
                children.push(node);
            }
            roots.push(Node::with_children(schema, ObjectKind::Schema, children));
        }
        Ok(SchemaSnapshot::new(level, roots))
    }

    fn execute(
        &mut self,
        statement: &Statement,
        options: &ExecOptions,
    ) -> Result<Execution, DbError> {
        let params: Vec<PgParam> = statement.params.iter().map(PgParam).collect();

        let prepared = crate::runtime()
            .block_on(self.client.prepare(&statement.text))
            .map_err(io_err)?;

        if prepared.columns().is_empty() {
            let refs: Vec<&(dyn ToSql + Sync)> =
                params.iter().map(|p| p as &(dyn ToSql + Sync)).collect();
            let affected = crate::runtime()
                .block_on(self.client.execute(&prepared, &refs))
                .map_err(io_err)?;
            return Ok(Execution::Affected(affected));
        }

        let columns: Vec<ColumnMeta> = prepared
            .columns()
            .iter()
            .map(|c| ColumnMeta {
                name: c.name().to_string(),
                type_name: c.type_().name().to_string(),
                nullable: true,
                origin: None,
            })
            .collect();

        let raw_stream = crate::runtime()
            .block_on(self.client.query_raw(&prepared, params))
            .map_err(io_err)?;

        let row_columns = columns.clone();
        let mapped = raw_stream.map(move |row_result| {
            row_result.map_err(io_err).map(|row| {
                let mut values = Vec::with_capacity(row_columns.len());
                for i in 0..row_columns.len() {
                    let PgValue(value) = row.get(i);
                    values.push(value);
                }
                values
            })
        });

        Ok(Execution::Rows(Box::new(PostgresRowStream::new(
            columns,
            options,
            Box::pin(mapped),
        ))))
    }

    fn begin(&mut self) -> Result<(), DbError> {
        crate::runtime()
            .block_on(self.client.batch_execute("BEGIN"))
            .map_err(io_err)
    }

    fn commit(&mut self) -> Result<(), DbError> {
        crate::runtime()
            .block_on(self.client.batch_execute("COMMIT"))
            .map_err(io_err)
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        crate::runtime()
            .block_on(self.client.batch_execute("ROLLBACK"))
            .map_err(io_err)
    }

    fn set_read_only(&mut self, read_only: bool) -> Result<(), DbError> {
        let stmt = if read_only {
            "SET default_transaction_read_only = on"
        } else {
            "SET default_transaction_read_only = off"
        };
        crate::runtime()
            .block_on(self.client.batch_execute(stmt))
            .map_err(io_err)
    }

    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        Some(Box::new(PgServerCancel(self.client.cancel_token())))
    }

    fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
        // DDL reconstruction from the catalog is `db-sql`'s job (a later
        // phase); F1's foundation does not yet carry that generator.
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "Go to DDL for PostgreSQL is not yet implemented",
        ))
    }

    fn apply(&mut self, statements: &[Statement]) -> Result<u64, DbError> {
        let mut total = 0u64;
        for statement in statements {
            if let Execution::Affected(count) = self.execute(statement, &ExecOptions::default())? {
                total += count;
            }
        }
        Ok(total)
    }

    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

impl PostgresConnection {
    /// One table/view's columns (F2.1's `Columns`/`Full` levels), fetched
    /// through `COLUMNS_QUERY` — the per-object round trip `introspect`
    /// only pays for the objects a scope actually narrows to, never for
    /// every table in a 5 000-table catalog at once.
    fn table_node(
        &self,
        schema: &str,
        name: &str,
        kind: ObjectKind,
        level: IntrospectLevel,
    ) -> Result<Node, DbError> {
        let rows = crate::runtime()
            .block_on(self.client.query(COLUMNS_QUERY, &[&schema, &name]))
            .map_err(io_err)?;
        let mut children: Vec<Node> = rows
            .iter()
            .map(|row| {
                let column_name: String = row.get(0);
                let type_name: String = row.get(1);
                let nullable: bool = row.get(2);
                let primary_key: bool = row.get(3);
                Node::leaf(column_name, ObjectKind::Column).with_detail(NodeDetail {
                    type_name: Some(type_name),
                    nullable: Some(nullable),
                    default: None,
                    primary_key,
                    ..NodeDetail::default()
                })
            })
            .collect();
        if level == IntrospectLevel::Full {
            children.extend(self.index_nodes(schema, name)?);
            children.extend(self.constraint_nodes(schema, name)?);
        }
        Ok(Node::with_children(name, kind, children))
    }

    /// `IntrospectLevel::Full`'s own indexes, via [`INDEXES_QUERY`].
    fn index_nodes(&self, schema: &str, name: &str) -> Result<Vec<Node>, DbError> {
        let rows = crate::runtime()
            .block_on(self.client.query(INDEXES_QUERY, &[&schema, &name]))
            .map_err(io_err)?;
        Ok(rows
            .iter()
            .map(|row| {
                let index_name: String = row.get(0);
                let unique: bool = row.get(1);
                let method: String = row.get(2);
                let columns: Vec<String> = row.get(3);
                Node::leaf(index_name, ObjectKind::Index).with_detail(NodeDetail {
                    index: Some(IndexDetail {
                        columns,
                        unique,
                        method: Some(method),
                    }),
                    ..NodeDetail::default()
                })
            })
            .collect())
    }

    /// `IntrospectLevel::Full`'s own constraints, via [`CONSTRAINTS_QUERY`]
    /// — `contype`'s one-character code selects which [`ConstraintKind`]
    /// variant a row becomes (`p` primary key, `f` foreign key, `u`
    /// unique, `c` check; every other code — `t` constraint trigger, `x`
    /// exclusion — is skipped, `database-tools.md` §11's recorded gap).
    fn constraint_nodes(&self, schema: &str, name: &str) -> Result<Vec<Node>, DbError> {
        let rows = crate::runtime()
            .block_on(self.client.query(CONSTRAINTS_QUERY, &[&schema, &name]))
            .map_err(io_err)?;
        let mut nodes = Vec::new();
        for row in &rows {
            let constraint_name: String = row.get(0);
            let contype: i8 = row.get(1);
            let columns: Vec<String> = row.get(2);
            let ref_schema: Option<String> = row.get(3);
            let ref_table: Option<String> = row.get(4);
            let ref_columns: Vec<String> = row.get(5);
            let on_update: i8 = row.get(6);
            let on_delete: i8 = row.get(7);
            let definition: String = row.get(8);
            let kind = match contype as u8 as char {
                'p' => ConstraintKind::PrimaryKey { columns },
                'u' => ConstraintKind::Unique { columns },
                'f' => {
                    let mut reference =
                        ObjectRef::new(ref_table.unwrap_or_default()).with_kind(ObjectKind::Table);
                    if let Some(ref_schema) = ref_schema {
                        reference = reference.with_schema(ref_schema);
                    }
                    ConstraintKind::ForeignKey {
                        columns,
                        ref_table: reference,
                        ref_columns,
                        on_delete: referential_action(&(on_delete as u8 as char).to_string()),
                        on_update: referential_action(&(on_update as u8 as char).to_string()),
                    }
                }
                'c' => ConstraintKind::Check { expr: definition },
                _ => continue,
            };
            nodes.push(
                Node::leaf(constraint_name, ObjectKind::Constraint).with_detail(NodeDetail {
                    constraint: Some(kind),
                    ..NodeDetail::default()
                }),
            );
        }
        Ok(nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `PostgresRowStream` sees only `Vec<Value>`, never a
    /// `tokio_postgres::Row` (which has no public constructor) — so its
    /// batching/`max_rows`/error-propagation logic is exercised here
    /// against a fake `futures_util::stream::iter`, with no real server
    /// needed; `db-integration`'s tests below cover the real wire mapping.
    fn fake_stream(
        values: Vec<Result<Vec<Value>, DbError>>,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<Value>, DbError>> + Send>> {
        Box::pin(futures_util::stream::iter(values))
    }

    fn one_int_column() -> Vec<ColumnMeta> {
        vec![ColumnMeta {
            name: "n".to_string(),
            type_name: "int4".to_string(),
            nullable: true,
            origin: None,
        }]
    }

    #[test]
    fn next_batch_pages_a_live_stream_by_fetch_size() {
        let values: Vec<Result<Vec<Value>, DbError>> =
            (0..5).map(|n| Ok(vec![Value::Int(n)])).collect();
        let options = ExecOptions {
            fetch_size: 2,
            ..Default::default()
        };
        let mut stream = PostgresRowStream::new(one_int_column(), &options, fake_stream(values));

        let first = stream.next_batch().unwrap().unwrap();
        assert_eq!(first.rows, vec![vec![Value::Int(0)], vec![Value::Int(1)]]);
        let second = stream.next_batch().unwrap().unwrap();
        assert_eq!(second.rows, vec![vec![Value::Int(2)], vec![Value::Int(3)]]);
        let third = stream.next_batch().unwrap().unwrap();
        assert_eq!(third.rows, vec![vec![Value::Int(4)]]);
        // Exhausted: further calls stay `None` without re-polling the stream.
        assert!(stream.next_batch().unwrap().is_none());
        assert!(stream.next_batch().unwrap().is_none());
    }

    #[test]
    fn max_rows_caps_the_total_even_with_more_available_in_the_stream() {
        let values: Vec<Result<Vec<Value>, DbError>> =
            (0..10).map(|n| Ok(vec![Value::Int(n)])).collect();
        let options = ExecOptions {
            fetch_size: 3,
            max_rows: Some(4),
            ..Default::default()
        };
        let mut stream = PostgresRowStream::new(one_int_column(), &options, fake_stream(values));

        let mut seen = 0usize;
        while let Some(batch) = stream.next_batch().unwrap() {
            seen += batch.rows.len();
        }
        assert_eq!(seen, 4);
    }

    #[test]
    fn an_error_mid_stream_surfaces_as_a_db_error() {
        let values = vec![
            Ok(vec![Value::Int(1)]),
            Err(DbError::new(DbErrorCode::Unknown, "boom")),
        ];
        let options = ExecOptions {
            fetch_size: 1,
            ..Default::default()
        };
        let mut stream = PostgresRowStream::new(one_int_column(), &options, fake_stream(values));

        let first = stream.next_batch().unwrap().unwrap();
        assert_eq!(first.rows, vec![vec![Value::Int(1)]]);
        let error = stream.next_batch().unwrap_err();
        assert_eq!(error.code, DbErrorCode::Unknown);
    }

    #[test]
    fn an_empty_stream_yields_no_batches() {
        let mut stream = PostgresRowStream::new(
            one_int_column(),
            &ExecOptions::default(),
            fake_stream(vec![]),
        );
        assert!(stream.next_batch().unwrap().is_none());
    }

    #[test]
    fn decode_numeric_zero() {
        // ndigits=0, weight=0, sign=0, dscale=0
        let raw = [0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(decode_numeric(&raw).unwrap(), "0");
    }

    #[test]
    fn decode_numeric_positive_with_fraction() {
        // 123.45: digits [123, 4500], weight=0, sign=0x0000, dscale=2
        let mut raw = vec![];
        raw.extend_from_slice(&2i16.to_be_bytes()); // ndigits
        raw.extend_from_slice(&0i16.to_be_bytes()); // weight
        raw.extend_from_slice(&0u16.to_be_bytes()); // sign (positive)
        raw.extend_from_slice(&2u16.to_be_bytes()); // dscale
        raw.extend_from_slice(&123i16.to_be_bytes());
        raw.extend_from_slice(&4500i16.to_be_bytes());
        assert_eq!(decode_numeric(&raw).unwrap(), "123.45");
    }

    #[test]
    fn decode_numeric_negative() {
        let mut raw = vec![];
        raw.extend_from_slice(&2i16.to_be_bytes());
        raw.extend_from_slice(&0i16.to_be_bytes());
        raw.extend_from_slice(&0x4000u16.to_be_bytes()); // negative
        raw.extend_from_slice(&2u16.to_be_bytes());
        raw.extend_from_slice(&123i16.to_be_bytes());
        raw.extend_from_slice(&4500i16.to_be_bytes());
        assert_eq!(decode_numeric(&raw).unwrap(), "-123.45");
    }

    #[test]
    fn decode_numeric_nan() {
        let mut raw = vec![];
        raw.extend_from_slice(&0i16.to_be_bytes());
        raw.extend_from_slice(&0i16.to_be_bytes());
        raw.extend_from_slice(&0xC000u16.to_be_bytes());
        raw.extend_from_slice(&0u16.to_be_bytes());
        assert_eq!(decode_numeric(&raw).unwrap(), "NaN");
    }

    #[test]
    fn decode_numeric_integer_no_scale() {
        // 42: digits [42], weight=0, dscale=0
        let mut raw = vec![];
        raw.extend_from_slice(&1i16.to_be_bytes());
        raw.extend_from_slice(&0i16.to_be_bytes());
        raw.extend_from_slice(&0u16.to_be_bytes());
        raw.extend_from_slice(&0u16.to_be_bytes());
        raw.extend_from_slice(&42i16.to_be_bytes());
        assert_eq!(decode_numeric(&raw).unwrap(), "42");
    }

    #[test]
    fn decode_numeric_small_fraction_below_one() {
        // 0.001: digits [10], weight=-1, dscale=3
        // group value 10 at weight -1 means 10 * 10000^-1 = 0.001
        let mut raw = vec![];
        raw.extend_from_slice(&1i16.to_be_bytes());
        raw.extend_from_slice(&(-1i16).to_be_bytes());
        raw.extend_from_slice(&0u16.to_be_bytes());
        raw.extend_from_slice(&3u16.to_be_bytes());
        raw.extend_from_slice(&10i16.to_be_bytes());
        assert_eq!(decode_numeric(&raw).unwrap(), "0.001");
    }

    #[test]
    fn introspection_fixture_text_shapes_the_expected_columns() {
        assert!(NAMES_QUERY.contains("pg_catalog.pg_class"));
        assert!(NAMES_QUERY.contains("relkind"));
        assert!(COLUMNS_QUERY.contains("pg_attribute"));
        assert!(COLUMNS_QUERY.contains("$1"));
        assert!(COLUMNS_QUERY.contains("$2"));
        assert!(COLUMNS_QUERY.contains("is_primary_key"));
    }

    #[test]
    fn constraints_fixture_text_names_pg_constraint_and_the_fk_reference() {
        assert!(CONSTRAINTS_QUERY.contains("pg_catalog.pg_constraint"));
        assert!(CONSTRAINTS_QUERY.contains("confrelid"));
        assert!(CONSTRAINTS_QUERY.contains("confupdtype"));
        assert!(CONSTRAINTS_QUERY.contains("confdeltype"));
        assert!(CONSTRAINTS_QUERY.contains("pg_get_constraintdef"));
        assert!(CONSTRAINTS_QUERY.contains("$1"));
        assert!(CONSTRAINTS_QUERY.contains("$2"));
    }

    #[test]
    fn indexes_fixture_text_names_pg_index_and_its_access_method() {
        assert!(INDEXES_QUERY.contains("pg_catalog.pg_index"));
        assert!(INDEXES_QUERY.contains("indisunique"));
        assert!(INDEXES_QUERY.contains("pg_am"));
        assert!(INDEXES_QUERY.contains("$1"));
        assert!(INDEXES_QUERY.contains("$2"));
    }

    #[test]
    fn referential_action_maps_the_known_codes_and_omits_no_action() {
        assert_eq!(referential_action("a"), None);
        assert_eq!(referential_action(""), None);
        assert_eq!(referential_action("c"), Some("CASCADE".to_string()));
        assert_eq!(referential_action("n"), Some("SET NULL".to_string()));
        assert_eq!(referential_action("d"), Some("SET DEFAULT".to_string()));
        assert_eq!(referential_action("r"), Some("RESTRICT".to_string()));
    }

    #[test]
    fn driver_id_and_capabilities() {
        let driver = PostgresDriver;
        assert_eq!(driver.id(), "postgresql");
        assert!(driver.capabilities().contains(Capabilities::TRANSACTIONS));
        assert!(driver.capabilities().contains(Capabilities::RETURNING));
    }

    #[test]
    fn connecting_with_tls_requested_is_not_yet_supported() {
        let spec = ConnectSpec {
            driver: "postgresql".to_string(),
            host: "localhost".to_string(),
            port: Some(5432),
            database: "postgres".to_string(),
            user: "postgres".to_string(),
            url: String::new(),
            password: None,
            ssl: db_core::datasource::SslConfig {
                mode: SslMode::Require,
                ca_file: None,
            },
        };
        match PostgresDriver.connect(&spec) {
            Err(error) => assert_eq!(error.code, DbErrorCode::NotSupported),
            Ok(_) => panic!("expected NotSupported"),
        }
    }

    /// Only runs with `--features db-integration` against a real server
    /// named by `IDE_DB_POSTGRES_URL` (`docs/architecture/db-integration.md`)
    /// — skips itself (does not fail) when that env var is unset, so
    /// `cargo test --features db-integration` still passes on a machine
    /// with no Postgres container up.
    #[cfg(feature = "db-integration")]
    #[test]
    fn connect_execute_and_introspect_against_a_real_server() {
        let Some(spec) = crate::testsupport::postgres_test_spec() else {
            eprintln!("IDE_DB_POSTGRES_URL not set — skipping");
            return;
        };
        let mut conn = PostgresDriver.connect(&spec).expect("connect");
        let Execution::Rows(mut stream) = conn
            .execute(&Statement::sql("SELECT 1"), &ExecOptions::default())
            .expect("execute")
        else {
            panic!("expected rows");
        };
        let batch = stream.next_batch().expect("batch").expect("some rows");
        assert_eq!(batch.rows, vec![vec![Value::Int(1)]]);

        let snapshot = conn
            .introspect(&IntrospectScope::default(), IntrospectLevel::Names)
            .expect("introspect");
        assert_eq!(snapshot.level, IntrospectLevel::Names);

        let with_columns = conn
            .introspect(&IntrospectScope::default(), IntrospectLevel::Columns)
            .expect("introspect at Columns level");
        assert_eq!(with_columns.level, IntrospectLevel::Columns);
    }

    /// `statm`'s resident-set field, in bytes — the same "read the kernel's
    /// own accounting rather than estimate" approach a heap-growth NFR
    /// check needs, gated behind `db-integration` since it needs a real,
    /// million-row result to be meaningful.
    #[cfg(feature = "db-integration")]
    fn resident_bytes() -> u64 {
        let statm = std::fs::read_to_string("/proc/self/statm").expect("read /proc/self/statm");
        let pages: u64 = statm
            .split_whitespace()
            .nth(1)
            .expect("resident field")
            .parse()
            .expect("resident field is a number");
        pages * 4096
    }

    /// F3d's own NFR: a 1 000 000-row result's first page must be fast
    /// (proves `execute` no longer blocks on draining the whole result
    /// before returning) and later pages must not accumulate memory
    /// (proves `next_batch` is paging a live stream, not re-chunking an
    /// already-fully-materialised buffer — the defect `database-tools.md`
    /// §4/§11 tracked against this driver).
    #[cfg(feature = "db-integration")]
    #[test]
    fn a_million_row_generate_series_streams_a_fast_first_batch_with_bounded_memory() {
        let Some(spec) = crate::testsupport::postgres_test_spec() else {
            eprintln!("IDE_DB_POSTGRES_URL not set — skipping");
            return;
        };
        let mut conn = PostgresDriver.connect(&spec).expect("connect");
        let options = ExecOptions {
            fetch_size: 200,
            ..ExecOptions::default()
        };
        let Execution::Rows(mut stream) = conn
            .execute(
                &Statement::sql("SELECT * FROM generate_series(1, 1000000)"),
                &options,
            )
            .expect("execute")
        else {
            panic!("expected rows");
        };

        let started = std::time::Instant::now();
        let first = stream
            .next_batch()
            .expect("first batch")
            .expect("some rows");
        let elapsed = started.elapsed();
        assert_eq!(first.rows.len(), 200);
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "first batch took {elapsed:?}, expected < 100ms"
        );

        let baseline = resident_bytes();
        for _ in 0..3 {
            stream.next_batch().expect("batch").expect("some rows");
        }
        let growth = resident_bytes().saturating_sub(baseline);
        assert!(
            growth < 30 * 1024 * 1024,
            "RSS grew by {growth} bytes after three more pages, expected < 30MB"
        );
    }

    /// The parked-cursor rule `database-tools.md` §4 documents: a live
    /// stream from one `execute` must not block or be disturbed by a
    /// second `execute` on the same `PostgresConnection` — unlike SQLite,
    /// Postgres needs no second connection for this, since tokio-postgres
    /// pipelines concurrent commands on one `Client` transparently.
    #[cfg(feature = "db-integration")]
    #[test]
    fn a_second_execute_does_not_disturb_an_open_stream() {
        let Some(spec) = crate::testsupport::postgres_test_spec() else {
            eprintln!("IDE_DB_POSTGRES_URL not set — skipping");
            return;
        };
        let mut conn = PostgresDriver.connect(&spec).expect("connect");
        let options = ExecOptions {
            fetch_size: 2,
            ..ExecOptions::default()
        };
        let Execution::Rows(mut stream) = conn
            .execute(
                &Statement::sql("SELECT * FROM generate_series(1, 5)"),
                &options,
            )
            .expect("execute")
        else {
            panic!("expected rows");
        };
        let first = stream.next_batch().expect("batch").expect("some rows");
        assert_eq!(first.rows, vec![vec![Value::Int(1)], vec![Value::Int(2)]]);

        conn.execute(&Statement::sql("SELECT 1"), &ExecOptions::default())
            .expect("second execute");

        let rest = stream.next_batch().expect("batch").expect("some rows");
        assert_eq!(rest.rows, vec![vec![Value::Int(3)], vec![Value::Int(4)]]);
        let last = stream.next_batch().expect("batch").expect("some rows");
        assert_eq!(last.rows, vec![vec![Value::Int(5)]]);
        assert!(stream.next_batch().expect("batch").is_none());
    }
}
