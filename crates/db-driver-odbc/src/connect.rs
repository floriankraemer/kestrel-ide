//! Builds the ODBC connection string a `Environment::connect_with_connection_string`
//! call needs from a `db_core::ConnectSpec` — either a DSN (the data
//! source's `url` already names one, `DSN=...`) or an assembled
//! `Driver={…};Server=…;Port=…;Database=…;Uid=…;Pwd=…` string
//! (database-tools-plan.md F8.2).
//!
//! Every attribute value is escaped per the ODBC connection-string grammar
//! (MS-ODBCSTR): a value containing `;`, `=`, `{`, `}` or leading/trailing
//! whitespace is wrapped in braces, with any literal `}` inside doubled.

use db_core::datasource::ConnectSpec;

/// The assembled connection string, wrapped so nothing ever formats it
/// with `{:?}`/`{}` and leaks the password it carries (ADR-0061 §1) —
/// only [`ConnectionString::as_str`] reads it, right before the one
/// `SQLDriverConnect`/`SQLConnect` call that needs it.
pub struct ConnectionString(String);

impl ConnectionString {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ConnectionString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectionString(<redacted>)")
    }
}

/// Escapes one attribute value for the ODBC connection-string grammar.
/// Braces only when needed, so the common case (`Driver=SQLite3`) stays
/// exactly as readable as before.
fn escape_odbc_value(value: &str) -> String {
    let needs_braces = value.is_empty()
        || value
            .chars()
            .any(|c| matches!(c, ';' | '=' | '{' | '}' | ' '));
    if !needs_braces {
        return value.to_string();
    }
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('{');
    for c in value.chars() {
        if c == '}' {
            escaped.push('}');
        }
        escaped.push(c);
    }
    escaped.push('}');
    escaped
}

fn push_attr(out: &mut String, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    out.push_str(key);
    out.push('=');
    out.push_str(&escape_odbc_value(value));
    out.push(';');
}

/// Builds the connection string for one connect call.
///
/// `spec.url`, when set, is either a bare `DSN=<name>` (the data source
/// picks an OS-configured DSN, e.g. one an admin registered for a
/// mainframe-only driver) or a driver-specific connection string an
/// advanced user pasted in verbatim (kept as-is, `Uid`/`Pwd` appended only
/// if the string does not already carry them). Otherwise the string is
/// assembled from `spec`'s structured fields, `spec.driver` naming the
/// ODBC driver entry (`"SQLite3"`, `"ODBC Driver 18 for SQL Server"`, …).
pub fn build_connection_string(spec: &ConnectSpec) -> ConnectionString {
    let trimmed = spec.url.trim();
    if !trimmed.is_empty() {
        let mut out = trimmed.to_string();
        if !out.ends_with(';') {
            out.push(';');
        }
        let lower = out.to_ascii_lowercase();
        if !spec.user.is_empty() && !lower.contains("uid=") {
            push_attr(&mut out, "Uid", &spec.user);
        }
        if let Some(password) = spec.password.as_deref() {
            if !password.is_empty() && !lower.contains("pwd=") {
                push_attr(&mut out, "Pwd", password);
            }
        }
        return ConnectionString(out);
    }

    let mut out = String::new();
    push_attr(&mut out, "Driver", &spec.driver);
    push_attr(&mut out, "Server", &spec.host);
    if let Some(port) = spec.port {
        push_attr(&mut out, "Port", &port.to_string());
    }
    push_attr(&mut out, "Database", &spec.database);
    push_attr(&mut out, "Uid", &spec.user);
    if let Some(password) = spec.password.as_deref() {
        push_attr(&mut out, "Pwd", password);
    }
    ConnectionString(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::datasource::SslConfig;

    fn spec() -> ConnectSpec {
        ConnectSpec {
            driver: "SQLite3".to_string(),
            host: String::new(),
            port: None,
            database: "/tmp/db.sqlite".to_string(),
            user: String::new(),
            url: String::new(),
            password: None,
            ssl: SslConfig::default(),
        }
    }

    #[test]
    fn assembles_driver_database_when_no_url_is_given() {
        let built = build_connection_string(&spec());
        assert_eq!(built.as_str(), "Driver=SQLite3;Database=/tmp/db.sqlite;");
    }

    #[test]
    fn includes_server_port_uid_pwd_when_present() {
        let mut source = spec();
        source.driver = "ODBC Driver 18 for SQL Server".to_string();
        source.host = "db.internal".to_string();
        source.port = Some(1433);
        source.database = "shop".to_string();
        source.user = "florian".to_string();
        source.password = Some("hunter2".to_string());
        let built = build_connection_string(&source);
        assert_eq!(
            built.as_str(),
            "Driver={ODBC Driver 18 for SQL Server};Server=db.internal;Port=1433;\
             Database=shop;Uid=florian;Pwd=hunter2;"
        );
    }

    #[test]
    fn escapes_a_closing_brace_inside_a_password_by_doubling_it() {
        let mut source = spec();
        source.user = "florian".to_string();
        source.password = Some("a}b".to_string());
        let built = build_connection_string(&source);
        assert!(built.as_str().contains("Pwd={a}}b};"));
    }

    #[test]
    fn escapes_a_value_containing_a_semicolon() {
        let mut source = spec();
        source.password = Some("a;b".to_string());
        let built = build_connection_string(&source);
        assert!(built.as_str().contains("Pwd={a;b};"));
    }

    #[test]
    fn a_dsn_url_is_used_verbatim_with_uid_pwd_appended() {
        let mut source = spec();
        source.url = "DSN=MyDataSource".to_string();
        source.user = "florian".to_string();
        source.password = Some("hunter2".to_string());
        let built = build_connection_string(&source);
        assert_eq!(built.as_str(), "DSN=MyDataSource;Uid=florian;Pwd=hunter2;");
    }

    #[test]
    fn a_raw_connection_string_already_carrying_uid_is_left_alone() {
        let mut source = spec();
        source.url = "Driver=PostgreSQL Unicode;Server=x;Uid=admin;".to_string();
        source.user = "florian".to_string();
        source.password = Some("hunter2".to_string());
        let built = build_connection_string(&source);
        // The caller's own Uid is respected; only the missing Pwd is added.
        assert_eq!(
            built.as_str(),
            "Driver=PostgreSQL Unicode;Server=x;Uid=admin;Pwd=hunter2;"
        );
    }

    #[test]
    fn debug_never_prints_the_connection_string() {
        let built = build_connection_string(&spec());
        let rendered = format!("{built:?}");
        assert_eq!(rendered, "ConnectionString(<redacted>)");
    }

    #[test]
    fn an_empty_value_is_never_emitted_as_an_attribute() {
        let built = build_connection_string(&spec());
        assert!(!built.as_str().contains("Server="));
        assert!(!built.as_str().contains("Uid="));
    }
}
