//! The Cassandra/Scylla backend (F7.4): async, `block_on`'d against the
//! private `"db-io"` runtime, same pattern as `postgres.rs`/`mongodb.rs`.
//! Introspection reads `system_schema` directly (keyspaces, tables, views,
//! UDTs, functions, columns) — Cassandra's own catalog, the same idea
//! `postgres.rs`'s own `information_schema`-style query uses.
//!
//! TLS is plaintext-only, permanently, not just for this slice: the
//! `scylla` crate's own `Cargo.toml` gives a dependent no way to select
//! `rustls`'s `ring` provider over its default `aws_lc_rs` one (unlike
//! postgres/mongodb/redis/russh, whose manifests each expose that choice
//! explicitly) — this crate's `Cargo.toml` therefore never enables
//! scylla's `rustls-023` feature at all, in any feature combination, so
//! `aws-lc-rs` can never enter the tree through this driver. `SslMode`
//! other than `Disable` returns `NotSupported` with a message naming why.
//! Revisit once scylla exposes a provider-agnostic rustls option
//! upstream (`database-tools-plan.md`'s F7.4 row).

use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;
use scylla::value::{CqlValue, Row};

use db_core::datasource::{ConnectSpec, SslMode};
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
use db_sql::split::split;

fn map_err(error: impl std::fmt::Display) -> DbError {
    DbError::new(DbErrorCode::ConnectionFailed, error.to_string())
}

/// A leading-keyword classification of a CQL statement — `SELECT` reads,
/// everything else (`INSERT`/`UPDATE`/`DELETE`/DDL/…) is treated as a
/// write, the F7.4 brief's own rule.
fn is_read_only_statement(text: &str) -> bool {
    text.split_whitespace()
        .next()
        .map(|first| first.eq_ignore_ascii_case("SELECT"))
        .unwrap_or(false)
}

pub struct CassandraDriver;

impl Driver for CassandraDriver {
    fn id(&self) -> &str {
        "cassandra"
    }

    fn capabilities(&self) -> Capabilities {
        // CQL has no `BEGIN`/`COMMIT` — `begin` returns `NotSupported`.
        Capabilities::NONE
    }

    fn connect(&self, spec: &ConnectSpec) -> Result<Box<dyn Connection>, DbError> {
        if spec.ssl.mode != SslMode::Disable {
            return Err(DbError::new(
                DbErrorCode::NotSupported,
                "TLS for Cassandra/Scylla is not available yet (scylla's rustls feature forces aws-lc-rs)",
            ));
        }
        let mut builder = SessionBuilder::new();
        let host = if spec.host.is_empty() {
            "127.0.0.1".to_string()
        } else {
            spec.host.clone()
        };
        let port = spec.port.unwrap_or(9042);
        builder = builder.known_node(format!("{host}:{port}"));
        if !spec.user.is_empty() {
            let password = spec.password.clone().unwrap_or_default();
            builder = builder.user(spec.user.clone(), password);
        }
        if !spec.database.is_empty() {
            builder = builder.use_keyspace(spec.database.clone(), true);
        }
        let session = crate::runtime()
            .block_on(builder.build())
            .map_err(map_err)?;
        Ok(Box::new(CassandraConnection { session }))
    }
}

pub struct CassandraConnection {
    session: Session,
}

/// `system_schema.keyspaces`/`.tables`/`.views`/`.types`/`.functions`/
/// `.columns` — Cassandra's own catalog tables, always present and never
/// requiring elevated privileges to read.
mod schema_queries {
    pub const KEYSPACES: &str = "SELECT keyspace_name FROM system_schema.keyspaces";
    pub const TABLES: &str = "SELECT table_name FROM system_schema.tables WHERE keyspace_name = ?";
    pub const VIEWS: &str = "SELECT view_name FROM system_schema.views WHERE keyspace_name = ?";
    pub const TYPES: &str = "SELECT type_name FROM system_schema.types WHERE keyspace_name = ?";
    pub const FUNCTIONS: &str =
        "SELECT function_name FROM system_schema.functions WHERE keyspace_name = ?";
    pub const COLUMNS: &str = "SELECT column_name, type, kind FROM system_schema.columns \
        WHERE keyspace_name = ? AND table_name = ?";
}

