//! Dump/restore argv builders (F5.3, database-tools-plan.md): one
//! [`Command`] per `pg_dump`/`pg_restore`/`psql`, `mysqldump`/`mysql`,
//! `mongodump`/`mongorestore`, and `sqlite3 .dump` invocation. A password
//! never appears as an argv word — any other user on the box can read a
//! process's argv from `/proc/<pid>/cmdline`, which is exactly the leak
//! ADR-0061 §1 rules out for a bound SQL parameter and this module rules
//! out for a subprocess credential too — it crosses instead as an env var
//! (`PGPASSWORD`) or a 0600 temp file (MySQL's `--defaults-extra-file`,
//! Mongo's `--config`).
//!
//! Running one of these is [`spawn`], a thin wrapper over
//! [`process_exec::spawn_with_env`] (added here: the existing `spawn` had
//! no way to set an env var on the child at all, which `PGPASSWORD` needs).
//! Wiring a `Command` into the Run dock's console is F5b's job — this
//! module only builds the argv and can smoke-test that spawning finds (or
//! reports missing) the tool.

use std::io::{self, Write};
use std::path::Path;

use db_core::datasource::DataSource;
use process_exec::host::{resolve_program, ExecHost};
use process_exec::{spawn_with_stdin, Failure, Spawned};

/// A resolved password, kept out of `Debug`/`Display` the same way
/// [`db_core::datasource::Secrets`] is — a panic message or a log line
/// built from this must never carry it.
#[derive(Default, Clone)]
pub struct Credentials {
    pub password: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// What a dump/restore run should cover.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DumpOptions {
    pub schema_only: bool,
    pub data_only: bool,
    /// Empty means every table.
    pub tables: Vec<String>,
    pub output_file: Option<String>,
}

/// One invocation: the program, its argv (never a secret word), the env
/// vars to set, and — for `mysql*`/`mongo*` — the 0600 temp file the argv
/// points a `--defaults-extra-file`/`--config` flag at. Some restores
/// (`mysql`) read their script from stdin rather than a `-f` flag;
/// `stdin_file` names the file the caller should stream in, if any.
pub struct Command {
    pub program: String,
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub credentials_file: Option<tempfile::NamedTempFile>,
    pub stdin_file: Option<String>,
}

impl Command {
    fn new(program: impl Into<String>, argv: Vec<String>, env: Vec<(String, String)>) -> Self {
        Self {
            program: program.into(),
            argv,
            env,
            credentials_file: None,
            stdin_file: None,
        }
    }
}

/// Write `contents` to a fresh temp file, `0600` on Unix (Windows has no
/// equivalent bit; the file still lives in the user's own temp directory,
/// which is the closest that platform gets).
fn secret_file(contents: &str) -> io::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::NamedTempFile::new()?;
    file.write_all(contents.as_bytes())?;
    file.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

// ---- PostgreSQL ----

fn pg_host_user_args(source: &DataSource) -> Vec<String> {
    let mut argv = vec!["-h".to_string(), source.host.clone()];
    if let Some(port) = source.port {
        argv.push("-p".to_string());
        argv.push(port.to_string());
    }
    argv.push("-U".to_string());
    argv.push(source.user.clone());
    argv
}

fn pg_env(credentials: &Credentials) -> Vec<(String, String)> {
    credentials
        .password
        .clone()
        .map(|password| vec![("PGPASSWORD".to_string(), password)])
        .unwrap_or_default()
}

pub fn pg_dump(source: &DataSource, credentials: &Credentials, options: &DumpOptions) -> Command {
    let mut argv = pg_host_user_args(source);
    if options.schema_only {
        argv.push("--schema-only".to_string());
    }
    if options.data_only {
        argv.push("--data-only".to_string());
    }
    for table in &options.tables {
        argv.push("-t".to_string());
        argv.push(table.clone());
    }
    if let Some(output) = &options.output_file {
        argv.push("-f".to_string());
        argv.push(output.clone());
    }
    argv.push(source.database.clone());
    Command::new("pg_dump", argv, pg_env(credentials))
}

pub fn pg_restore(
    source: &DataSource,
    credentials: &Credentials,
    options: &DumpOptions,
    input_file: &str,
) -> Command {
    let mut argv = pg_host_user_args(source);
    argv.push("-d".to_string());
    argv.push(source.database.clone());
    if options.schema_only {
        argv.push("--schema-only".to_string());
    }
    if options.data_only {
        argv.push("--data-only".to_string());
    }
    for table in &options.tables {
        argv.push("-t".to_string());
        argv.push(table.clone());
    }
    argv.push(input_file.to_string());
    Command::new("pg_restore", argv, pg_env(credentials))
}

