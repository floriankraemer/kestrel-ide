//! MongoDB console support (F7.2): a pure parser turning one console
//! statement — either a raw `runCommand` JSON document, or `db.<collection>.
//! <method>(<json>[, <json>])` sugar — into a [`MongoCommand`] the
//! `db_drivers::mongodb` connection then executes. No JS engine: the sugar
//! form is recognised structurally (collection/method/argument JSON
//! documents), never evaluated as a language.
//!
//! Kept JSON-value-typed (`serde_json::Value`) rather than `bson::
//! Document` so this parser stays in the Qt-free, tokio-free `db-sql`
//! crate alongside `resp`/`split`/`classify` — `db-drivers::mongodb` is
//! the one place a parsed [`MongoCommand`] is turned into a real BSON
//! command against a live connection.

use serde_json::Value as Json;

/// The methods F7.2's sugar recognises — every other method name, and
/// every malformed call, is a parse error rather than a best-effort guess.
pub const SUGAR_METHODS: &[&str] = &[
    "find",
    "findOne",
    "aggregate",
    "insertOne",
    "insertMany",
    "updateOne",
    "updateMany",
    "deleteOne",
    "deleteMany",
    "countDocuments",
    "distinct",
];

/// One parsed console statement.
#[derive(Debug, Clone, PartialEq)]
pub enum MongoCommand {
    /// The statement text was a JSON object, run verbatim as a
    /// `runCommand` document.
    RunCommand(Json),
    /// `db.<collection>.<method>(<args>)` sugar, structurally parsed —
    /// `args` is `method`'s comma-separated JSON arguments, in order,
    /// never more or fewer than what a hand call of that method in the
    /// real `mongosh` shell would take.
    Sugar {
        collection: String,
        method: String,
        args: Vec<Json>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MongoParseError(pub String);

impl std::fmt::Display for MongoParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MongoParseError {}

fn err(message: impl Into<String>) -> MongoParseError {
    MongoParseError(message.into())
}

/// Parses one console statement into a [`MongoCommand`].
pub fn parse(text: &str) -> Result<MongoCommand, MongoParseError> {
    let trimmed = text.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err(err("empty statement"));
    }
    if trimmed.starts_with('{') {
        let value: Json = serde_json::from_str(trimmed)
            .map_err(|error| err(format!("not a valid runCommand document: {error}")))?;
        if !value.is_object() {
            return Err(err("a runCommand statement must be a JSON object"));
        }
        return Ok(MongoCommand::RunCommand(value));
    }
    parse_sugar(trimmed)
}

fn parse_sugar(text: &str) -> Result<MongoCommand, MongoParseError> {
    let rest = text.strip_prefix("db.").ok_or_else(|| {
        err(
            "expected a runCommand JSON document (starting with `{`) or `db.<collection>.<method>(…)` sugar",
        )
    })?;

    let dot = rest.find('.').ok_or_else(|| {
        err("expected `db.<collection>.<method>(…)` — missing the `.<method>` part")
    })?;
    let collection = &rest[..dot];
    if collection.is_empty() || !is_identifier(collection) {
        return Err(err(format!("not a valid collection name: {collection:?}")));
    }

    let after_collection = &rest[dot + 1..];
    let paren = after_collection
        .find('(')
        .ok_or_else(|| err("expected `(…)` after the method name"))?;
    let method = &after_collection[..paren];
    if !is_identifier(method) {
        return Err(err(format!("not a valid method name: {method:?}")));
    }
    if !SUGAR_METHODS.contains(&method) {
        return Err(err(format!(
            "unsupported method `{method}` — supported: {}",
            SUGAR_METHODS.join(", ")
        )));
    }

    let call_tail = &after_collection[paren..];
    if !call_tail.ends_with(')') {
        return Err(err("unterminated `(…)` call"));
    }
    let inside = &call_tail[1..call_tail.len() - 1];

    let args = split_top_level_args(inside)?
        .into_iter()
        .map(|piece| {
            let piece = piece.trim();
            serde_json::from_str::<Json>(piece)
                .map_err(|error| err(format!("argument `{piece}` is not valid JSON: {error}")))
        })
        .collect::<Result<Vec<Json>, MongoParseError>>()?;

    Ok(MongoCommand::Sugar {
        collection: collection.to_string(),
        method: method.to_string(),
        args,
    })
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !text.chars().next().unwrap().is_ascii_digit()
}

/// Splits `inside` (the text between a call's outer parentheses) into its
/// top-level comma-separated arguments — bracket- and string-aware, so a
/// comma inside a nested JSON object/array or a quoted string is never
/// mistaken for an argument separator.
fn split_top_level_args(inside: &str) -> Result<Vec<String>, MongoParseError> {
    let trimmed = inside.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let mut args = Vec::new();
    let mut depth = 0i32;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut current = String::new();

    for c in trimmed.chars() {
        if let Some(quote) = in_string {
            current.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                in_string = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                in_string = Some(c);
                current.push(c);
            }
            '{' | '[' | '(' => {
                depth += 1;
                current.push(c);
            }
            '}' | ']' | ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err(err("unbalanced brackets in call arguments"));
                }
                current.push(c);
            }
            ',' if depth == 0 => {
                args.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    if in_string.is_some() {
        return Err(err("unterminated string in call arguments"));
    }
    if depth != 0 {
        return Err(err("unbalanced brackets in call arguments"));
    }
    args.push(current);
    Ok(args)
}

/// Classifies a sugar method (or a `runCommand` document's top-level
/// command name) as read-only or not — `db_drivers::mongodb` uses this to
/// refuse a write against a read-only data source client-side, the same
/// "fail closed on the unknown" rule `crate::resp::is_read_only_command`
/// applies to Redis.
pub fn is_read_only_method(method: &str) -> bool {
    matches!(
        method,
        "find" | "findOne" | "aggregate" | "countDocuments" | "distinct"
    )
}

/// The command name a `runCommand` document names — its first key, the
/// convention every Mongo wire command follows (`{find: "coll", ...}`,
/// `{insert: "coll", ...}`, …).
pub fn run_command_name(document: &Json) -> Option<&str> {
    document.as_object()?.keys().next().map(String::as_str)
}

/// Splits a Mongo console's whole buffer into the byte range of each
/// top-level statement — one `db.<collection>.<method>(…)` call, or one
/// JSON `runCommand` document (F7b's multi-statement Mongo console,
/// database-tools.md §4). Bracket- and string-aware like
/// [`split_top_level_args`]: a statement ends once its own brackets close
/// back to depth zero, at an explicit `;`, or at the next non-whitespace
/// character once one already has — so `db.a.find({})\ndb.b.find({})` and
/// `db.a.find({}); db.b.find({});` both split into two statements, and a
/// `;`/newline inside a nested document never does.
pub fn command_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut depth = 0i32;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut start: Option<usize> = None;
    // Byte offset right after a bracket that just closed depth back to
    // zero — the earliest point a following statement could start; `None`
    // once anything other than trailing whitespace is seen past it (so a
    // `.sort()` chained after `db.a.find({})` does not fork a boundary).
    let mut boundary: Option<usize> = None;

