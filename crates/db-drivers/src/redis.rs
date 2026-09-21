//! The Redis backend (F7.3): synchronous — bypasses the private tokio
//! runtime entirely, the same reasoning `sqlite.rs` gives (Redis's own
//! client is blocking already, forcing it through `block_on` would be
//! pure overhead).
//!
//! Introspection models Redis's own DBs 0–15 as [`ObjectKind::Catalog`]
//! nodes, and — once one is expanded — its keys as a `key_separator`
//! grouping into [`ObjectKind::KeyNamespace`]/[`ObjectKind::Key`]. The
//! RESP argv tokenizer and the read-only command table live in
//! `db_sql::resp`, shared with whatever writes the console side of this
//! (F7b).

use redis_driver::{Client, Connection as RawConnection, Value as RedisValue};

use db_core::datasource::{ConnectSpec, SslMode};
use db_core::dialect::Dialect;
use db_core::driver::{
    CancelHandle, Capabilities, Connection, Driver, ExecOptions, Execution, QueryLang, RowStream,
    Statement,
};
use db_core::error::{DbError, DbErrorCode};
use db_core::schema::{
    Children, IntrospectLevel, IntrospectScope, Node, NodeDetail, ObjectKind, ObjectRef, RedisType,
    SchemaSnapshot,
};
use db_core::value::{ColumnMeta, RowBatch, Value};
use db_sql::resp;

/// Redis always has exactly 16 logical databases (`databases` in
/// `redis.conf` defaults to 16, and this driver does not attempt to read
/// a server-specific override — see the module doc's F7.3 scope note).
const DB_COUNT: u32 = 16;
/// How many keys a single `SCAN` walk collects before stopping — the
/// F7.3 brief's default, overridable later through a per-source setting
/// this slice does not yet expose.
const DEFAULT_SCAN_LIMIT: usize = 10_000;
const DEFAULT_KEY_SEPARATOR: &str = ":";

fn map_err(error: redis_driver::RedisError) -> DbError {
    DbError::new(DbErrorCode::ConnectionFailed, error.to_string())
}

fn build_url(spec: &ConnectSpec) -> String {
    // ponytail: `SslMode::Prefer`/`Require`/`VerifyCa`/`VerifyFull` all map
    // to the same hard `rediss://` (the `redis` crate's TLS scheme has no
    // "try TLS, then fall back to plaintext" mode of its own to hand
    // `Prefer` off to) — only `Disable` differs. Revisit if a real Redis
    // deployment actually needs `Prefer`'s softer fallback behaviour.
    let scheme = if spec.ssl.mode == SslMode::Disable {
        "redis"
    } else {
        "rediss"
    };
    let host = if spec.host.is_empty() {
        "127.0.0.1"
    } else {
        spec.host.as_str()
    };
    let port = spec.port.unwrap_or(6379);
    let db_index: u32 = spec.database.trim().parse().unwrap_or(0);
    let auth = match (spec.user.as_str(), &spec.password) {
        ("", Some(password)) => format!(":{password}@"),
        (user, Some(password)) => format!("{user}:{password}@"),
        _ => String::new(),
    };
    format!("{scheme}://{auth}{host}:{port}/{db_index}")
}

pub struct RedisDriver;

impl Driver for RedisDriver {
    fn id(&self) -> &str {
        "redis"
    }

    fn capabilities(&self) -> Capabilities {
        // ponytail: Redis does support `MULTI`/`EXEC`, but wiring that
        // into `db_core::driver::Connection::begin`/`commit` is a
        // separate slice from F7.3's scan/execute/classify scope — revisit
        // once a console actually wants transactional Redis scripting.
        Capabilities::NONE
    }

    fn connect(&self, spec: &ConnectSpec) -> Result<Box<dyn Connection>, DbError> {
        let client = Client::open(build_url(spec)).map_err(map_err)?;
        let conn = client.get_connection().map_err(map_err)?;
        Ok(Box::new(RedisConnection {
            conn,
            scan_limit: DEFAULT_SCAN_LIMIT,
            key_separator: DEFAULT_KEY_SEPARATOR.to_string(),
        }))
    }
}

pub struct RedisConnection {
    conn: RawConnection,
    scan_limit: usize,
    key_separator: String,
}

fn redis_type_of(name: &str) -> Result<RedisType, DbError> {
    match name {
        "string" => Ok(RedisType::String),
        "list" => Ok(RedisType::List),
        "hash" => Ok(RedisType::Hash),
        "set" => Ok(RedisType::Set),
        "zset" => Ok(RedisType::SortedSet),
        "stream" => Ok(RedisType::Stream),
        other => Err(DbError::new(
            DbErrorCode::Unknown,
            format!("unrecognised Redis key type: {other}"),
        )),
    }
}

