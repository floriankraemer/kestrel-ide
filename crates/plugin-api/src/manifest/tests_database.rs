//! Unit tests for the `database-drivers` and `sql-dialects` contribution
//! points (Database Tools plan F1.5) — split from `tests.rs` to keep both
//! files under the file-size ceiling.
use super::tests::with;
use super::*;

#[test]
fn a_native_database_driver_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "sqlite"
            name = "SQLite"
            family = "sqlite"
            backend = "native"
            native-id = "sqlite"
            url-template = "sqlite://{database}"
            "#,
    ))
    .expect("valid");
    let drivers = &manifest.contributes.database_drivers;
    assert_eq!(drivers.len(), 1);
    assert_eq!(drivers[0].id, "sqlite");
    assert_eq!(drivers[0].backend, "native");
    assert_eq!(drivers[0].native_id.as_deref(), Some("sqlite"));
    assert!(!manifest.contributes.is_empty());
    assert_eq!(ContributionPoint::DatabaseDrivers.key(), "database-drivers");
}

#[test]
fn a_native_database_driver_needs_no_wasm_component() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "postgresql"
            name = "PostgreSQL"
            family = "postgresql"
            backend = "native"
            native-id = "postgresql"
            default-port = 5432
            dump-tool = "pg_dump"
            "#,
    ))
    .expect("valid");
    assert!(manifest.wasm.is_none());
    assert_eq!(
        manifest.contributes.database_drivers[0].default_port,
        Some(5432)
    );
}

#[test]
fn a_native_driver_without_a_native_id_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "postgresql"
            name = "PostgreSQL"
            family = "postgresql"
            backend = "native"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn an_unknown_backend_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "mystery"
            name = "Mystery"
            family = "mystery"
            backend = "telepathic"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn an_adbc_driver_needs_a_manifest_name_or_an_artifact() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "mssql"
            name = "SQL Server"
            family = "mssql"
            backend = "adbc"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn an_adbc_driver_with_a_manifest_name_alone_is_accepted() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "mssql"
            name = "SQL Server"
            family = "mssql"
            backend = "adbc"

            [contributes.database-drivers.adbc]
            manifest-name = "adbc_driver_mssql"
            "#,
    ))
    .expect("valid");
    assert_eq!(manifest.contributes.database_drivers.len(), 1);
}

#[test]
fn an_adbc_artifact_url_must_be_https() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "duckdb"
            name = "DuckDB"
            family = "duckdb"
            backend = "adbc"

            [contributes.database-drivers.adbc]
            url = "http://example.com/libduckdb.so"
            sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn an_adbc_artifact_needs_a_well_formed_sha256() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "duckdb"
            name = "DuckDB"
            family = "duckdb"
            backend = "adbc"

            [contributes.database-drivers.adbc]
            url = "https://example.com/libduckdb.so"
            sha256 = "not-a-hash"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn an_adbc_artifact_with_a_valid_url_and_sha256_is_accepted() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "duckdb"
            name = "DuckDB"
            family = "duckdb"
            backend = "adbc"

            [contributes.database-drivers.adbc]
            url = "https://example.com/libduckdb.so"
            sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            "#,
    ))
    .expect("valid");
    assert_eq!(manifest.contributes.database_drivers.len(), 1);
}

#[test]
fn per_platform_artifacts_round_trip_with_an_entrypoint_override() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "duckdb"
            name = "DuckDB"
            family = "duckdb"
            backend = "adbc"

            [contributes.database-drivers.adbc]
            manifest-name = "duckdb"
            entrypoint = "duckdb_adbc_init"

            [contributes.database-drivers.adbc.artifacts.linux_amd64]
            url = "https://example.com/libduckdb-linux-amd64.zip"
            sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            library = "libduckdb.so"

            [contributes.database-drivers.adbc.artifacts.windows_amd64]
            url = "https://example.com/libduckdb-windows-amd64.zip"
            sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            library = "duckdb.dll"
            "#,
    ))
    .expect("valid");
    let adbc = manifest.contributes.database_drivers[0]
        .adbc
        .as_ref()
        .unwrap();
    assert_eq!(adbc.entrypoint.as_deref(), Some("duckdb_adbc_init"));
    assert_eq!(adbc.artifacts.len(), 2);
    assert_eq!(adbc.artifacts["linux_amd64"].library, "libduckdb.so");
    assert_eq!(adbc.artifacts["windows_amd64"].library, "duckdb.dll");
}

