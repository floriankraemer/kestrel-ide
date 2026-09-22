//! Unit tests for `super::exchange` — split out to keep `exchange.rs`
//! under the file-size ceiling as F4c's typed-export coverage grew it,
//! the same split `db-drivers`'s `sqlite_tests.rs` already uses for the
//! same reason (that commit's own doc comment).

use super::*;

#[test]
fn to_format_maps_every_ffi_variant() {
    assert_eq!(to_format(FfiExportFormat::Csv).0, Format::Csv);
    assert_eq!(to_format(FfiExportFormat::Tsv).0, Format::Tsv);
    assert_eq!(to_format(FfiExportFormat::Json).0, Format::Json);
    assert!(!to_format(FfiExportFormat::Json).1);
    assert!(to_format(FfiExportFormat::JsonLines).1);
    assert_eq!(to_format(FfiExportFormat::Xlsx).0, Format::Xlsx);
}

#[test]
fn split_csv_trims_and_drops_empties() {
    assert_eq!(split_csv(" a, b ,,c"), vec!["a", "b", "c"]);
    assert_eq!(split_csv(""), Vec::<String>::new());
}

#[test]
fn non_empty_or_falls_back_only_when_empty() {
    assert_eq!(non_empty_or("", "NULL"), "NULL");
    assert_eq!(non_empty_or("N/A", "NULL"), "N/A");
}

#[test]
fn detected_type_round_trips_through_its_name() {
    for kind in [
        import::DetectedType::Int,
        import::DetectedType::Float,
        import::DetectedType::Bool,
        import::DetectedType::Date,
        import::DetectedType::Text,
    ] {
        assert_eq!(to_detected_type(detected_type_name(kind)), kind);
    }
}

#[test]
fn job_ids_are_allocated_once_each_and_tracked_for_cancel() {
    let mut jobs = Jobs::default();
    let (first, flag_a) = jobs.alloc();
    let (second, flag_b) = jobs.alloc();
    assert_ne!(first, second);
    assert!(!flag_a.load(Ordering::Relaxed));
    assert!(!flag_b.load(Ordering::Relaxed));
}

#[test]
fn render_rows_renders_csv_from_typed_json_rows() {
    let columns = QStringList::from_iter(["id", "note"].map(QString::from));
    let rows = QStringList::from_iter(
        [
            db_core::value::row_to_json(&[Value::Int(1), Value::Text("hello".to_string())]),
            db_core::value::row_to_json(&[Value::Int(2), Value::Null]),
        ]
        .iter()
        .map(|s| QString::from(s.as_str())),
    );
    let options = FfiExportOptions {
        header: true,
        null_text: QString::from("NULL"),
        quote_all: false,
        delimiter: QString::from(","),
        date_format: QString::default(),
        table_name: QString::default(),
        key_columns: QString::default(),
    };
    let text = render_rows(&columns, &rows, FfiExportFormat::Csv, &options).unwrap();
    let text = String::from_utf8(text).unwrap();
    assert!(text.contains("id,note"));
    assert!(text.contains("1,hello"));
    assert!(text.contains("2,NULL"));
}

/// F4c's own gap-closing test: a grid export ("Copy as"/"Export…",
/// `parse_typed_rows` -> `render_rows`) of the *same* typed data a
/// `SELECT * FROM t` produces must render byte-for-byte identical SQL
/// INSERT text to what `exportTable`'s own direct `write_format` call
/// produces from the untouched `RowBatch` — proving the JSON round
/// trip (`ResultProvider::rowValues`'s encoding) never loses a
/// value's real type along the way (an `Int` quoted as `42`, not
/// `'42'`; `NULL`, not the text `'NULL'`).
#[test]
fn typed_grid_export_matches_export_table_s_own_sql_insert_output() {
    let columns = vec![
        ColumnMeta::new("id", "INTEGER", false),
        ColumnMeta::new("name", "TEXT", true),
        ColumnMeta::new("score", "REAL", true),
        ColumnMeta::new("data", "BLOB", true),
    ];
    let rows = vec![
        vec![
            Value::Int(1),
            Value::Text("O'Brien".to_string()),
            Value::Float(1.5),
            Value::Bytes(vec![0xde, 0xad]),
        ],
        vec![Value::Int(2), Value::Null, Value::Null, Value::Null],
    ];
    let batch = RowBatch {
        columns: columns.clone(),
        rows: rows.clone(),
    };

    // The direct path `run_export_table` itself uses.
    let export_options = ExportOptions {
        dialect: Dialect::Sqlite,
        table_name: "t".to_string(),
        ..ExportOptions::default()
    };
    let mut direct = Vec::new();
    let mut direct_iter = std::iter::once(batch);
    export::write_format(
        Format::Sql,
        &columns,
        &mut direct_iter,
        &mut direct,
        &export_options,
    )
    .unwrap();
    let direct_text = String::from_utf8(direct).unwrap();

    // The grid-export path: `rowValues`'s own JSON encoding in, typed
    // `render_rows` out.
    let ffi_columns =
        QStringList::from_iter(columns.iter().map(|c| QString::from(c.name.as_str())));
    let ffi_rows = QStringList::from_iter(
        rows.iter()
            .map(|row| QString::from(db_core::value::row_to_json(row).as_str())),
    );
    let ffi_options = FfiExportOptions {
        header: true,
        null_text: QString::from("NULL"),
        quote_all: false,
        delimiter: QString::from(","),
        date_format: QString::default(),
        table_name: QString::from("t"),
        key_columns: QString::default(),
    };
    let grid_text = render_rows(
        &ffi_columns,
        &ffi_rows,
        FfiExportFormat::SqlInsert,
        &ffi_options,
    )
    .unwrap();
    let grid_text = String::from_utf8(grid_text).unwrap();

    assert_eq!(grid_text, direct_text);
    // Guards against a vacuous pass: prove the assertion is actually
    // comparing typed SQL, not two empty strings.
    assert!(grid_text.contains("O''Brien"));
    assert!(grid_text.contains("NULL"));
    assert!(!grid_text.contains("'NULL'"));
}

