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
// ---- database: F3 ----
pub mod console;
// ---- database: F8b ----
pub mod backend;
pub mod drivers;
// ---- database: F5b ----
pub mod exchange;

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
    pub backend: String,
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
            backend: contribution.backend.clone(),
        })
        .collect()
}

/// Looks `driver_id` up among the live registry's `database-drivers` rows
/// and resolves its `backend` (`backend::backend_for`) — the half of the
/// seam that touches `plugin_host::registry()`, kept out of
/// [`connect_with_backend`] so that function stays testable with a
/// hand-built [`backend::Backend`] and no live registry.
fn resolve_backend(driver_id: &str) -> Result<backend::Backend, String> {
    let contribution = plugin_host::registry()
        .database_drivers()
        .map(|(_, contribution)| contribution.clone())
        .find(|contribution| contribution.id == driver_id)
        .ok_or_else(|| format!("no driver registered for `{driver_id}`"))?;
    backend::backend_for(&contribution).map_err(|e| e.to_string())
}

/// Dispatches an already-resolved [`backend::Backend`] to the matching
/// driver crate — `db_drivers::DriverRegistry` (native),
/// `db_driver_adbc::AdbcDriver` (adbc, quarantine-guarded internally),
/// `db_driver_odbc::OdbcDriver` (odbc). Never crosses `plugin-api` into a
/// driver crate itself (database-tools.md §7, F8b).
pub fn connect_with_backend(
    backend: &backend::Backend,
    spec: &db_core::datasource::ConnectSpec,
) -> Result<Box<dyn db_core::driver::Connection>, String> {
    use db_core::driver::Driver as _;

    match backend {
        backend::Backend::Native { native_id } => {
            let registry = db_drivers::DriverRegistry::builtin();
            let driver = registry
                .get(native_id)
                .ok_or_else(|| format!("no native driver registered for `{native_id}`"))?;
            driver.connect(spec).map_err(|error| error.to_string())
        }
        backend::Backend::Adbc {
            manifest_name,
            entrypoint,
        } => {
            let config_dir = app_core::resolve_config_dir();
            let location = db_driver_adbc::locate::locate(
                &config_dir,
                manifest_name,
                backend::ADBC_INSTALLED_SLOT,
            );
            let driver = db_driver_adbc::AdbcDriver::with_entrypoint(
                manifest_name.clone(),
                location,
                config_dir,
                entrypoint.clone(),
            );
            driver.connect(spec).map_err(|error| error.to_string())
        }
        backend::Backend::Odbc => {
            let driver = db_driver_odbc::OdbcDriver::new();
            driver.connect(spec).map_err(|error| error.to_string())
        }
    }
}

/// The one place a `plugin_api::DatabaseDriverContribution` becomes an
/// actual connection: [`resolve_backend`] against the live registry, then
/// [`connect_with_backend`].
pub fn connect(
    spec: &db_core::datasource::ConnectSpec,
) -> Result<Box<dyn db_core::driver::Connection>, String> {
    let backend = resolve_backend(&spec.driver)?;
    connect_with_backend(&backend, spec)
}

/// Attempt a real connection through [`connect`], close it immediately,
/// and report which happened — the blocking half of "Test connection",
/// always run off the UI thread by its caller.
pub fn test_connection(spec: &db_core::datasource::ConnectSpec) -> Result<(), String> {
    let mut connection = connect(spec)?;
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

    fn sqlite_spec(database: &str) -> db_core::datasource::ConnectSpec {
        db_core::datasource::ConnectSpec {
            driver: "sqlite".to_string(),
            host: String::new(),
            port: None,
            database: database.to_string(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: Default::default(),
        }
    }

    /// Exercises the dispatch in [`connect_with_backend`] directly, with a
    /// hand-built `Backend` rather than a live plugin registry — see that
    /// function's own doc comment for why.
    #[test]
    fn a_native_backend_connects_through_the_driver_registry() {
        let backend = backend::Backend::Native {
            native_id: "sqlite".to_string(),
        };
        let connection = connect_with_backend(&backend, &sqlite_spec(":memory:"));
        assert!(connection.is_ok());
    }

    #[test]
    fn an_unknown_native_id_fails_with_a_typed_message() {
        let backend = backend::Backend::Native {
            native_id: "does-not-exist".to_string(),
        };
        // `.err()` rather than `.unwrap_err()`: `Box<dyn Connection>` (the
        // `Ok` side) implements no `Debug`, which `unwrap_err` requires
        // even though it never prints it.
        let error = connect_with_backend(&backend, &sqlite_spec(":memory:"))
            .err()
            .unwrap();
        assert!(error.contains("does-not-exist"));
    }

    #[test]
    fn resolve_backend_fails_with_a_typed_message_for_an_unregistered_driver_id() {
        // No plugin registry is loaded in a unit test process, so any id
        // is "unregistered" here — this exercises the not-found path, not
        // a real registry lookup (database-tools.md §7, F8b's split).
        let error = resolve_backend("does-not-exist").unwrap_err();
        assert!(error.contains("does-not-exist"));
    }
}
