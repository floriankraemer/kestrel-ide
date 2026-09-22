//! The PostgreSQL backend (ADR-0058): async, `block_on`'d against the
//! private `"db-io"` runtime (`crate::runtime`) from whichever `std::thread`
//! calls it (`ui-shell`'s `SessionWorker`, in F1.6) — never the ambient
//! runtime, and this connection's own methods stay ordinary blocking calls
//! from every other crate's point of view (ADR-0058 §1).
//!
//! ## TLS (F2 follow-up)
//!
//! `SslMode::Disable` connects with `NoTls`, unchanged from F1. Every
//! other mode goes through `tokio_postgres_rustls::MakeRustlsConnect`
//! over a `rustls::ClientConfig` [`tls_config_for`] builds, `ring` the
//! only crypto provider in the tree (ADR-0061 §2/R2 — `rustls`'s own
//! Cargo feature list here never enables `aws_lc_rs`):
//! - `VerifyFull` verifies the full chain *and* the hostname, against
//!   `ca_file`'s PEM roots if one is set, else the OS trust store
//!   (`rustls-native-certs`).
//! - `VerifyCa` is handled identically to `VerifyFull` in this pass: a
//!   verifier that checks the chain but skips the hostname/SAN match
//!   needs a deeper corner of `rustls::client::danger` than this pass
//!   budgeted for, and being *stricter* than asked (still checking the
//!   hostname) is the safe direction to round a gap in, not a silent
//!   downgrade — documented here rather than left to be discovered.
//! - `Prefer`/`Require` both use [`DangerousNoVerify`], an explicit,
//!   named "encrypted, not verified" `ServerCertVerifier` — ADR-0061 §2's
//!   own "no skip-verify flag" rule is about the *user-facing* option
//!   the Data Source dialog offers, not about whether `Require`'s own
//!   documented meaning ("must be encrypted, verification is `VerifyCa`/
//!   `VerifyFull`'s job") gets a working implementation.

use std::error::Error as StdError;
use std::pin::Pin;
use std::sync::Arc;

use bytes::BytesMut;
use futures_util::{Stream, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio_postgres::types::{FromSql, IsNull, ToSql, Type as PgType};
use tokio_postgres::{Client, NoTls};

use db_core::datasource::{ConnectSpec, SslConfig, SslMode};
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

/// [`PostgresConnection::table_name_for_oid`]'s own query (`ColumnMeta::
/// origin`, F4b) — short enough to keep inline rather than its own
/// `.sql` file, unlike the multi-join introspection queries above.
const TABLE_NAME_BY_OID_QUERY: &str = "SELECT relname FROM pg_catalog.pg_class WHERE oid = $1";
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

/// `Prefer`/`Require`: encrypted, deliberately unverified — see this
/// module's own doc comment for why this is a named, explicit type
/// rather than a config flag.
#[derive(Debug)]
struct DangerousNoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for DangerousNoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// `ca_file`'s PEM roots, or (no `ca_file` set) the OS trust store —
/// shared by `VerifyCa`/`VerifyFull` (see this module's own doc comment
/// for why the two are not distinguished further in this pass).
fn root_store_for(ca_file: Option<&str>) -> Result<RootCertStore, DbError> {
    let mut store = RootCertStore::empty();
    if let Some(path) = ca_file {
        let bytes = std::fs::read(path)
            .map_err(|e| DbError::new(DbErrorCode::Io, format!("reading CA file {path}: {e}")))?;
        for cert in rustls_pemfile::certs(&mut bytes.as_slice()) {
            let cert = cert.map_err(|e| {
                DbError::new(DbErrorCode::Io, format!("parsing CA file {path}: {e}"))
            })?;
            store.add(cert).map_err(|e| {
                DbError::new(
                    DbErrorCode::ConnectionFailed,
                    format!("invalid CA certificate in {path}: {e}"),
                )
            })?;
        }
    } else {
        let native = rustls_native_certs::load_native_certs().map_err(|e| {
            DbError::new(
                DbErrorCode::Io,
                format!("reading the OS certificate trust store: {e}"),
            )
        })?;
        for cert in native {
            // A handful of unparsable/duplicate system entries is normal
            // (rustls-native-certs' own documented behaviour); this only
            // fails if the store ends up with nothing at all, below.
            let _ = store.add(cert);
        }
    }
    if store.is_empty() {
        return Err(DbError::new(
            DbErrorCode::ConnectionFailed,
            "no CA certificates available to verify the server against \
             (set a ca_file, or check the OS trust store)",
        ));
    }
    Ok(store)
}

/// See this module's own doc comment for the per-mode reasoning.
/// `Disable` never reaches this function — `PostgresDriver::connect`'s
/// own branch on `spec.ssl.mode` is what decides that.
fn tls_config_for(ssl: &SslConfig) -> Result<ClientConfig, DbError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| DbError::new(DbErrorCode::ConnectionFailed, e.to_string()))?;
    match ssl.mode {
        SslMode::Disable => unreachable!("connect() only calls this for a non-Disable mode"),
        SslMode::Prefer | SslMode::Require => Ok(builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(DangerousNoVerify(provider)))
            .with_no_client_auth()),
        SslMode::VerifyCa | SslMode::VerifyFull => {
            let roots = Arc::new(root_store_for(ssl.ca_file.as_deref())?);
            let verifier = WebPkiServerVerifier::builder(roots)
                .build()
                .map_err(|e| DbError::new(DbErrorCode::ConnectionFailed, e.to_string()))?;
            Ok(builder
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth())
        }
    }
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

        // The connection object drives the actual socket I/O; it must keep
        // running for as long as `client` is used, the same background
        // task every tokio-postgres consumer spawns — one arm per
        // transport, since `NoTls`'s and `MakeRustlsConnect`'s own
        // `Connection<S, T>` are different concrete types with no common
        // trait this driver needs beyond "poll it to completion".
        let client = if spec.ssl.mode == SslMode::Disable {
            let (client, connection) = crate::runtime()
                .block_on(config.connect(NoTls))
                .map_err(io_err)?;
            crate::runtime().spawn(async move {
                let _ = connection.await;
            });
            client
        } else {
            let tls_config = tls_config_for(&spec.ssl)?;
            let connector = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
            let (client, connection) = crate::runtime()
                .block_on(config.connect(connector))
                .map_err(io_err)?;
            crate::runtime().spawn(async move {
                let _ = connection.await;
            });
            client
        };
        Ok(Box::new(PostgresConnection {
            client,
            table_names: std::collections::HashMap::new(),
        }))
    }
}