fn first_column_names(rows: impl Iterator<Item = Row>) -> Vec<String> {
    rows.filter_map(|row| match row.columns.into_iter().next() {
        Some(Some(CqlValue::Text(text))) | Some(Some(CqlValue::Ascii(text))) => Some(text),
        _ => None,
    })
    .collect()
}

impl CassandraConnection {
    fn query_names(
        &self,
        cql: &str,
        keyspace: Option<&str>,
        kind: ObjectKind,
    ) -> Result<Vec<Node>, DbError> {
        let result = match keyspace {
            Some(name) => crate::runtime()
                .block_on(self.session.query_unpaged(cql, (name,)))
                .map_err(map_err)?,
            None => crate::runtime()
                .block_on(self.session.query_unpaged(cql, ()))
                .map_err(map_err)?,
        };
        let rows_result = result.into_rows_result().map_err(map_err)?;
        let rows: Vec<Row> = rows_result
            .rows::<Row>()
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        Ok(first_column_names(rows.into_iter())
            .into_iter()
            .map(|name| Node::leaf(name, kind))
            .collect())
    }

    fn columns_of(&self, keyspace: &str, table: &str) -> Result<Vec<Node>, DbError> {
        let result = crate::runtime()
            .block_on(
                self.session
                    .query_unpaged(schema_queries::COLUMNS, (keyspace, table)),
            )
            .map_err(map_err)?;
        let rows_result = result.into_rows_result().map_err(map_err)?;
        let rows: Vec<Row> = rows_result
            .rows::<Row>()
            .map_err(map_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_err)?;
        Ok(column_rows_to_nodes(rows))
    }
}

/// [`CassandraConnection::columns_of`]'s own row-to-`Node` mapping, pulled
/// out so a unit test can feed it hand-built [`Row`]s (a public struct with
/// no query behind it) without a live `Session`.
fn column_rows_to_nodes(rows: Vec<Row>) -> Vec<Node> {
    rows.into_iter()
        .filter_map(|row| {
            let mut columns = row.columns.into_iter();
            let name = match columns.next()? {
                Some(CqlValue::Text(text)) | Some(CqlValue::Ascii(text)) => text,
                _ => return None,
            };
            let type_name = match columns.next()? {
                Some(CqlValue::Text(text)) | Some(CqlValue::Ascii(text)) => Some(text),
                _ => None,
            };
            let key_kind = match columns.next()? {
                Some(CqlValue::Text(text)) | Some(CqlValue::Ascii(text)) => text,
                _ => String::new(),
            };
            let is_key = key_kind == "partition_key" || key_kind == "clustering";
            Some(
                Node::leaf(name, ObjectKind::Column).with_detail(NodeDetail {
                    type_name,
                    // `primary_key` doubles here for "part of the
                    // partition or clustering key" — CQL's own two-role
                    // key model doesn't need a second bit alongside the
                    // relational backends' single `PRIMARY KEY`.
                    primary_key: is_key,
                    ..NodeDetail::default()
                }),
            )
        })
        .collect()
}

