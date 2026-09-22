//! `db-drivers`: the native backend crate (ADR-0058) — PostgreSQL, MySQL,
//! SQLite, MongoDB, Redis, Cassandra/Scylla, sqlite and postgres landing in
//! F1, the rest through F7. Owns a private `tokio::Runtime` every async
//! driver's calls are `block_on`'d against; SQLite and Redis bypass it
//! entirely, since both have synchronous APIs.

#[cfg(any(
    feature = "postgres",
    feature = "mongodb",
    feature = "cassandra",
    feature = "ssh"
))]
use std::sync::OnceLock;

use db_core::driver::Driver;

#[cfg(feature = "cassandra")]
pub mod cassandra;
#[cfg(feature = "mongodb")]
pub mod mongodb;
#[cfg(feature = "mysql")]
pub mod mysql;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "redis")]
pub mod redis;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "ssh")]
pub mod ssh;

#[cfg(all(
    test,
    feature = "db-integration",
    any(feature = "postgres", feature = "mysql")
))]
mod testsupport;

#[cfg(all(test, feature = "db-integration", feature = "sqlite"))]
mod sqlite_bench;

#[cfg(any(
    feature = "postgres",
    feature = "mongodb",
    feature = "cassandra",
    feature = "ssh"
))]
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// The shared private runtime every async driver call blocks on
/// (`OnceLock`, 2 workers named `"db-io"` — ADR-0058 §1). Never the
/// ambient kind: `db-core` and every consumer above it stay entirely
/// synchronous, and this is the one place tokio actually runs anything.
/// SQLite and Redis bypass it entirely (both have synchronous APIs).
#[cfg(any(
    feature = "postgres",
    feature = "mongodb",
    feature = "cassandra",
    feature = "ssh"
))]
pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("db-io")
            .enable_all()
            .build()
            .expect("failed to start the db-io runtime")
    })
}

/// Every backend this crate ships, by id — what `ui-shell` maps a
/// `database-drivers` plugin contribution's `native-id` onto.
pub struct DriverRegistry {
    drivers: Vec<Box<dyn Driver>>,
}

impl DriverRegistry {
    /// The drivers compiled into this build (gated by cargo feature, both
    /// on by default).
    #[allow(clippy::vec_init_then_push)] // each push is gated by its own cargo feature
    pub fn builtin() -> Self {
        let mut drivers: Vec<Box<dyn Driver>> = Vec::new();
        #[cfg(feature = "sqlite")]
        drivers.push(Box::new(sqlite::SqliteDriver));
        #[cfg(feature = "postgres")]
        drivers.push(Box::new(postgres::PostgresDriver));
        #[cfg(feature = "mysql")]
        drivers.push(Box::new(mysql::MySqlDriver));
        #[cfg(feature = "mongodb")]
        drivers.push(Box::new(mongodb::MongoDriver));
        #[cfg(feature = "redis")]
        drivers.push(Box::new(redis::RedisDriver));
        #[cfg(feature = "cassandra")]
        drivers.push(Box::new(cassandra::CassandraDriver));
        Self { drivers }
    }

    pub fn get(&self, id: &str) -> Option<&dyn Driver> {
        self.drivers
            .iter()
            .map(|driver| driver.as_ref())
            .find(|driver| driver.id() == id)
    }

    pub fn ids(&self) -> Vec<&str> {
        self.drivers.iter().map(|driver| driver.id()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registers_sqlite_and_postgres() {
        let registry = DriverRegistry::builtin();
        let ids = registry.ids();
        #[cfg(feature = "sqlite")]
        assert!(ids.contains(&"sqlite"));
        #[cfg(feature = "postgres")]
        assert!(ids.contains(&"postgresql"));
        #[cfg(feature = "mysql")]
        assert!(ids.contains(&"mysql"));
    }

    #[test]
    fn get_returns_none_for_an_unknown_id() {
        assert!(DriverRegistry::builtin().get("does-not-exist").is_none());
    }
}