pub fn psql_file(source: &DataSource, credentials: &Credentials, input_file: &str) -> Command {
    let mut argv = pg_host_user_args(source);
    argv.push("-d".to_string());
    argv.push(source.database.clone());
    argv.push("-f".to_string());
    argv.push(input_file.to_string());
    Command::new("psql", argv, pg_env(credentials))
}

// ---- MySQL / MariaDB ----

fn mysql_host_user_args(source: &DataSource) -> Vec<String> {
    let mut argv = vec!["-h".to_string(), source.host.clone()];
    if let Some(port) = source.port {
        argv.push("-P".to_string());
        argv.push(port.to_string());
    }
    argv.push("-u".to_string());
    argv.push(source.user.clone());
    argv
}

/// `--defaults-extra-file` must be the client's *first* argument — this
/// is what carries the password, never `-p<password>`/`MYSQL_PWD` (the
/// latter is readable from the environment of any process the same user
/// can inspect, e.g. another process's `/proc/<pid>/environ`).
fn mysql_credentials_prefix(
    credentials: &Credentials,
) -> io::Result<(Vec<String>, Option<tempfile::NamedTempFile>)> {
    match &credentials.password {
        None => Ok((Vec::new(), None)),
        Some(password) => {
            let file = secret_file(&format!("[client]\npassword={password}\n"))?;
            let flag = format!("--defaults-extra-file={}", file.path().display());
            Ok((vec![flag], Some(file)))
        }
    }
}

pub fn mysqldump(
    source: &DataSource,
    credentials: &Credentials,
    options: &DumpOptions,
) -> io::Result<Command> {
    let (mut argv, credentials_file) = mysql_credentials_prefix(credentials)?;
    argv.extend(mysql_host_user_args(source));
    if options.schema_only {
        argv.push("--no-data".to_string());
    }
    if options.data_only {
        argv.push("--no-create-info".to_string());
    }
    argv.push(source.database.clone());
    argv.extend(options.tables.iter().cloned());
    let mut command = Command::new("mysqldump", argv, Vec::new());
    command.credentials_file = credentials_file;
    Ok(command)
}

/// The `mysql` client reads its restore script from stdin, not a `-f`
/// flag — [`Command::stdin_file`] names `input_file` for the caller to
/// stream in.
pub fn mysql_restore(
    source: &DataSource,
    credentials: &Credentials,
    input_file: &str,
) -> io::Result<Command> {
    let (mut argv, credentials_file) = mysql_credentials_prefix(credentials)?;
    argv.extend(mysql_host_user_args(source));
    argv.push(source.database.clone());
    let mut command = Command::new("mysql", argv, Vec::new());
    command.credentials_file = credentials_file;
    command.stdin_file = Some(input_file.to_string());
    Ok(command)
}

// ---- MongoDB ----

fn mongo_host(source: &DataSource) -> String {
    match source.port {
        Some(port) => format!("{}:{}", source.host, port),
        None => source.host.clone(),
    }
}

/// With no password, a plain `--uri` (no credentials) is safe on argv.
/// With one, the URI (which would otherwise embed `user:password@host`)
/// moves into a `--config` YAML file instead, the same "never on argv"
/// rule the MySQL path applies via `--defaults-extra-file`.
fn mongo_credentials(
    source: &DataSource,
    credentials: &Credentials,
) -> io::Result<(Vec<String>, Option<tempfile::NamedTempFile>)> {
    match &credentials.password {
        None => Ok((
            vec![format!(
                "--uri=mongodb://{}/{}",
                mongo_host(source),
                source.database
            )],
            None,
        )),
        Some(password) => {
            let uri = format!(
                "mongodb://{}:{}@{}/{}",
                source.user,
                password,
                mongo_host(source),
                source.database
            );
            let file = secret_file(&format!("uri: \"{uri}\"\n"))?;
            let flag = format!("--config={}", file.path().display());
            Ok((vec![flag], Some(file)))
        }
    }
}

pub fn mongodump(
    source: &DataSource,
    credentials: &Credentials,
    options: &DumpOptions,
) -> io::Result<Command> {
    let (mut argv, credentials_file) = mongo_credentials(source, credentials)?;
    for table in &options.tables {
        argv.push("--collection".to_string());
        argv.push(table.clone());
    }
    if let Some(output) = &options.output_file {
        argv.push("--archive".to_string());
        argv.push(output.clone());
    }
    let mut command = Command::new("mongodump", argv, Vec::new());
    command.credentials_file = credentials_file;
    Ok(command)
}