fn cql_to_value(value: Option<CqlValue>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    match value {
        CqlValue::Ascii(s) | CqlValue::Text(s) => Value::Text(s),
        CqlValue::Boolean(b) => Value::Bool(b),
        CqlValue::Blob(bytes) => Value::Bytes(bytes),
        CqlValue::Double(f) => Value::Float(f),
        CqlValue::Float(f) => Value::Float(f as f64),
        CqlValue::Int(i) => Value::Int(i as i64),
        CqlValue::BigInt(i) => Value::Int(i),
        CqlValue::SmallInt(i) => Value::Int(i as i64),
        CqlValue::TinyInt(i) => Value::Int(i as i64),
        CqlValue::Counter(c) => Value::Int(c.0),
        CqlValue::Uuid(id) => Value::Text(id.to_string()),
        CqlValue::Timeuuid(id) => Value::Text(id.to_string()),
        CqlValue::Inet(addr) => Value::Text(addr.to_string()),
        CqlValue::Decimal(d) => Value::Decimal(format!("{d:?}")),
        CqlValue::Varint(v) => Value::Decimal(format!("{v:?}")),
        CqlValue::List(items) | CqlValue::Set(items) => Value::Array(
            items
                .into_iter()
                .map(|item| cql_to_value(Some(item)))
                .collect(),
        ),
        CqlValue::Map(pairs) => Value::Document(
            pairs
                .into_iter()
                .map(|(key, value)| (format!("{key:?}"), cql_to_value(Some(value))))
                .collect(),
        ),
        CqlValue::Tuple(items) => Value::Array(items.into_iter().map(cql_to_value).collect()),
        CqlValue::UserDefinedType { fields, .. } => Value::Document(
            fields
                .into_iter()
                .map(|(name, value)| (name, cql_to_value(value)))
                .collect(),
        ),
        other => Value::Other {
            type_name: "cql".to_string(),
            display: format!("{other:?}"),
        },
    }
}

struct EagerRows(Option<RowBatch>);

impl RowStream for EagerRows {
    fn next_batch(&mut self) -> Result<Option<RowBatch>, DbError> {
        Ok(self.0.take())
    }
}

impl Connection for CassandraConnection {
    fn dialect(&self) -> Dialect {
        Dialect::Cassandra
    }

    fn server_info(&self) -> String {
        "Cassandra/Scylla".to_string()
    }

