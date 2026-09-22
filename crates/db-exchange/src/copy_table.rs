//! Copy a table between two live connections (F5.4, database-tools-plan.md):
//! stream `SELECT *` from the source, synthesize a `CREATE TABLE` for the
//! target dialect from the first batch's [`ColumnMeta`] (unless the
//! caller says the table already exists), then batch parameterised
//! `INSERT`s, one transaction per batch, with a progress callback and a
//! [`CancelToken`] checked between batches.

use db_core::dialect::Dialect;
use db_core::driver::{CancelToken, Connection, ExecOptions, Execution, QueryLang, Statement};
use db_core::error::{DbError, DbErrorCode};
use db_core::value::{ColumnMeta, Value};

/// Rows fetched (and inserted) per batch, and whether to synthesize a
/// `CREATE TABLE` before the first insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyOptions {
    pub batch_size: u32,
    pub create_if_missing: bool,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            batch_size: 500,
            create_if_missing: true,
        }
    }
}

/// The narrow type category every mapping below picks from, keyed off
/// substrings of a source driver's own type name — every native driver's
/// `information_schema`/`PRAGMA table_info` reports type names this
/// loosely, so a substring match covers `VARCHAR(255)`, `character
/// varying`, `TEXT`, … under one case rather than an exhaustive per-driver
/// list this crate would have to keep in sync with six drivers it does
/// not otherwise depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeCategory {
    Int,
    Float,
    Bool,
    Date,
    Blob,
    Text,
}

fn classify(source_type_name: &str) -> TypeCategory {
    let upper = source_type_name.to_ascii_uppercase();
    if upper.contains("BOOL") {
        TypeCategory::Bool
    } else if upper.contains("BLOB") || upper.contains("BINARY") || upper.contains("BYTEA") {
        TypeCategory::Blob
    } else if upper.contains("INT") {
        TypeCategory::Int
    } else if upper.contains("REAL")
        || upper.contains("FLOA")
        || upper.contains("DOUB")
        || upper.contains("DEC")
        || upper.contains("NUMERIC")
    {
        TypeCategory::Float
    } else if upper.contains("DATE") || upper.contains("TIME") {
        TypeCategory::Date
    } else {
        TypeCategory::Text
    }
}

/// Map a source driver's own column type name onto the closest type in
/// `dialect` — the "source type name -> target dialect type" table the
/// plan asks for, expressed as one classifier plus one per-dialect match
/// rather than a hand-written row per (source type, target dialect) pair.
pub fn map_type(dialect: Dialect, source_type_name: &str) -> &'static str {
    use TypeCategory::*;
    match (dialect, classify(source_type_name)) {
        (Dialect::Sqlite, Int) => "INTEGER",
        (Dialect::Sqlite, Float) => "REAL",
        (Dialect::Sqlite, Bool) => "INTEGER",
        (Dialect::Sqlite, Blob) => "BLOB",
        (Dialect::Sqlite, Date | Text) => "TEXT",

        (Dialect::MySql, Int) => "BIGINT",
        (Dialect::MySql, Float) => "DOUBLE",
        (Dialect::MySql, Bool) => "TINYINT(1)",
        (Dialect::MySql, Blob) => "BLOB",
        (Dialect::MySql, Date) => "DATETIME",
        (Dialect::MySql, Text) => "TEXT",

        (Dialect::SqlServer, Int) => "BIGINT",
        (Dialect::SqlServer, Float) => "FLOAT",
        (Dialect::SqlServer, Bool) => "BIT",
        (Dialect::SqlServer, Blob) => "VARBINARY(MAX)",
        (Dialect::SqlServer, Date) => "DATETIME2",
        (Dialect::SqlServer, Text) => "NVARCHAR(MAX)",

        (Dialect::Cassandra, Int) => "bigint",
        (Dialect::Cassandra, Float) => "double",
        (Dialect::Cassandra, Bool) => "boolean",
        (Dialect::Cassandra, Blob) => "blob",
        (Dialect::Cassandra, Date) => "timestamp",
        (Dialect::Cassandra, Text) => "text",

        // Postgres, and Mongo/Redis (no SQL DDL of their own — falling
        // back to Postgres-shaped names keeps this total).
        (_, Int) => "BIGINT",
        (_, Float) => "DOUBLE PRECISION",
        (_, Bool) => "BOOLEAN",
        (_, Blob) => "BYTEA",
        (_, Date) => "TIMESTAMP",
        (_, Text) => "TEXT",
    }
}