impl RedisConnection {
    /// Walks up to `scan_limit` keys of the currently selected DB via
    /// `SCAN` (never the blocking, whole-keyspace `KEYS`), then groups
    /// them by `key_separator` into a namespace tree.
    /// ponytail: one level of grouping (the first `key_separator`-delimited
    /// segment), not the fully recursive tree a deeply nested key naming
    /// scheme (`a:b:c:d`) would want — upgrade to recursive grouping if a
    /// real project's key namespace turns out to need more than one level
    /// to stay readable.
    fn scan_keys(&mut self) -> Result<Vec<Node>, DbError> {
        let mut keys = Vec::new();
        let mut cursor: u64 = 0;
        loop {
            let (next_cursor, batch): (u64, Vec<String>) = redis_driver::cmd("SCAN")
                .arg(cursor)
                .arg("COUNT")
                .arg(1000)
                .query(&mut self.conn)
                .map_err(map_err)?;
            keys.extend(batch);
            cursor = next_cursor;
            if cursor == 0 || keys.len() >= self.scan_limit {
                break;
            }
        }
        keys.truncate(self.scan_limit);

        let mut by_namespace: std::collections::BTreeMap<Option<String>, Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        for key in keys {
            let namespace = key
                .split_once(self.key_separator.as_str())
                .map(|(prefix, _)| prefix.to_string());
            let leaf = match &namespace {
                Some(prefix) => key[prefix.len() + self.key_separator.len()..].to_string(),
                None => key.clone(),
            };
            by_namespace.entry(namespace).or_default().push((leaf, key));
        }

        let mut roots = Vec::with_capacity(by_namespace.len());
        for (namespace, entries) in by_namespace {
            let mut children = Vec::with_capacity(entries.len());
            for (leaf, full_key) in entries {
                let type_name: String = redis_driver::cmd("TYPE")
                    .arg(&full_key)
                    .query(&mut self.conn)
                    .map_err(map_err)?;
                let redis_type = redis_type_of(&type_name)?;
                let ttl: i64 = redis_driver::cmd("TTL")
                    .arg(&full_key)
                    .query(&mut self.conn)
                    .map_err(map_err)?;
                let detail = NodeDetail {
                    ttl_seconds: (ttl >= 0).then_some(ttl),
                    ..NodeDetail::default()
                };
                children.push(Node::leaf(leaf, ObjectKind::Key(redis_type)).with_detail(detail));
            }
            match namespace {
                Some(name) => roots.push(Node::with_children(
                    name,
                    ObjectKind::KeyNamespace,
                    children,
                )),
                None => roots.extend(children),
            }
        }
        Ok(roots)
    }
}

/// Every row of a `next_batch` call is already in memory — a Redis reply
/// is never paged server-side the way a SQL cursor is.
struct RedisRowStream(Option<RowBatch>);

impl RowStream for RedisRowStream {
    fn next_batch(&mut self) -> Result<Option<RowBatch>, DbError> {
        Ok(self.0.take())
    }
}

/// A scalar reply becomes one column, one row. `Nil` becomes `NULL`. An
/// array becomes one row per item (one column, unless every item is
/// itself a two-element `[key, value]` pair, which the `HGETALL`/
/// `CONFIG GET`-shaped "hash reply" case below renders as two columns
/// instead). A `Map` reply renders as `key`/`value` columns directly.
fn value_to_batch(value: RedisValue, flat_pairs_as_map: bool) -> RowBatch {
    let column = |name: &str| ColumnMeta {
        name: name.to_string(),
        type_name: "redis".to_string(),
        nullable: true,
    };
    match value {
        RedisValue::Nil => RowBatch {
            columns: vec![column("value")],
            rows: vec![vec![Value::Null]],
        },
        RedisValue::Map(pairs) => RowBatch {
            columns: vec![column("key"), column("value")],
            rows: pairs
                .into_iter()
                .map(|(k, v)| vec![scalar(k), scalar(v)])
                .collect(),
        },
        RedisValue::Array(items) | RedisValue::Set(items) => {
            if flat_pairs_as_map && items.len() % 2 == 0 && !items.is_empty() {
                let mut rows = Vec::with_capacity(items.len() / 2);
                let mut iter = items.into_iter();
                while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                    rows.push(vec![scalar(k), scalar(v)]);
                }
                RowBatch {
                    columns: vec![column("key"), column("value")],
                    rows,
                }
            } else {
                RowBatch {
                    columns: vec![column("value")],
                    rows: items.into_iter().map(|item| vec![scalar(item)]).collect(),
                }
            }
        }
        other => RowBatch {
            columns: vec![column("value")],
            rows: vec![vec![scalar(other)]],
        },
    }
}