pub struct PostgresConnection {
    client: Client,
    /// `ColumnMeta::origin`'s own cache (F4b, `database-tools.md` §11):
    /// `Column::table_oid()` names a row description column's source
    /// table by oid, never by name, so `execute` resolves it against
    /// `pg_class` once per oid and keeps the answer here for the rest of
    /// this connection's life — a table's oid never changes without a
    /// `DROP`/`CREATE` this connection would have to reconnect to see
    /// anyway.
    table_names: std::collections::HashMap<u32, String>,
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
                origin: c.table_oid().and_then(|oid| self.table_name_for_oid(oid)),
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
    /// [`ColumnMeta::origin`]'s own lookup — see `table_names`'s own doc
    /// comment on why this caches per connection rather than per call.
    fn table_name_for_oid(&mut self, oid: u32) -> Option<String> {
        if let Some(name) = self.table_names.get(&oid) {
            return Some(name.clone());
        }
        let rows = crate::runtime()
            .block_on(self.client.query(TABLE_NAME_BY_OID_QUERY, &[&oid]))
            .ok()?;
        let name: String = rows.first()?.get(0);
        self.table_names.insert(oid, name.clone());
        Some(name)
    }

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
            let Some(kind) = constraint_kind_for(
                contype as u8 as char,
                columns,
                ref_schema,
                ref_table,
                ref_columns,
                on_update as u8 as char,
                on_delete as u8 as char,
                definition,
            ) else {
                continue;
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

/// [`PostgresConnection::constraint_nodes`]'s own `pg_constraint.contype`
/// classification — pulled out of the `Row`-iterating loop (a
/// `tokio_postgres::Row` has no public constructor, so this takes the
/// already-`row.get()`-extracted scalars instead) so a unit test can drive
/// every branch directly. `None` for a `contype` this driver does not
/// model (`t` constraint trigger, `x` exclusion — `database-tools.md`
/// §11's recorded gap).
#[allow(clippy::too_many_arguments)]
fn constraint_kind_for(
    contype: char,
    columns: Vec<String>,
    ref_schema: Option<String>,
    ref_table: Option<String>,
    ref_columns: Vec<String>,
    on_update: char,
    on_delete: char,
    definition: String,
) -> Option<ConstraintKind> {
    match contype {
        'p' => Some(ConstraintKind::PrimaryKey { columns }),
        'u' => Some(ConstraintKind::Unique { columns }),
        'f' => {
            let mut reference =
                ObjectRef::new(ref_table.unwrap_or_default()).with_kind(ObjectKind::Table);
            if let Some(ref_schema) = ref_schema {
                reference = reference.with_schema(ref_schema);
            }
            Some(ConstraintKind::ForeignKey {
                columns,
                ref_table: reference,
                ref_columns,
                on_delete: referential_action(&on_delete.to_string()),
                on_update: referential_action(&on_update.to_string()),
            })
        }
        'c' => Some(ConstraintKind::Check { expr: definition }),
        _ => None,
    }
}

#[cfg(test)]
#[path = "postgres_tests.rs"]
mod tests;