fn create_table_ddl(dialect: Dialect, table: &str, columns: &[ColumnMeta]) -> String {
    let defs: Vec<String> = columns
        .iter()
        .map(|column| {
            let mapped = map_type(dialect, &column.type_name);
            let nullability = if column.nullable { "" } else { " NOT NULL" };
            format!(
                "{} {mapped}{nullability}",
                dialect.quote_ident(&column.name)
            )
        })
        .collect();
    format!(
        "CREATE TABLE {} ({})",
        dialect.quote_ident(table),
        defs.join(", ")
    )
}

fn insert_statement(
    dialect: Dialect,
    table: &str,
    columns: &[ColumnMeta],
    rows: &[Vec<Value>],
) -> Statement {
    let column_list: Vec<String> = columns
        .iter()
        .map(|c| dialect.quote_ident(&c.name))
        .collect();
    let mut params = Vec::with_capacity(rows.len() * columns.len());
    let mut groups = Vec::with_capacity(rows.len());
    for row in rows {
        let marks: Vec<&str> = columns.iter().map(|_| "?").collect();
        groups.push(format!("({})", marks.join(", ")));
        params.extend(row.iter().cloned());
    }
    Statement {
        lang: QueryLang::Sql,
        text: format!(
            "INSERT INTO {} ({}) VALUES {}",
            dialect.quote_ident(table),
            column_list.join(", "),
            groups.join(", ")
        ),
        params,
    }
}