fn scalar(value: RedisValue) -> Value {
    match value {
        RedisValue::Nil => Value::Null,
        RedisValue::Int(i) => Value::Int(i),
        RedisValue::Double(f) => Value::Float(f),
        RedisValue::Boolean(b) => Value::Bool(b),
        RedisValue::BulkString(bytes) => Value::Text(String::from_utf8_lossy(&bytes).into_owned()),
        RedisValue::SimpleString(text) | RedisValue::VerbatimString { text, .. } => {
            Value::Text(text)
        }
        RedisValue::Okay => Value::Text("OK".to_string()),
        RedisValue::BigNumber(n) => Value::Text(n.to_string()),
        other => Value::Other {
            type_name: "redis".to_string(),
            display: format!("{other:?}"),
        },
    }
}

/// `HGETALL`/similar commands whose flat array reply is actually a
/// key/value hash, not a plain list — checked by command name since RESP2
/// gives both shapes the same `Array` variant.
fn is_hash_shaped_reply(command: &str) -> bool {
    matches!(command.to_ascii_uppercase().as_str(), "HGETALL" | "CONFIG")
}

impl Connection for RedisConnection {
    fn dialect(&self) -> Dialect {
        Dialect::Redis
    }

    fn server_info(&self) -> String {
        "Redis".to_string()
    }