pub fn mongorestore(
    source: &DataSource,
    credentials: &Credentials,
    input_file: &str,
) -> io::Result<Command> {
    let (mut argv, credentials_file) = mongo_credentials(source, credentials)?;
    argv.push("--archive".to_string());
    argv.push(input_file.to_string());
    let mut command = Command::new("mongorestore", argv, Vec::new());
    command.credentials_file = credentials_file;
    Ok(command)
}

// ---- SQLite ----

/// SQLite has no server credentials at all — the whole `Command` is just
/// the file path plus a `.dump`/`.schema` meta-command.
pub fn sqlite_dump(source: &DataSource, options: &DumpOptions) -> Command {
    let path = if source.url.is_empty() {
        source.database.clone()
    } else {
        source.url.clone()
    };
    let mut meta = if options.schema_only {
        ".schema"
    } else {
        ".dump"
    }
    .to_string();
    for table in &options.tables {
        meta.push(' ');
        meta.push_str(table);
    }
    Command::new("sqlite3", vec![path, meta], Vec::new())
}

/// SQLite has no `pg_restore`/`mysql`-shaped restore client of its own —
/// `sqlite3 DBFILE` reads a script from stdin the same way `mysql`'s own
/// restore does (its CLI has no "run this whole file as a script" flag,
/// only `.read` as an interactive meta-command), so this reuses
/// `stdin_file` exactly like [`mysql_restore`].
pub fn sqlite_restore(source: &DataSource, input_file: &str) -> Command {
    let path = if source.url.is_empty() {
        source.database.clone()
    } else {
        source.url.clone()
    };
    let mut command = Command::new("sqlite3", vec![path], Vec::new());
    command.stdin_file = Some(input_file.to_string());
    command
}

/// The restore command for `driver` (the same id `DataSource::driver`
/// carries), dispatching to the tool-specific builder above: `psql -f`
/// for PostgreSQL (this crate's own `pg_dump` never passes
/// `--format=custom`, so its output is always plain SQL `psql` reads
/// directly — `pg_restore` stays exported for a dump made some other
/// way, but is not this dispatch's default), `mysql` (stdin) for MySQL/
/// MariaDB, `mongorestore` (archive file arg) for MongoDB, and `sqlite3`
/// (stdin) otherwise.
pub fn restore_command(
    driver: &str,
    source: &DataSource,
    credentials: &Credentials,
    input_file: &str,
) -> io::Result<Command> {
    match driver {
        "postgres" | "postgresql" => Ok(psql_file(source, credentials, input_file)),
        "mysql" | "mariadb" => mysql_restore(source, credentials, input_file),
        "mongo" | "mongodb" => mongorestore(source, credentials, input_file),
        _ => Ok(sqlite_restore(source, input_file)),
    }
}

// ---- preview / spawn / tool presence ----

/// Shell-quote one argv word for display only — see
/// `container_core::run_config::quote`'s identical reasoning, reimplemented
/// here rather than depending on `container-core` (out of layering for a
/// support crate that has nothing to do with containers).
fn quote(word: &str) -> String {
    let needs_quoting = word.is_empty()
        || !word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=,@%+".contains(c));
    if !needs_quoting {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// One line: `program argv...`, shell-quoted — never includes `env` (a
/// password there is exactly what this module keeps off any surface a
/// screenshot or a copy-paste could leak).
pub fn preview(command: &Command) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.argv.iter().map(String::as_str))
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Spawn `command` in `work_dir` for the caller's own console — the
/// Run-dock hookup (reading `Spawned`'s pipes) is `ExchangeService`'s job;
/// this only starts the process with argv/env set exactly as built above,
/// piping `stdin_file`'s contents in when the command names one (F6c:
/// `mysql_restore`'s own doc comment — `mysql` reads its script from
/// stdin, not a `-f` flag, and this is the one place that stdin actually
/// gets connected to the file it names).
pub fn spawn(command: &Command, work_dir: &Path) -> Result<Spawned, Failure> {
    let args: Vec<&str> = command.argv.iter().map(String::as_str).collect();
    let env: Vec<(&str, &str)> = command
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let stdin_file = command.stdin_file.as_ref().map(Path::new);
    spawn_with_stdin(&command.program, &args, work_dir, &env, stdin_file)
}

/// Whether `program` resolves on `PATH` (or, for a WSL project root, in
/// the distro) — the dump dialog's "not installed" affordance.
///
/// `process_exec::host::resolve_program` is not enough on its own: by its
/// own doc comment, a *local* host "never probes" and always answers
/// `Some` — it exists to resolve a login shell's `PATH` inside WSL, not to
/// answer "is this actually installed". So a remote (WSL) project root
/// still asks it, but a local one checks `PATH` here directly.
pub fn tool_available(program: &str, work_dir: &Path) -> bool {
    let host = ExecHost::for_path(work_dir);
    if host.is_remote() {
        return resolve_program(&host, program, work_dir).is_some();
    }
    on_local_path(program)
}

fn on_local_path(program: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| {
        if dir.join(program).is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            dir.join(format!("{program}.exe")).is_file()
        }
        #[cfg(not(windows))]
        {
            false
        }
    })
}