/// Copy every row of `source_table` (on `source`) into `target_table` (on
/// `target`), synthesizing a `CREATE TABLE` first when
/// `options.create_if_missing`. `progress` is called with the running
/// total after each batch commits; `cancel` is checked between batches
/// (never mid-batch — a batch is one transaction, and half a transaction
/// left dangling on cancel would leave the target in a state neither
/// "before" nor "after"). Returns the total rows copied.
pub fn copy_table(
    source: &mut dyn Connection,
    target: &mut dyn Connection,
    source_table: &str,
    target_table: &str,
    options: &CopyOptions,
    mut progress: impl FnMut(u64),
    cancel: &CancelToken,
) -> Result<u64, DbError> {
    let select = Statement::sql(format!(
        "SELECT * FROM {}",
        source.dialect().quote_ident(source_table)
    ));
    let exec_options = ExecOptions {
        fetch_size: options.batch_size,
        ..ExecOptions::default()
    };
    let Execution::Rows(mut stream) = source.execute(&select, &exec_options)? else {
        return Err(DbError::new(
            DbErrorCode::Unknown,
            format!("SELECT * FROM {source_table} did not return rows"),
        ));
    };

    let target_dialect = target.dialect();
    let mut table_ready = !options.create_if_missing;
    let mut total = 0u64;

    while let Some(batch) = stream.next_batch()? {
        if cancel.is_cancelled() {
            break;
        }
        if !table_ready {
            let ddl = create_table_ddl(target_dialect, target_table, &batch.columns);
            target.execute(&Statement::sql(ddl), &ExecOptions::default())?;
            table_ready = true;
        }
        if batch.rows.is_empty() {
            continue;
        }
        let insert = insert_statement(target_dialect, target_table, &batch.columns, &batch.rows);
        target.begin()?;
        match target.execute(&insert, &ExecOptions::default()) {
            Ok(Execution::Affected(rows)) => {
                target.commit()?;
                total += rows;
            }
            Ok(_) => {
                target.commit()?;
            }
            Err(error) => {
                let _ = target.rollback();
                return Err(error);
            }
        }
        progress(total);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_drivers::sqlite::SqliteConnection;

    fn open(sql: &[&str]) -> SqliteConnection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for statement in sql {
            conn.execute(statement, []).unwrap();
        }
        SqliteConnection::wrap(conn)
    }

    #[test]
    fn map_type_classifies_common_sqlite_pragma_type_names() {
        assert_eq!(map_type(Dialect::Postgres, "INTEGER"), "BIGINT");
        assert_eq!(map_type(Dialect::Postgres, "TEXT"), "TEXT");
        assert_eq!(map_type(Dialect::Postgres, "REAL"), "DOUBLE PRECISION");
        assert_eq!(map_type(Dialect::Postgres, "BLOB"), "BYTEA");
        assert_eq!(map_type(Dialect::MySql, "VARCHAR(255)"), "TEXT");
        assert_eq!(map_type(Dialect::MySql, "BOOLEAN"), "TINYINT(1)");
    }

    #[test]
    fn map_type_falls_back_to_text_for_an_unrecognised_name() {
        assert_eq!(map_type(Dialect::Sqlite, "some_custom_enum"), "TEXT");
    }

    #[test]
    fn copy_table_creates_the_target_table_and_copies_every_row() {
        let mut source = open(&[
            "CREATE TABLE users (id INTEGER NOT NULL, name TEXT)",
            "INSERT INTO users VALUES (1, 'alice')",
            "INSERT INTO users VALUES (2, 'bob')",
        ]);
        let mut target = open(&[]);
        let cancel = CancelToken::new();
        let mut progress_calls = Vec::new();

        let total = copy_table(
            &mut source,
            &mut target,
            "users",
            "users",
            &CopyOptions::default(),
            |done| progress_calls.push(done),
            &cancel,
        )
        .unwrap();

        assert_eq!(total, 2);
        assert!(!progress_calls.is_empty());

        let select = target
            .execute(
                &Statement::sql("SELECT id, name FROM users ORDER BY id"),
                &ExecOptions::default(),
            )
            .unwrap();
        let Execution::Rows(mut stream) = select else {
            panic!("expected rows");
        };
        let batch = stream.next_batch().unwrap().unwrap();
        assert_eq!(batch.rows.len(), 2);
        assert_eq!(batch.rows[0][1], Value::Text("alice".to_string()));
    }

    #[test]
    fn copy_table_batches_by_the_configured_batch_size() {
        let mut inserts = vec!["CREATE TABLE t (n INTEGER)".to_string()];
        for i in 0..5 {
            inserts.push(format!("INSERT INTO t VALUES ({i})"));
        }
        let refs: Vec<&str> = inserts.iter().map(String::as_str).collect();
        let mut source = open(&refs);
        let mut target = open(&[]);
        let cancel = CancelToken::new();
        let mut batches_seen = 0;

        let total = copy_table(
            &mut source,
            &mut target,
            "t",
            "t",
            &CopyOptions {
                batch_size: 2,
                create_if_missing: true,
            },
            |_| batches_seen += 1,
            &cancel,
        )
        .unwrap();

        assert_eq!(total, 5);
        assert_eq!(batches_seen, 3); // 2 + 2 + 1
    }

    #[test]
    fn a_cancelled_token_stops_copying_between_batches() {
        let mut inserts = vec!["CREATE TABLE t (n INTEGER)".to_string()];
        for i in 0..10 {
            inserts.push(format!("INSERT INTO t VALUES ({i})"));
        }
        let refs: Vec<&str> = inserts.iter().map(String::as_str).collect();
        let mut source = open(&refs);
        let mut target = open(&[]);
        let cancel = CancelToken::new();
        cancel.cancel();

        let total = copy_table(
            &mut source,
            &mut target,
            "t",
            "t",
            &CopyOptions {
                batch_size: 2,
                create_if_missing: true,
            },
            |_| {},
            &cancel,
        )
        .unwrap();

        assert_eq!(total, 0);
    }
}
