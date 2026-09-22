//! NFR bench (database-tools-plan.md §5/§10, "Introspection"): a 5 000-
//! table SQLite catalog, 20 columns each, timing `IntrospectLevel::Names`
//! and `IntrospectLevel::Columns` against the targets in the NFR table
//! (`Names` <= 1.5 s, `Columns` <= 10 s).
//!
//! Gated behind `db-integration` like every other slow/real-backend test
//! in this crate (`docs/architecture/db-integration.md`) — `make test`
//! never builds this module; `make test-db` / a direct `cargo test
//! --features db-integration -- bench --nocapture` does, and prints the
//! measured numbers this task recorded in `database-tools.md` §10.

use std::time::Instant;

use db_core::driver::Connection;
use db_core::schema::{IntrospectLevel, IntrospectScope};

use crate::sqlite::SqliteConnection;

const TABLE_COUNT: usize = 5_000;
const COLUMN_COUNT: usize = 20;

fn seed_schema(conn: &rusqlite::Connection) {
    conn.execute_batch("BEGIN").unwrap();
    for t in 0..TABLE_COUNT {
        let columns: Vec<String> = (0..COLUMN_COUNT)
            .map(|c| format!("col_{c} INTEGER"))
            .collect();
        conn.execute(&format!("CREATE TABLE t_{t} ({})", columns.join(", ")), [])
            .unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
}

#[test]
fn bench_5000_table_introspection() {
    let raw = rusqlite::Connection::open_in_memory().unwrap();
    seed_schema(&raw);
    let mut conn = SqliteConnection::wrap(raw);

    let start = Instant::now();
    let names = conn
        .introspect(&IntrospectScope::default(), IntrospectLevel::Names)
        .unwrap();
    let names_elapsed = start.elapsed();
    assert_eq!(names.roots.len(), TABLE_COUNT);

    let start = Instant::now();
    let columns = conn
        .introspect(&IntrospectScope::default(), IntrospectLevel::Columns)
        .unwrap();
    let columns_elapsed = start.elapsed();
    assert_eq!(columns.roots.len(), TABLE_COUNT);

    println!(
        "bench_5000_table_introspection: Names={:?} (target <= 1.5s), Columns={:?} (target <= 10s)",
        names_elapsed, columns_elapsed
    );

    assert!(
        names_elapsed.as_secs_f64() <= 1.5,
        "Names-level introspection took {names_elapsed:?}, target is <= 1.5s"
    );
    assert!(
        columns_elapsed.as_secs_f64() <= 10.0,
        "Columns-level introspection took {columns_elapsed:?}, target is <= 10s"
    );
}
