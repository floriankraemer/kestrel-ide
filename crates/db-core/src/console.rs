//! Console file paths (database-tools.md §8): `<config_dir>/consoles/
//! <source-id>/console*.<ext>`, and `source_of`, the reverse lookup a
//! console tab uses to find which data source it defaults to.
//!
//! [`Family`] is the console's own query language, one step coarser than
//! `db_core::dialect::Dialect` (every SQL dialect shares one console file
//! extension and highlighting) — [`extension`] is the one place that
//! family maps to a file extension, so a console file always opens under
//! the language `syntax-core`'s catalog already highlights it with
//! (`.mongodb` → the `javascript` grammar, close enough to the sugar's own
//! `db.coll.method(...)` shape and JSON documents; `.redis` → no grammar
//! registered, so it renders as plain text; `.cql` → the `sql` grammar,
//! CQL being SQL-*like* enough for `sqlparser`'s generic dialect to
//! tokenize usefully, the same call `db_sql::dialects`'s own doc comment
//! already makes for splitting/classification).

use std::path::{Path, PathBuf};

/// A console's query language family — coarser than `Dialect` (every SQL
/// dialect is one `Family::Sql`), and named after the driver id
/// (`plugin.toml`'s `family = "..."`/`native-id`) that decides it rather
/// than duplicating a second dialect enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Sql,
    Mongo,
    Redis,
    Cql,
}

/// Which [`Family`] a driver id's console belongs to — every built-in
/// native driver id not named here is a SQL dialect (`sqlite`,
/// `postgresql`, and every ADBC/ODBC row `plugin.toml` contributes).
pub fn family_for_driver(driver: &str) -> Family {
    match driver {
        "mongodb" => Family::Mongo,
        "redis" => Family::Redis,
        "cassandra" => Family::Cql,
        _ => Family::Sql,
    }
}

/// The console file extension a [`Family`] opens under — see this
/// module's own doc comment for why each one is what it is.
pub fn extension(family: Family) -> &'static str {
    match family {
        Family::Sql => "sql",
        Family::Mongo => "mongodb",
        Family::Redis => "redis",
        Family::Cql => "cql",
    }
}

/// A stable key (never shown verbatim — the view maps it to a `tr()`'d
/// label, ADR-0049) for what the Data Source dialog's "Database" field
/// actually means for `family` (F7b's Data Source dialog task): a Mongo
/// auth database, a Redis numeric db index, a Cassandra keyspace, or an
/// ordinary SQL database name. The dialog reads this once per driver
/// choice rather than hard-coding a per-family label switch of its own —
/// which family a driver belongs to still decided only here, in Rust.
pub fn database_field_label_key(family: Family) -> &'static str {
    match family {
        Family::Sql => "database",
        Family::Mongo => "auth_database",
        Family::Redis => "db_index",
        Family::Cql => "keyspace",
    }
}

/// The directory a source's console files live under.
pub fn console_dir(config_dir: &Path, source_id: &str) -> PathBuf {
    config_dir.join("consoles").join(source_id)
}

/// A fresh console file's path within its source's directory — callers
/// pick `index` (the next unused number) themselves; this only spells the
/// name, with `family`'s own extension (see this module's doc comment).
pub fn console_file(config_dir: &Path, source_id: &str, index: u32, family: Family) -> PathBuf {
    console_dir(config_dir, source_id).join(format!("console{index}.{}", extension(family)))
}

/// Which source id a console file belongs to, if `path` sits under
/// `config_dir/consoles/<id>/…` — `None` for a plain project file (a
/// `.sql` file opened directly, or one covered instead by the project's
/// own `file_sources` map, which this function knows nothing about; that
/// join is `settings_model::database`'s, not this one's).
pub fn source_of(path: &Path, config_dir: &Path) -> Option<String> {
    let relative = path.strip_prefix(config_dir).ok()?;
    let mut components = relative.components();
    if components.next()?.as_os_str() != "consoles" {
        return None;
    }
    let id = components.next()?.as_os_str().to_str()?.to_string();
    // Must actually name a file beneath the id directory, not just the
    // directory itself.
    components.next()?;
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_dir_nests_under_the_source_id() {
        let dir = console_dir(Path::new("/cfg"), "abc123");
        assert_eq!(dir, Path::new("/cfg/consoles/abc123"));
    }

    #[test]
    fn console_file_names_by_index() {
        let file = console_file(Path::new("/cfg"), "abc123", 2, Family::Sql);
        assert_eq!(file, Path::new("/cfg/consoles/abc123/console2.sql"));
    }

    #[test]
    fn each_nosql_family_opens_its_own_extension() {
        assert_eq!(extension(Family::Sql), "sql");
        assert_eq!(extension(Family::Mongo), "mongodb");
        assert_eq!(extension(Family::Redis), "redis");
        assert_eq!(extension(Family::Cql), "cql");
    }

    #[test]
    fn console_file_uses_the_family_s_extension() {
        let file = console_file(Path::new("/cfg"), "src1", 1, Family::Mongo);
        assert_eq!(file, Path::new("/cfg/consoles/src1/console1.mongodb"));
    }

    #[test]
    fn family_for_driver_recognises_every_nosql_backend() {
        assert!(matches!(family_for_driver("mongodb"), Family::Mongo));
        assert!(matches!(family_for_driver("redis"), Family::Redis));
        assert!(matches!(family_for_driver("cassandra"), Family::Cql));
        assert!(matches!(family_for_driver("postgresql"), Family::Sql));
        assert!(matches!(family_for_driver("sqlite"), Family::Sql));
        assert!(matches!(family_for_driver("odbc"), Family::Sql));
    }

    #[test]
    fn database_field_label_key_names_each_family_s_own_meaning() {
        assert_eq!(database_field_label_key(Family::Sql), "database");
        assert_eq!(database_field_label_key(Family::Mongo), "auth_database");
        assert_eq!(database_field_label_key(Family::Redis), "db_index");
        assert_eq!(database_field_label_key(Family::Cql), "keyspace");
    }

    #[test]
    fn source_of_reads_the_id_back_out_of_a_console_path() {
        let path = Path::new("/cfg/consoles/abc123/console1.sql");
        assert_eq!(
            source_of(path, Path::new("/cfg")),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn source_of_is_none_for_a_plain_project_file() {
        let path = Path::new("/project/sql/reports.sql");
        assert_eq!(source_of(path, Path::new("/cfg")), None);
    }

    #[test]
    fn source_of_is_none_for_the_consoles_directory_itself() {
        assert_eq!(
            source_of(Path::new("/cfg/consoles"), Path::new("/cfg")),
            None
        );
        assert_eq!(
            source_of(Path::new("/cfg/consoles/abc123"), Path::new("/cfg")),
            None
        );
    }

    #[test]
    fn source_of_is_none_outside_the_config_dir() {
        assert_eq!(
            source_of(
                Path::new("/other/consoles/abc123/console1.sql"),
                Path::new("/cfg")
            ),
            None
        );
    }
}