/// A short install hint for a missing dump/restore tool.
pub fn install_hint(program: &str) -> &'static str {
    match program {
        "pg_dump" | "pg_restore" | "psql" => {
            "Install the PostgreSQL client tools (e.g. the postgresql-client package)."
        }
        "mysqldump" | "mysql" => {
            "Install the MySQL/MariaDB client tools (e.g. the mysql-client package)."
        }
        "mongodump" | "mongorestore" => {
            "Install the MongoDB Database Tools (mongodb-database-tools)."
        }
        "sqlite3" => "Install the sqlite3 command-line tool.",
        _ => "Install the matching database client tools.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> DataSource {
        DataSource {
            id: "s1".to_string(),
            name: "shop".to_string(),
            driver: "postgresql".to_string(),
            host: "db.internal".to_string(),
            port: Some(5432),
            database: "shop".to_string(),
            user: "florian".to_string(),
            url: String::new(),
            read_only: false,
            ssl: Default::default(),
            ssh: None,
        }
    }

    #[test]
    fn pg_dump_never_puts_the_password_on_argv() {
        let credentials = Credentials {
            password: Some("SECRET_MARKER".to_string()),
        };
        let command = pg_dump(&source(), &credentials, &DumpOptions::default());
        assert!(!command
            .argv
            .iter()
            .any(|word| word.contains("SECRET_MARKER")));
        assert_eq!(
            command.env,
            vec![("PGPASSWORD".to_string(), "SECRET_MARKER".to_string())]
        );
        assert!(!preview(&command).contains("SECRET_MARKER"));
    }

    #[test]
    fn pg_dump_argv_carries_host_user_and_database() {
        let command = pg_dump(&source(), &Credentials::default(), &DumpOptions::default());
        assert_eq!(command.program, "pg_dump");
        assert_eq!(
            command.argv,
            vec!["-h", "db.internal", "-p", "5432", "-U", "florian", "shop"]
        );
    }

    #[test]
    fn schema_only_and_table_subset_are_reflected_in_argv() {
        let options = DumpOptions {
            schema_only: true,
            tables: vec!["users".to_string(), "orders".to_string()],
            ..Default::default()
        };
        let command = pg_dump(&source(), &Credentials::default(), &options);
        assert!(command.argv.contains(&"--schema-only".to_string()));
        assert_eq!(command.argv.iter().filter(|w| *w == "-t").count(), 2);
        assert!(command.argv.contains(&"users".to_string()));
    }

    #[test]
    fn mysqldump_never_puts_the_password_on_argv_and_uses_a_0600_defaults_file() {
        let credentials = Credentials {
            password: Some("SECRET_MARKER".to_string()),
        };
        let command = mysqldump(&source(), &credentials, &DumpOptions::default()).unwrap();
        assert!(!command
            .argv
            .iter()
            .any(|word| word.contains("SECRET_MARKER")));
        assert!(command.env.is_empty());
        let file = command
            .credentials_file
            .as_ref()
            .expect("a defaults file was created");
        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert!(contents.contains("SECRET_MARKER"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = file.as_file().metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert!(!preview(&command).contains("SECRET_MARKER"));
    }

    #[test]
    fn mysqldump_with_no_password_creates_no_credentials_file() {
        let command =
            mysqldump(&source(), &Credentials::default(), &DumpOptions::default()).unwrap();
        assert!(command.credentials_file.is_none());
        assert!(!command
            .argv
            .iter()
            .any(|w| w.starts_with("--defaults-extra-file")));
    }

    #[test]
    fn mysql_restore_reads_its_script_from_stdin_not_a_flag() {
        let command = mysql_restore(&source(), &Credentials::default(), "/tmp/dump.sql").unwrap();
        assert_eq!(command.stdin_file.as_deref(), Some("/tmp/dump.sql"));
        assert!(!command.argv.iter().any(|w| w == "/tmp/dump.sql"));
    }

    #[test]
    fn mongodump_with_a_password_moves_the_uri_into_a_config_file() {
        let credentials = Credentials {
            password: Some("SECRET_MARKER".to_string()),
        };
        let command = mongodump(&source(), &credentials, &DumpOptions::default()).unwrap();
        assert!(!command
            .argv
            .iter()
            .any(|word| word.contains("SECRET_MARKER")));
        let file = command.credentials_file.expect("a config file was created");
        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert!(contents.contains("SECRET_MARKER"));
    }

    #[test]
    fn mongodump_with_no_password_puts_a_credential_free_uri_on_argv() {
        let command =
            mongodump(&source(), &Credentials::default(), &DumpOptions::default()).unwrap();
        assert!(command.credentials_file.is_none());
        assert!(command
            .argv
            .iter()
            .any(|w| w == "--uri=mongodb://db.internal:5432/shop"));
    }

    #[test]
    fn sqlite_restore_reads_its_script_from_stdin() {
        let mut source = source();
        source.database = "/data/app.db".to_string();
        let command = sqlite_restore(&source, "/tmp/dump.sql");
        assert_eq!(command.program, "sqlite3");
        assert_eq!(command.argv, vec!["/data/app.db"]);
        assert_eq!(command.stdin_file.as_deref(), Some("/tmp/dump.sql"));
    }

    #[test]
    fn restore_command_dispatches_by_driver() {
        let credentials = Credentials::default();
        let pg = restore_command("postgresql", &source(), &credentials, "/tmp/d.sql").unwrap();
        assert_eq!(pg.program, "psql");
        assert!(pg.stdin_file.is_none());

        let my = restore_command("mysql", &source(), &credentials, "/tmp/d.sql").unwrap();
        assert_eq!(my.program, "mysql");
        assert_eq!(my.stdin_file.as_deref(), Some("/tmp/d.sql"));

        let mongo = restore_command("mongodb", &source(), &credentials, "/tmp/d.archive").unwrap();
        assert_eq!(mongo.program, "mongorestore");

        let sqlite = restore_command("sqlite", &source(), &credentials, "/tmp/d.sql").unwrap();
        assert_eq!(sqlite.program, "sqlite3");
        assert_eq!(sqlite.stdin_file.as_deref(), Some("/tmp/d.sql"));
    }

    #[test]
    fn sqlite_dump_has_no_credentials_at_all() {
        let mut source = source();
        source.database = "/data/app.db".to_string();
        let command = sqlite_dump(&source, &DumpOptions::default());
        assert_eq!(command.program, "sqlite3");
        assert_eq!(command.argv, vec!["/data/app.db", ".dump"]);
        assert!(command.env.is_empty());
    }

    #[test]
    fn sqlite_schema_only_uses_the_dot_schema_meta_command() {
        let options = DumpOptions {
            schema_only: true,
            ..Default::default()
        };
        let command = sqlite_dump(&source(), &options);
        assert_eq!(command.argv[1], ".schema");
    }

    #[test]
    fn preview_shell_quotes_a_word_that_needs_it() {
        let command = Command::new(
            "sqlite3",
            vec!["a b".to_string(), ".dump".to_string()],
            Vec::new(),
        );
        assert_eq!(preview(&command), "sqlite3 'a b' .dump");
    }

    /// `spawn` must actually connect `stdin_file` to the child's stdin —
    /// `mysql_restore_reads_its_script_from_stdin_not_a_flag` above only
    /// proves the `Command` *names* the file; this proves `spawn` reads
    /// it. `cat` stands in for `mysql`, same reasoning as
    /// `process_exec`'s own stdin-piping test.
    #[test]
    fn spawn_pipes_a_commands_stdin_file_into_the_child() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let input_path = dir.path().join("dump.sql");
        std::fs::write(&input_path, b"INSERT INTO t VALUES (1);\n").unwrap();
        let mut command = Command::new("cat", Vec::new(), Vec::new());
        command.stdin_file = Some(input_path.to_string_lossy().into_owned());

        let spawned = spawn(&command, dir.path()).unwrap();
        let mut stdout = spawned.take_stdout().unwrap();
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).unwrap();
        assert_eq!(buffer, b"INSERT INTO t VALUES (1);\n");
        assert!(spawned.wait().unwrap().success());
    }

    #[test]
    fn a_missing_tool_is_reported_unavailable_with_an_install_hint() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!tool_available(
            "this-database-tool-does-not-exist-anywhere",
            dir.path()
        ));
        assert!(install_hint("pg_dump").contains("PostgreSQL"));
    }
}