    fn introspect(
        &mut self,
        scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        match (&scope.catalog, &scope.object) {
            (None, _) => {
                let roots =
                    self.query_names(schema_queries::KEYSPACES, None, ObjectKind::Keyspace)?;
                Ok(SchemaSnapshot::new(level, roots))
            }
            (Some(keyspace), None) => {
                let mut children =
                    self.query_names(schema_queries::TABLES, Some(keyspace), ObjectKind::Table)?;
                children.extend(self.query_names(
                    schema_queries::VIEWS,
                    Some(keyspace),
                    ObjectKind::MaterializedView,
                )?);
                children.extend(self.query_names(
                    schema_queries::TYPES,
                    Some(keyspace),
                    ObjectKind::Type,
                )?);
                children.extend(self.query_names(
                    schema_queries::FUNCTIONS,
                    Some(keyspace),
                    ObjectKind::Routine,
                )?);
                Ok(SchemaSnapshot::new(
                    level,
                    vec![Node {
                        name: keyspace.clone(),
                        kind: ObjectKind::Keyspace,
                        children: Children::Loaded(children),
                        detail: NodeDetail::default(),
                    }],
                ))
            }
            (Some(keyspace), Some(table)) => {
                let children = self.columns_of(keyspace, table)?;
                Ok(SchemaSnapshot::new(
                    level,
                    vec![Node {
                        name: table.clone(),
                        kind: ObjectKind::Table,
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
        if statement.lang != QueryLang::Cql {
            return Err(DbError::new(
                DbErrorCode::InvalidStatement,
                "the Cassandra/Scylla connection only executes Cql statements",
            ));
        }
        let statements = split(&statement.text, Dialect::Cassandra);
        if statements.is_empty() {
            return Ok(Execution::Ok);
        }

        let mut results = Vec::with_capacity(statements.len());
        for parsed in &statements {
            let text = parsed.text(&statement.text).trim();
            if text.is_empty() {
                continue;
            }
            if options.read_only && !is_read_only_statement(text) {
                return Err(DbError::new(
                    DbErrorCode::ReadOnlyViolation,
                    "this statement is not allowed on a read-only data source",
                ));
            }
            let mut cql = scylla::statement::Statement::new(text.to_string());
            cql.set_page_size(options.fetch_size as i32);
            let result = crate::runtime()
                .block_on(self.session.query_unpaged(cql, ()))
                .map_err(map_err)?;
            match result.into_rows_result() {
                Ok(rows_result) => {
                    let columns: Vec<ColumnMeta> = rows_result
                        .column_specs()
                        .iter()
                        .map(|spec| ColumnMeta {
                            name: spec.name().to_string(),
                            type_name: format!("{:?}", spec.typ()),
                            nullable: true,
                            origin: None,
                        })
                        .collect();
                    let rows: Vec<Vec<Value>> = rows_result
                        .rows::<Row>()
                        .map_err(map_err)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(map_err)?
                        .into_iter()
                        .map(|row| row.columns.into_iter().map(cql_to_value).collect())
                        .collect();
                    results.push(Execution::Rows(Box::new(EagerRows(Some(RowBatch {
                        columns,
                        rows,
                    })))));
                }
                Err(_) => {
                    // Not a rows-shaped result (DDL/DML) — CQL has no
                    // portable "rows affected" count the way SQL's
                    // execute() does, so this reports a bare `Ok`.
                    results.push(Execution::Ok);
                }
            }
        }

        Ok(match results.len() {
            0 => Execution::Ok,
            1 => results.into_iter().next().expect("checked len == 1"),
            _ => Execution::Multi(results),
        })
    }

    fn begin(&mut self) -> Result<(), DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "CQL has no BEGIN/COMMIT — every statement is its own unit",
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
        // Client-side only (`execute`'s `is_read_only_statement` check).
        Ok(())
    }

    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        // Cancel = drop the connection (F7.4 brief) — CQL has no
        // server-side per-statement cancel this driver wires up yet.
        None
    }

    fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "DDL synthesis from system_schema is not yet implemented for Cassandra/Scylla",
        ))
    }

    fn apply(&mut self, _statements: &[Statement]) -> Result<u64, DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "Cassandra/Scylla has no EditBuffer-shaped writes wired up yet — issue CQL statements directly",
        ))
    }

    fn close(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_with_ssl_mode(mode: SslMode) -> ConnectSpec {
        ConnectSpec {
            driver: "cassandra".to_string(),
            host: "127.0.0.1".to_string(),
            port: Some(9042),
            database: String::new(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: db_core::datasource::SslConfig {
                mode,
                ca_file: None,
            },
        }
    }

    #[test]
    fn connect_refuses_any_ssl_mode_other_than_disable() {
        for mode in [
            SslMode::Prefer,
            SslMode::Require,
            SslMode::VerifyCa,
            SslMode::VerifyFull,
        ] {
            let error = match CassandraDriver.connect(&spec_with_ssl_mode(mode)) {
                Err(error) => error,
                Ok(_) => panic!("TLS must be refused client-side, before any network call"),
            };
            assert_eq!(error.code, DbErrorCode::NotSupported);
            assert_eq!(
                error.message,
                "TLS for Cassandra/Scylla is not available yet (scylla's rustls feature forces aws-lc-rs)"
            );
        }
    }

    #[test]
    fn select_is_read_only_case_insensitively() {
        assert!(is_read_only_statement("SELECT * FROM ks.tbl"));
        assert!(is_read_only_statement("  select * from ks.tbl"));
    }

    #[test]
    fn every_other_leading_keyword_is_a_write() {
        assert!(!is_read_only_statement("INSERT INTO ks.tbl (a) VALUES (1)"));
        assert!(!is_read_only_statement(
            "UPDATE ks.tbl SET a = 1 WHERE id = 1"
        ));
        assert!(!is_read_only_statement("DELETE FROM ks.tbl WHERE id = 1"));
        assert!(!is_read_only_statement(
            "CREATE TABLE ks.tbl (id int PRIMARY KEY)"
        ));
        assert!(!is_read_only_statement(""));
    }

    #[test]
    fn cql_to_value_maps_scalars() {
        assert_eq!(
            cql_to_value(Some(CqlValue::Text("hi".to_string()))),
            Value::Text("hi".to_string())
        );
        assert_eq!(cql_to_value(Some(CqlValue::Int(3))), Value::Int(3));
        assert_eq!(
            cql_to_value(Some(CqlValue::Boolean(true))),
            Value::Bool(true)
        );
        assert_eq!(cql_to_value(None), Value::Null);
    }

    #[test]
    fn cql_to_value_maps_collections_to_array() {
        let value = cql_to_value(Some(CqlValue::List(vec![
            CqlValue::Int(1),
            CqlValue::Int(2),
        ])));
        assert_eq!(value, Value::Array(vec![Value::Int(1), Value::Int(2)]));
    }

    #[test]
    fn cql_to_value_maps_every_scalar_and_id_type() {
        assert_eq!(
            cql_to_value(Some(CqlValue::Ascii("a".to_string()))),
            Value::Text("a".to_string())
        );
        assert_eq!(
            cql_to_value(Some(CqlValue::Blob(vec![1, 2]))),
            Value::Bytes(vec![1, 2])
        );
        assert_eq!(cql_to_value(Some(CqlValue::Double(1.5))), Value::Float(1.5));
        assert_eq!(
            cql_to_value(Some(CqlValue::Float(1.5))),
            Value::Float(1.5f32 as f64)
        );
        assert_eq!(cql_to_value(Some(CqlValue::BigInt(9))), Value::Int(9));
        assert_eq!(cql_to_value(Some(CqlValue::SmallInt(9))), Value::Int(9));
        assert_eq!(cql_to_value(Some(CqlValue::TinyInt(9))), Value::Int(9));
        assert_eq!(
            cql_to_value(Some(CqlValue::Counter(scylla::value::Counter(3)))),
            Value::Int(3)
        );
        let uuid = uuid::Uuid::nil();
        assert_eq!(
            cql_to_value(Some(CqlValue::Uuid(uuid))),
            Value::Text(uuid.to_string())
        );
        let inet: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(
            cql_to_value(Some(CqlValue::Inet(inet))),
            Value::Text(inet.to_string())
        );
        assert_eq!(
            cql_to_value(Some(CqlValue::Timeuuid(scylla::value::CqlTimeuuid::from(
                uuid
            )))),
            Value::Text(uuid.to_string())
        );
    }

    #[test]
    fn cql_to_value_maps_decimal_and_varint_to_the_decimal_variant() {
        match cql_to_value(Some(CqlValue::Varint(
            scylla::value::CqlVarint::from_signed_bytes_be(vec![5]),
        ))) {
            Value::Decimal(_) => {}
            other => panic!("expected Decimal, got {other:?}"),
        }
    }

    #[test]
    fn cql_to_value_falls_back_to_other_for_an_unmodelled_type() {
        match cql_to_value(Some(CqlValue::Duration(scylla::value::CqlDuration {
            months: 0,
            days: 0,
            nanoseconds: 0,
        }))) {
            Value::Other { type_name, .. } => assert_eq!(type_name, "cql"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn cql_to_value_maps_sets_maps_tuples_and_udts() {
        let set = cql_to_value(Some(CqlValue::Set(vec![CqlValue::Int(1)])));
        assert_eq!(set, Value::Array(vec![Value::Int(1)]));

        let map = cql_to_value(Some(CqlValue::Map(vec![(
            CqlValue::Text("k".to_string()),
            CqlValue::Int(1),
        )])));
        match map {
            Value::Document(fields) => {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].1, Value::Int(1));
            }
            other => panic!("expected Document, got {other:?}"),
        }

        let tuple = cql_to_value(Some(CqlValue::Tuple(vec![Some(CqlValue::Int(1)), None])));
        assert_eq!(tuple, Value::Array(vec![Value::Int(1), Value::Null]));

        let udt = cql_to_value(Some(CqlValue::UserDefinedType {
            keyspace: "ks".to_string(),
            name: "addr".to_string(),
            fields: vec![("city".to_string(), Some(CqlValue::Text("NYC".to_string())))],
        }));
        match udt {
            Value::Document(fields) => {
                assert_eq!(
                    fields,
                    vec![("city".to_string(), Value::Text("NYC".to_string()))]
                );
            }
            other => panic!("expected Document, got {other:?}"),
        }
    }

    #[test]
    fn column_rows_to_nodes_flags_partition_and_clustering_keys() {
        let rows = vec![
            Row {
                columns: vec![
                    Some(CqlValue::Text("id".to_string())),
                    Some(CqlValue::Text("uuid".to_string())),
                    Some(CqlValue::Text("partition_key".to_string())),
                ],
            },
            Row {
                columns: vec![
                    Some(CqlValue::Ascii("created_at".to_string())),
                    Some(CqlValue::Text("timestamp".to_string())),
                    Some(CqlValue::Text("clustering".to_string())),
                ],
            },
            Row {
                columns: vec![
                    Some(CqlValue::Text("name".to_string())),
                    Some(CqlValue::Text("text".to_string())),
                    Some(CqlValue::Text("regular".to_string())),
                ],
            },
        ];
        let nodes = column_rows_to_nodes(rows);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].name, "id");
        assert!(nodes[0].detail.primary_key);
        assert_eq!(nodes[1].name, "created_at");
        assert!(nodes[1].detail.primary_key);
        assert_eq!(nodes[2].name, "name");
        assert!(!nodes[2].detail.primary_key);
        assert_eq!(nodes[2].detail.type_name, Some("text".to_string()));
    }

    #[test]
    fn column_rows_to_nodes_skips_a_row_with_no_name_column() {
        let rows = vec![Row {
            columns: vec![None],
        }];
        assert!(column_rows_to_nodes(rows).is_empty());
    }

    #[test]
    fn eager_rows_yields_its_batch_once_then_none() {
        let batch = RowBatch {
            columns: vec![],
            rows: vec![],
        };
        let mut rows = EagerRows(Some(batch));
        assert!(rows.next_batch().unwrap().is_some());
        assert!(rows.next_batch().unwrap().is_none());
    }

    #[test]
    fn first_column_names_reads_the_first_text_column_of_each_row() {
        let rows = vec![
            Row {
                columns: vec![Some(CqlValue::Text("a".to_string()))],
            },
            Row {
                columns: vec![Some(CqlValue::Ascii("b".to_string()))],
            },
            Row {
                columns: vec![None],
            },
        ];
        assert_eq!(first_column_names(rows.into_iter()), vec!["a", "b"]);
    }
}

