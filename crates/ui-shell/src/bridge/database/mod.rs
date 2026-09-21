//! Data sources (Database Tools plan F1.6): the Settings > Database
//! accessors and `DataSourceEditor` live in `settings.rs`; this module
//! holds what both share — the driver catalogue a plugin contributes, and
//! the `db_core`/`db_drivers` seam a real "Test connection" actually
//! drives.
//!
//! No `DatabaseService` dock-backing QObject yet: F1 has no console or
//! tree to hold a live `db_core::session::Session` open (that lands in
//! F3/F4), so a `connect`/`disconnect` pair with nothing to keep the
//! result in would just be `testConnection` under a second name — this
//! module adds that seam back once a real consumer needs to keep a
//! session alive across calls, not speculatively now (YAGNI).

pub mod service;
pub mod sessions;
pub mod settings;
pub mod tree;

pub use service::DatabaseServiceRust;

/// One driver a plugin contributes, mapped to the plain strings `db_core`
/// understands — `ui-shell` is the one place a `plugin_api::
/// DatabaseDriverContribution` ever turns into something `db-core`/
/// `db-drivers` read, per `database-tools.md` §7 ("never crossing
/// `plugin-api` into the driver crates").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverOption {
    pub id: String,
    pub name: String,
}

/// Every `database-drivers` contribution, gathered fresh on every call —
/// the same freshness `build_tools::contributed_build_tools` gives its own
/// catalogue.
pub fn driver_catalog() -> Vec<DriverOption> {
    plugin_host::registry()
        .database_drivers()
        .map(|(_, contribution)| DriverOption {
            id: contribution.id.clone(),
            name: contribution.name.clone(),
        })
        .collect()
}

/// Attempt a real connection through `db_drivers::DriverRegistry`, close it
/// immediately, and report which happened — the blocking half of "Test
/// connection", always run off the UI thread by its caller.
pub fn test_connection(spec: &db_core::datasource::ConnectSpec) -> Result<(), String> {
    let registry = db_drivers::DriverRegistry::builtin();
    let driver = registry
        .get(&spec.driver)
        .ok_or_else(|| format!("no driver registered for `{}`", spec.driver))?;
    let mut connection = driver.connect(spec).map_err(|error| error.to_string())?;
    let result = connection
        .execute(
            &db_core::driver::Statement::sql("SELECT 1"),
            &db_core::driver::ExecOptions::default(),
        )
        .map(|_| ())
        .map_err(|error| error.to_string());
    let _ = connection.close();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_against_an_in_memory_sqlite_database_succeeds() {
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
        assert_eq!(test_connection(&spec), Ok(()));
    }

    #[test]
    fn test_connection_against_an_unknown_driver_fails_with_a_typed_message() {
        let spec = db_core::datasource::ConnectSpec {
            driver: "does-not-exist".to_string(),
            host: String::new(),
            port: None,
            database: String::new(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: Default::default(),
        };
        assert!(test_connection(&spec)
            .unwrap_err()
            .contains("does-not-exist"));
    }
}