    fn introspect(
        &mut self,
        scope: &IntrospectScope,
        level: IntrospectLevel,
    ) -> Result<SchemaSnapshot, DbError> {
        match &scope.catalog {
            None => {
                let roots = (0..DB_COUNT)
                    .map(|db| Node::leaf(db.to_string(), ObjectKind::Catalog))
                    .collect();
                Ok(SchemaSnapshot::new(level, roots))
            }
            Some(catalog) => {
                let db_index: u32 = catalog.parse().map_err(|_| {
                    DbError::new(
                        DbErrorCode::InvalidStatement,
                        format!("not a Redis DB index: {catalog}"),
                    )
                })?;
                redis_driver::cmd("SELECT")
                    .arg(db_index)
                    .query::<()>(&mut self.conn)
                    .map_err(map_err)?;
                let children = self.scan_keys()?;
                Ok(SchemaSnapshot::new(
                    level,
                    vec![Node {
                        name: catalog.clone(),
                        kind: ObjectKind::Catalog,
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
        if statement.lang != QueryLang::RedisCommand {
            return Err(DbError::new(
                DbErrorCode::InvalidStatement,
                "the Redis connection only executes RedisCommand statements",
            ));
        }
        let lines = resp::split_lines(&statement.text);
        if lines.is_empty() {
            return Ok(Execution::Ok);
        }

        let mut results = Vec::with_capacity(lines.len());
        for line in lines {
            let args = resp::tokenize(line).map_err(|error| {
                DbError::new(
                    DbErrorCode::InvalidStatement,
                    format!("{line:?}: {error:?}"),
                )
            })?;
            let Some(command_name) = args.first() else {
                continue;
            };
            if (options.read_only) && !resp::is_read_only_command(command_name) {
                return Err(DbError::new(
                    DbErrorCode::ReadOnlyViolation,
                    format!("{command_name} is not allowed on a read-only data source"),
                ));
            }
            let mut cmd = redis_driver::cmd(command_name);
            for arg in &args[1..] {
                cmd.arg(arg);
            }
            let reply: RedisValue = cmd.query(&mut self.conn).map_err(map_err)?;
            let batch = value_to_batch(reply, is_hash_shaped_reply(command_name));
            results.push(Execution::Rows(Box::new(RedisRowStream(Some(batch)))));
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
            "Redis transactions (MULTI/EXEC) are not yet wired into this connection",
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
        // Client-side only (`execute`'s `options.read_only` check) — Redis
        // itself has no per-connection read-only mode short of a replica.
        Ok(())
    }

    fn cancel_handle(&self) -> Option<Box<dyn CancelHandle>> {
        // Redis has no per-command server-side cancel; `CancelToken` (the
        // consumer-side layer) is all a caller gets — matching F7.3's
        // "cancel = drop the connection" brief.
        None
    }

    fn ddl_of(&mut self, _object: &ObjectRef) -> Result<String, DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "Redis has no DDL to show",
        ))
    }

    fn apply(&mut self, _statements: &[Statement]) -> Result<u64, DbError> {
        Err(DbError::new(
            DbErrorCode::NotSupported,
            "Redis has no EditBuffer-shaped writes — issue RedisCommand statements directly",
        ))
    }

    fn close(&mut self) -> Result<(), DbError> {
        // Cancel = drop the connection (F7.3 brief) — `RawConnection`
        // itself closes its socket on `Drop`, nothing else to do here.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_uses_redis_scheme_when_tls_is_disabled() {
        let spec = ConnectSpec {
            driver: "redis".to_string(),
            host: "cache.internal".to_string(),
            port: Some(6380),
            database: "3".to_string(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: db_core::datasource::SslConfig {
                mode: SslMode::Disable,
                ca_file: None,
            },
        };
        assert_eq!(build_url(&spec), "redis://cache.internal:6380/3");
    }

    #[test]
    fn build_url_uses_rediss_scheme_and_embeds_credentials_when_tls_is_on() {
        let spec = ConnectSpec {
            driver: "redis".to_string(),
            host: "cache.internal".to_string(),
            port: None,
            database: "0".to_string(),
            user: "app".to_string(),
            url: String::new(),
            password: Some("s3cret".to_string()),
            ssl: db_core::datasource::SslConfig {
                mode: SslMode::Require,
                ca_file: None,
            },
        };
        assert_eq!(
            build_url(&spec),
            "rediss://app:s3cret@cache.internal:6379/0"
        );
    }

    #[test]
    fn build_url_defaults_host_port_and_db_index() {
        let spec = ConnectSpec {
            driver: "redis".to_string(),
            host: String::new(),
            port: None,
            database: String::new(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: db_core::datasource::SslConfig {
                mode: SslMode::Disable,
                ca_file: None,
            },
        };
        assert_eq!(build_url(&spec), "redis://127.0.0.1:6379/0");
    }

    #[test]
    fn redis_type_of_maps_every_known_type_name() {
        assert_eq!(redis_type_of("string").unwrap(), RedisType::String);
        assert_eq!(redis_type_of("list").unwrap(), RedisType::List);
        assert_eq!(redis_type_of("hash").unwrap(), RedisType::Hash);
        assert_eq!(redis_type_of("set").unwrap(), RedisType::Set);
        assert_eq!(redis_type_of("zset").unwrap(), RedisType::SortedSet);
        assert_eq!(redis_type_of("stream").unwrap(), RedisType::Stream);
        assert!(redis_type_of("nonesuch").is_err());
    }

    #[test]
    fn a_nil_reply_becomes_one_null_cell() {
        let batch = value_to_batch(RedisValue::Nil, false);
        assert_eq!(batch.rows, vec![vec![Value::Null]]);
    }

    #[test]
    fn a_scalar_reply_becomes_one_cell() {
        let batch = value_to_batch(RedisValue::Int(42), false);
        assert_eq!(batch.rows, vec![vec![Value::Int(42)]]);
    }

    #[test]
    fn a_plain_array_reply_becomes_one_row_per_item() {
        let batch = value_to_batch(
            RedisValue::Array(vec![
                RedisValue::BulkString(b"a".to_vec()),
                RedisValue::BulkString(b"b".to_vec()),
            ]),
            false,
        );
        assert_eq!(
            batch.rows,
            vec![
                vec![Value::Text("a".to_string())],
                vec![Value::Text("b".to_string())],
            ]
        );
        assert_eq!(batch.columns.len(), 1);
    }

    #[test]
    fn a_hash_shaped_flat_array_becomes_key_value_columns() {
        let batch = value_to_batch(
            RedisValue::Array(vec![
                RedisValue::BulkString(b"field1".to_vec()),
                RedisValue::BulkString(b"value1".to_vec()),
                RedisValue::BulkString(b"field2".to_vec()),
                RedisValue::BulkString(b"value2".to_vec()),
            ]),
            true,
        );
        assert_eq!(batch.columns.len(), 2);
        assert_eq!(
            batch.rows,
            vec![
                vec![
                    Value::Text("field1".to_string()),
                    Value::Text("value1".to_string())
                ],
                vec![
                    Value::Text("field2".to_string()),
                    Value::Text("value2".to_string())
                ],
            ]
        );
    }

    #[test]
    fn a_map_reply_becomes_key_value_columns_directly() {
        let batch = value_to_batch(
            RedisValue::Map(vec![(
                RedisValue::BulkString(b"k".to_vec()),
                RedisValue::Int(1),
            )]),
            false,
        );
        assert_eq!(
            batch.rows,
            vec![vec![Value::Text("k".to_string()), Value::Int(1)]]
        );
    }

    #[test]
    fn is_hash_shaped_reply_recognises_hgetall_case_insensitively() {
        assert!(is_hash_shaped_reply("HGETALL"));
        assert!(is_hash_shaped_reply("hgetall"));
        assert!(is_hash_shaped_reply("CONFIG"));
        assert!(!is_hash_shaped_reply("LRANGE"));
    }
}