#[cfg(all(test, feature = "db-integration"))]
mod integration_tests {
    use super::*;

    /// Compiles a real connect+introspect+execute path against the
    /// `scylla` service `docker/db-compose.yml` adds (env
    /// `IDE_DB_CASSANDRA_HOSTS`) — not run in this sandbox (no network
    /// services available here), only proven to compile and type-check.
    #[test]
    #[ignore = "needs docker/db-compose.yml's scylla service"]
    fn connects_and_lists_keyspaces() {
        let hosts = std::env::var("IDE_DB_CASSANDRA_HOSTS").expect("IDE_DB_CASSANDRA_HOSTS");
        let host = hosts
            .split(',')
            .next()
            .expect("at least one host")
            .to_string();
        let (host, port) = host.split_once(':').unwrap_or((host.as_str(), "9042"));
        let spec = ConnectSpec {
            driver: "cassandra".to_string(),
            host: host.to_string(),
            port: Some(port.parse().unwrap_or(9042)),
            database: String::new(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: db_core::datasource::SslConfig {
                mode: SslMode::Disable,
                ca_file: None,
            },
        };
        let mut connection = CassandraDriver.connect(&spec).expect("connect");
        let snapshot = connection
            .introspect(&IntrospectScope::default(), IntrospectLevel::Names)
            .expect("introspect");
        assert!(!snapshot.roots.is_empty());
    }
}