    for (i, c) in text.char_indices() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                in_string = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                in_string = Some(c);
                if start.is_none() {
                    start = Some(i);
                }
                boundary = None;
            }
            '{' | '[' | '(' => {
                depth += 1;
                if start.is_none() {
                    start = Some(i);
                }
                boundary = None;
            }
            '}' | ']' | ')' => {
                depth -= 1;
                if depth <= 0 {
                    depth = depth.max(0);
                    boundary = Some(i + c.len_utf8());
                }
            }
            ';' if depth == 0 => {
                if let Some(s) = start.take() {
                    spans.push((s, i));
                }
                boundary = None;
            }
            c if depth == 0 && !c.is_whitespace() => {
                if let (Some(b), Some(s)) = (boundary, start) {
                    spans.push((s, b));
                    start = Some(i);
                } else if start.is_none() {
                    start = Some(i);
                }
                boundary = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        spans.push((s, text.len()));
    }
    spans
        .into_iter()
        .filter(|(s, e)| !text[*s..*e].trim().is_empty())
        .collect()
}

/// [`command_spans`], rendered as the trimmed statement texts themselves —
/// what a console's "run selection"/script runner actually sends one at a
/// time to `parse`.
pub fn split_commands(text: &str) -> Vec<&str> {
    command_spans(text)
        .into_iter()
        .map(|(s, e)| text[s..e].trim())
        .collect()
}

/// Read-only `runCommand` names (F7b) — the raw-document counterpart to
/// [`is_read_only_method`]'s sugar-method table, same fail-closed rule: a
/// command this table has never heard of is a write as far as the guard
/// is concerned.
const READ_ONLY_RUN_COMMANDS: &[&str] = &[
    "find",
    "aggregate",
    "count",
    "distinct",
    "listCollections",
    "listDatabases",
    "listIndexes",
    "dbStats",
    "collStats",
    "ping",
    "explain",
    "hello",
    "isMaster",
    "buildInfo",
    "serverStatus",
    "whatsmyuri",
];

