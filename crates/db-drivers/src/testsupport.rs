//! Test support for the `db-integration`-gated real-Postgres tests
//! (`docs/architecture/db-integration.md`) — never built by `make test`/
//! `cargo test --workspace`, since the `db-integration` feature is not
//! enabled there, the exact shape `jvm-integration` already uses for
//! `jvm-build-core`.

use db_core::datasource::{ConnectSpec, SslConfig, SslMode};

/// `IDE_DB_POSTGRES_URL`, parsed into a [`ConnectSpec`] — `None` when the
/// env var is unset, so a real-server test can skip itself (rather than
/// panic) when nothing is configured, e.g. a developer running
/// `cargo test --features db-integration` locally without a Postgres
/// container up.
pub fn postgres_test_spec() -> Option<ConnectSpec> {
    let url = std::env::var("IDE_DB_POSTGRES_URL").ok()?;
    // `postgres://user:password@host:port/dbname` — a minimal parse; the
    // real URL grammar (query params, unix sockets, …) is not needed for
    // a fixed nightly-compose connection string.
    let rest = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))?;
    let (auth, hostpart) = rest.split_once('@').unwrap_or(("", rest));
    let (user, password) = match auth.split_once(':') {
        Some((user, password)) => (user.to_string(), Some(password.to_string())),
        None => (auth.to_string(), None),
    };
    let (hostport, database) = hostpart.split_once('/').unwrap_or((hostpart, ""));
    let (host, port) = match hostport.split_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().ok()),
        None => (hostport.to_string(), None),
    };
    Some(ConnectSpec {
        driver: "postgresql".to_string(),
        host,
        port,
        database: database.to_string(),
        user,
        url: String::new(),
        password,
        ssl: SslConfig {
            mode: SslMode::Disable,
            ca_file: None,
        },
    })
}

/// `IDE_DB_MYSQL_URL`/`IDE_DB_MARIADB_URL`, parsed the same minimal way as
/// [`postgres_test_spec`] (`mysql://user:password@host:port/dbname`) — a
/// second env var name so `db-ci` can run the same fixture-tested driver
/// against both engines in one pass without a second copy of this parser.
pub fn mysql_test_spec(env_var: &str) -> Option<ConnectSpec> {
    let url = std::env::var(env_var).ok()?;
    let rest = url.strip_prefix("mysql://")?;
    let (auth, hostpart) = rest.split_once('@').unwrap_or(("", rest));
    let (user, password) = match auth.split_once(':') {
        Some((user, password)) => (user.to_string(), Some(password.to_string())),
        None => (auth.to_string(), None),
    };
    let (hostport, database) = hostpart.split_once('/').unwrap_or((hostpart, ""));
    let (host, port) = match hostport.split_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().ok()),
        None => (hostport.to_string(), None),
    };
    Some(ConnectSpec {
        driver: "mysql".to_string(),
        host,
        port,
        database: database.to_string(),
        user,
        url: String::new(),
        password,
        ssl: SslConfig {
            mode: SslMode::Disable,
            ca_file: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_env_var_is_none_not_a_panic() {
        std::env::remove_var("IDE_DB_POSTGRES_URL");
        assert!(postgres_test_spec().is_none());
    }

    #[test]
    fn a_full_url_parses_into_every_field() {
        std::env::set_var(
            "IDE_DB_POSTGRES_URL",
            "postgres://ide:secret@localhost:5432/ide_test",
        );
        let spec = postgres_test_spec().expect("parsed");
        assert_eq!(spec.host, "localhost");
        assert_eq!(spec.port, Some(5432));
        assert_eq!(spec.database, "ide_test");
        assert_eq!(spec.user, "ide");
        assert_eq!(spec.password.as_deref(), Some("secret"));
        std::env::remove_var("IDE_DB_POSTGRES_URL");
    }

    #[test]
    fn a_url_without_credentials_still_parses() {
        std::env::set_var("IDE_DB_POSTGRES_URL", "postgres://localhost/db");
        let spec = postgres_test_spec().expect("parsed");
        assert_eq!(spec.host, "localhost");
        assert_eq!(spec.database, "db");
        assert_eq!(spec.user, "");
        assert_eq!(spec.password, None);
        std::env::remove_var("IDE_DB_POSTGRES_URL");
    }
}