#[test]
fn a_per_platform_artifact_url_must_also_be_https() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "duckdb"
            name = "DuckDB"
            family = "duckdb"
            backend = "adbc"

            [contributes.database-drivers.adbc]
            manifest-name = "duckdb"

            [contributes.database-drivers.adbc.artifacts.linux_amd64]
            url = "http://example.com/libduckdb.zip"
            sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            library = "libduckdb.so"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn an_install_hint_alone_satisfies_no_artifact_requirement_only_with_a_manifest_name() {
    // `install-hint` is informational, not a substitute for `manifest-name`
    // or an artifact — a row with only a hint is still rejected.
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "trino"
            name = "Trino"
            family = "trino"
            backend = "adbc"

            [contributes.database-drivers.adbc]
            install-hint = "No ADBC driver for Trino is published."
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn a_manifest_name_with_an_install_hint_is_accepted() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "trino"
            name = "Trino"
            family = "trino"
            backend = "adbc"

            [contributes.database-drivers.adbc]
            manifest-name = "trino"
            install-hint = "No ADBC driver for Trino is published; use the ODBC backend instead."
            "#,
    ))
    .expect("valid");
    assert_eq!(
        manifest.contributes.database_drivers[0]
            .adbc
            .as_ref()
            .unwrap()
            .install_hint
            .as_deref(),
        Some("No ADBC driver for Trino is published; use the ODBC backend instead.")
    );
}

#[test]
fn a_zero_default_port_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "sqlite"
            name = "SQLite"
            family = "sqlite"
            backend = "native"
            native-id = "sqlite"
            default-port = 0
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn a_url_template_with_an_unlisted_placeholder_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "sqlite"
            name = "SQLite"
            family = "sqlite"
            backend = "native"
            native-id = "sqlite"
            url-template = "sqlite://{password}"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn a_url_template_with_only_allow_listed_placeholders_is_accepted() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "postgresql"
            name = "PostgreSQL"
            family = "postgresql"
            backend = "native"
            native-id = "postgresql"
            url-template = "postgresql://{user}@{host}:{port}/{database}"
            "#,
    ))
    .expect("valid");
    assert_eq!(manifest.contributes.database_drivers.len(), 1);
}

#[test]
fn duplicate_database_driver_ids_in_one_manifest_are_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.database-drivers]]
            id = "sqlite"
            name = "SQLite"
            family = "sqlite"
            backend = "native"
            native-id = "sqlite"

            [[contributes.database-drivers]]
            id = "sqlite"
            name = "SQLite, again"
            family = "sqlite"
            backend = "native"
            native-id = "sqlite"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "database-drivers",
            id: "sqlite".to_string(),
        }
    );
}

#[test]
fn a_sql_dialect_contribution_round_trips() {
    let manifest = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.sql-dialects]]
            id = "postgresql"
            name = "PostgreSQL"
            parser = "sql"
            identifier-quote = "double"
            param-style = "dollar"
            keywords = "dialects/postgresql.keywords"
            "#,
    ))
    .expect("valid");
    let dialects = &manifest.contributes.sql_dialects;
    assert_eq!(dialects.len(), 1);
    assert_eq!(dialects[0].parser, "sql");
    assert_eq!(dialects[0].identifier_quote, "double");
    assert_eq!(dialects[0].param_style, "dollar");
    assert_eq!(
        dialects[0].keywords,
        Some(PathBuf::from("dialects/postgresql.keywords"))
    );
    assert!(manifest.wasm.is_none());
    assert_eq!(ContributionPoint::SqlDialects.key(), "sql-dialects");
}

#[test]
fn an_unknown_parser_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.sql-dialects]]
            id = "postgresql"
            name = "PostgreSQL"
            parser = "plsql"
            identifier-quote = "double"
            param-style = "dollar"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn an_unknown_identifier_quote_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.sql-dialects]]
            id = "postgresql"
            name = "PostgreSQL"
            parser = "sql"
            identifier-quote = "curly"
            param-style = "dollar"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn an_unknown_param_style_is_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.sql-dialects]]
            id = "postgresql"
            name = "PostgreSQL"
            parser = "sql"
            identifier-quote = "double"
            param-style = "percent"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::MalformedManifest(_)));
}

#[test]
fn a_dialect_keywords_path_may_not_escape_the_plugin_directory() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.sql-dialects]]
            id = "postgresql"
            name = "PostgreSQL"
            parser = "sql"
            identifier-quote = "double"
            param-style = "dollar"
            keywords = "../../etc/passwd"
            "#,
    ))
    .unwrap_err();
    assert!(matches!(err, LoadErrorKind::UnsafePath { .. }));
}

#[test]
fn duplicate_sql_dialect_ids_in_one_manifest_are_rejected() {
    let err = PluginManifest::from_toml_str(&with(
        r#"
            [[contributes.sql-dialects]]
            id = "postgresql"
            name = "PostgreSQL"
            parser = "sql"
            identifier-quote = "double"
            param-style = "dollar"

            [[contributes.sql-dialects]]
            id = "postgresql"
            name = "PostgreSQL, again"
            parser = "sql"
            identifier-quote = "double"
            param-style = "dollar"
            "#,
    ))
    .unwrap_err();
    assert_eq!(
        err,
        LoadErrorKind::DuplicateContributionId {
            point: "sql-dialects",
            id: "postgresql".to_string(),
        }
    );
}