pub fn is_read_only_run_command(name: &str) -> bool {
    READ_ONLY_RUN_COMMANDS.contains(&name)
}

/// Splits console text into its top-level statements: one per `db.<coll>.
/// <method>(…)` sugar call or top-level JSON document, bracket- and
/// string-aware so a `}`/`)` nested inside a filter or a quoted string
/// never ends a statement early (the same depth-tracking
/// [`split_top_level_args`] uses, applied to the whole buffer rather than
/// to one call's argument list). A stray `;` or blank line between
/// statements is skipped, matching `mongosh`'s own tolerance for a
/// trailing semicolon (`parse` already trims one per statement).
pub fn split_statements(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    let mut start: Option<usize> = None;

    for (i, c) in text.char_indices() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                in_string = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => in_string = Some(c),
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => {
                depth = (depth - 1).max(0);
                if depth == 0 {
                    if let Some(s) = start {
                        let end = i + c.len_utf8();
                        let piece = text[s..end].trim();
                        if !piece.is_empty() {
                            out.push(piece);
                        }
                        start = None;
                    }
                    continue;
                }
            }
            _ if depth == 0 && start.is_none() && (c.is_whitespace() || c == ';') => continue,
            _ => {}
        }
        if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        let rest = text[s..].trim();
        if !rest.is_empty() {
            out.push(rest);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_run_command_document_parses_verbatim() {
        let parsed = parse(r#"{ "ping": 1 }"#).unwrap();
        assert_eq!(parsed, MongoCommand::RunCommand(json!({"ping": 1})));
    }

    #[test]
    fn a_non_object_json_value_is_rejected_as_run_command() {
        assert!(parse("[1, 2, 3]").is_err());
    }

    #[test]
    fn find_sugar_with_one_filter_argument() {
        let parsed = parse(r#"db.users.find({ "active": true })"#).unwrap();
        assert_eq!(
            parsed,
            MongoCommand::Sugar {
                collection: "users".to_string(),
                method: "find".to_string(),
                args: vec![json!({"active": true})],
            }
        );
    }

    #[test]
    fn find_sugar_with_filter_and_projection() {
        let parsed = parse(r#"db.users.find({"active": true}, {"name": 1})"#).unwrap();
        let MongoCommand::Sugar { args, .. } = parsed else {
            panic!("expected sugar");
        };
        assert_eq!(args, vec![json!({"active": true}), json!({"name": 1})]);
    }

    #[test]
    fn find_sugar_with_no_arguments() {
        let parsed = parse("db.users.find()").unwrap();
        let MongoCommand::Sugar { args, .. } = parsed else {
            panic!("expected sugar");
        };
        assert!(args.is_empty());
    }

    #[test]
    fn a_trailing_semicolon_is_tolerated() {
        let parsed = parse("db.users.find();").unwrap();
        assert!(matches!(parsed, MongoCommand::Sugar { .. }));
    }

    #[test]
    fn every_documented_sugar_method_parses() {
        for method in SUGAR_METHODS {
            let text = format!("db.things.{method}({{}})");
            let parsed = parse(&text).unwrap_or_else(|e| panic!("{method} failed: {e}"));
            assert!(matches!(parsed, MongoCommand::Sugar { .. }));
        }
    }

    #[test]
    fn an_unsupported_method_is_rejected() {
        let error = parse("db.users.mapReduce({})").unwrap_err();
        assert!(error.0.contains("mapReduce"));
    }

    #[test]
    fn a_comma_inside_a_nested_document_is_not_an_argument_separator() {
        let parsed = parse(r#"db.users.updateOne({"id": 1}, {"$set": {"a": 1, "b": 2}})"#).unwrap();
        let MongoCommand::Sugar { args, .. } = parsed else {
            panic!("expected sugar");
        };
        assert_eq!(args.len(), 2);
        assert_eq!(args[1], json!({"$set": {"a": 1, "b": 2}}));
    }

    #[test]
    fn a_comma_inside_a_quoted_string_argument_is_not_a_separator() {
        let parsed = parse(r#"db.users.find({"name": "a, b"})"#).unwrap();
        let MongoCommand::Sugar { args, .. } = parsed else {
            panic!("expected sugar");
        };
        assert_eq!(args, vec![json!({"name": "a, b"})]);
    }

    #[test]
    fn missing_db_prefix_is_rejected() {
        assert!(parse("users.find({})").is_err());
    }

    #[test]
    fn missing_method_is_rejected() {
        assert!(parse("db.users").is_err());
    }

    #[test]
    fn unterminated_call_is_rejected() {
        assert!(parse("db.users.find({}").is_err());
    }

    #[test]
    fn invalid_json_argument_is_rejected() {
        assert!(parse("db.users.find({not json})").is_err());
    }

    #[test]
    fn anything_that_is_neither_a_document_nor_sugar_is_rejected() {
        assert!(parse("SELECT * FROM users").is_err());
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
    }

    #[test]
    fn two_semicolon_separated_calls_split_into_two_statements() {
        assert_eq!(
            split_commands("db.a.find({});db.b.find({});"),
            vec!["db.a.find({})", "db.b.find({})"]
        );
    }

    #[test]
    fn two_newline_separated_calls_with_no_semicolons_still_split() {
        assert_eq!(
            split_commands("db.a.find({})\ndb.b.find({})"),
            vec!["db.a.find({})", "db.b.find({})"]
        );
    }

    #[test]
    fn back_to_back_run_command_documents_split_on_the_closing_brace() {
        assert_eq!(
            split_commands(r#"{"ping": 1} {"ping": 2}"#),
            vec![r#"{"ping": 1}"#, r#"{"ping": 2}"#]
        );
    }

    #[test]
    fn a_semicolon_inside_a_nested_document_does_not_split() {
        assert_eq!(
            split_commands(r#"db.a.find({"msg": "a;b"});"#),
            vec![r#"db.a.find({"msg": "a;b"})"#]
        );
    }

    #[test]
    fn a_single_statement_with_no_terminator_is_kept_whole() {
        assert_eq!(split_commands("db.users.find({})"), vec!["db.users.find({})"]);
    }

    #[test]
    fn blank_input_yields_no_statements() {
        assert_eq!(split_commands("   \n\t "), Vec::<&str>::new());
    }

    #[test]
    fn read_only_methods_are_classified_correctly() {
        assert!(is_read_only_method("find"));
        assert!(is_read_only_method("aggregate"));
        assert!(is_read_only_method("countDocuments"));
        assert!(is_read_only_method("distinct"));
        assert!(!is_read_only_method("insertOne"));
        assert!(!is_read_only_method("updateMany"));
        assert!(!is_read_only_method("deleteOne"));
    }

    #[test]
    fn run_command_name_reads_the_first_key() {
        assert_eq!(
            run_command_name(&json!({"insert": "users", "documents": []})),
            Some("insert")
        );
        assert_eq!(run_command_name(&json!([1, 2])), None);
    }

    #[test]
    fn read_only_run_commands_are_classified_correctly() {
        assert!(is_read_only_run_command("find"));
        assert!(is_read_only_run_command("aggregate"));
        assert!(!is_read_only_run_command("insert"));
        assert!(!is_read_only_run_command("update"));
        assert!(!is_read_only_run_command("mapReduce"));
    }

    #[test]
    fn split_statements_separates_two_sugar_calls_on_separate_lines() {
        let text = "db.users.find({\"a\": 1})\ndb.orders.find({\"b\": 2})";
        assert_eq!(
            split_statements(text),
            vec!["db.users.find({\"a\": 1})", "db.orders.find({\"b\": 2})"]
        );
    }

    #[test]
    fn split_statements_separates_two_run_command_documents() {
        let text = "{\"ping\": 1}\n{\"find\": \"users\"}";
        assert_eq!(split_statements(text), vec!["{\"ping\": 1}", "{\"find\": \"users\"}"]);
    }

    #[test]
    fn split_statements_ignores_a_trailing_semicolon_between_statements() {
        let text = "db.users.find({});\ndb.users.find({})";
        assert_eq!(
            split_statements(text),
            vec!["db.users.find({})", "db.users.find({})"]
        );
    }

    #[test]
    fn split_statements_does_not_split_on_a_brace_nested_inside_a_call() {
        let text = r#"db.users.updateOne({"id": 1}, {"$set": {"a": 1}})"#;
        assert_eq!(split_statements(text), vec![text]);
    }

    #[test]
    fn split_statements_does_not_split_on_a_closing_paren_inside_a_quoted_string() {
        let text = r#"db.users.find({"note": "a) b"})"#;
        assert_eq!(split_statements(text), vec![text]);
    }

    #[test]
    fn split_statements_on_empty_text_is_empty() {
        assert!(split_statements("   \n  ").is_empty());
    }
}