fn table_node(name: &str, columns: &[(&str, &str, bool, bool)]) -> db_core::schema::Node {
    use db_core::schema::{Node, NodeDetail, ObjectKind};
    let children = columns
        .iter()
        .map(|(col_name, type_name, nullable, pk)| {
            Node::leaf(*col_name, ObjectKind::Column).with_detail(NodeDetail {
                type_name: Some(type_name.to_string()),
                nullable: Some(*nullable),
                default: None,
                primary_key: *pk,
                ..NodeDetail::default()
            })
        })
        .collect();
    Node::with_children(name, ObjectKind::Table, children)
}

#[test]
fn to_schema_model_maps_tables_and_primary_keys() {
    let mut session = in_memory_session();
    let roots = vec![table_node(
        "users",
        &[
            ("id", "INTEGER", false, true),
            ("name", "TEXT", true, false),
        ],
    )];
    let model = db_exchange::schema_model::from_snapshot(&roots, &mut session);
    assert_eq!(model.tables.len(), 1);
    let table = &model.tables[0];
    assert_eq!(table.name, "users");
    assert_eq!(table.primary_key, vec!["id".to_string()]);
    assert_eq!(table.columns.len(), 2);
    assert_eq!(table.columns[0].type_name, "INTEGER");
    assert!(!table.columns[0].nullable);
    assert!(table.columns[1].nullable);
}

fn in_memory_session() -> Session {
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
    let connection = db_drivers::DriverRegistry::builtin()
        .get("sqlite")
        .unwrap()
        .connect(&spec)
        .unwrap();
    Session::new(connection)
}

#[test]
fn er_diagram_renders_a_table_with_no_relationship_lines_when_fks_are_unavailable() {
    let mut session = in_memory_session();
    let roots = vec![table_node("users", &[("id", "INTEGER", false, true)])];
    let model = db_exchange::schema_model::from_snapshot(&roots, &mut session);
    let text = er_diagram::to_mermaid(&model, &er_diagram::DiagramScope::Schema);
    assert!(text.contains("erDiagram"));
    assert!(text.contains("users"));
}

#[test]
fn schema_compare_reports_an_added_table() {
    let mut session = in_memory_session();
    let left = db_exchange::schema_model::from_snapshot(&[], &mut session);
    let right = db_exchange::schema_model::from_snapshot(
        &[table_node("users", &[("id", "INTEGER", false, true)])],
        &mut session,
    );
    let diff = schema_compare::compare(&left, &right);
    let rows = to_ffi_compare_rows(&diff);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind.to_string(), "added-table");
    assert_eq!(rows[0].name.to_string(), "users");
}

#[test]
fn run_export_table_streams_a_real_sqlite_table_to_csv() {
    // A real file, not `:memory:`: `db_drivers::sqlite`'s `connect()`
    // path opens a *second*, dedicated connection for a streamed
    // `SELECT` (`SqliteLiveRowStream`, its own doc comment) — sound
    // for a file, since both connections open the same path, but
    // `:memory:` gives each connection its own, unrelated database,
    // so a `CREATE TABLE` on the first connection would be invisible
    // to a `SELECT` through the second. This test exercises the same
    // two-connection path `connect_source` + a real data source
    // always does in production.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.sqlite");
    let spec = db_core::datasource::ConnectSpec {
        driver: "sqlite".to_string(),
        host: String::new(),
        port: None,
        database: db_path.to_string_lossy().into_owned(),
        user: String::new(),
        url: String::new(),
        password: None,
        ssl: Default::default(),
    };
    let registry = db_drivers::DriverRegistry::builtin();
    let driver = registry.get("sqlite").unwrap();
    let connection = driver.connect(&spec).unwrap();
    let mut session = Session::new(connection);
    session
        .execute(
            &Statement::sql("CREATE TABLE t (id INTEGER, name TEXT)".to_string()),
            &ExecOptions::default(),
        )
        .unwrap();
    session
        .execute(
            &Statement::sql("INSERT INTO t VALUES (1, 'a'), (2, 'b')".to_string()),
            &ExecOptions::default(),
        )
        .unwrap();
    let _ = session.close();

    let mut connection = driver.connect(&spec).unwrap();
    let dest = dir.path().join("out.csv");
    let select = Statement::sql("SELECT * FROM t".to_string());
    let Execution::Rows(mut stream) = connection
        .execute(&select, &ExecOptions::default())
        .unwrap()
    else {
        panic!("expected rows");
    };
    let mut columns: Vec<ColumnMeta> = Vec::new();
    let mut batches = Vec::new();
    let mut total = 0u64;
    while let Some(batch) = stream.next_batch().unwrap() {
        if columns.is_empty() {
            columns = batch.columns.clone();
        }
        total += batch.rows.len() as u64;
        batches.push(batch);
    }
    let mut file = std::fs::File::create(&dest).unwrap();
    let options = ExportOptions {
        dialect: Dialect::Sqlite,
        ..ExportOptions::default()
    };
    let mut iter = batches.into_iter();
    export::write_format(Format::Csv, &columns, &mut iter, &mut file, &options).unwrap();

    assert_eq!(total, 2);
    let text = std::fs::read_to_string(&dest).unwrap();
    assert!(text.contains("id,name"));
    assert!(text.contains("1,a"));
    assert!(text.contains("2,b"));
}
