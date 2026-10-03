//! `workspace/symbol`: parsing the answer and merging it with the index's
//! name-based hits for Go to Symbol and Go to Class (N3).
//!
//! Index rows come in as plain data, so this crate stays free of `index-core`.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::diagnostics::path_from_uri;
use crate::manager::{LspError, LspManager};

/// Search Everywhere is a popup the user is typing into; a server slower
/// than this is left out of the list rather than waited for.
pub const WORKSPACE_SYMBOL_TIMEOUT: Duration = Duration::from_secs(2);

/// LSP `SymbolKind` numbers that Go to Class lists (LSP 3.17).
const CLASS: u64 = 5;
const ENUM: u64 = 10;
const INTERFACE: u64 = 11;
const STRUCT: u64 = 23;

/// One `workspace/symbol` result, addressed the way the editor jumps:
/// `line` 1-based, `column` 0-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: u64,
    pub container: Option<String>,
    pub path: String,
    pub line: u32,
    pub column: u32,
}

impl WorkspaceSymbol {
    /// A class, interface, trait (servers report traits as classes), enum or
    /// struct: what Go to Class lists.
    pub fn is_class_like(&self) -> bool {
        matches!(self.kind, CLASS | ENUM | INTERFACE | STRUCT)
    }

    /// The kind as the word the index's rows use, so both sources read alike.
    pub fn kind_word(&self) -> &'static str {
        match self.kind {
            CLASS => "class",
            ENUM => "enum",
            INTERFACE => "interface",
            STRUCT => "struct",
            6 | 12 => "method or function",
            8 => "field",
            13 | 14 => "constant",
            7 => "property",
            9 => "constructor",
            22 => "enum member",
            _ => "symbol",
        }
    }
}

/// Parse a `workspace/symbol` result: `SymbolInformation[]` or
/// `WorkspaceSymbol[]` (whose location may lack a range, in which case the
/// symbol points at the top of its file). Malformed entries are dropped.
pub fn parse_workspace_symbols(result: &Value) -> Vec<WorkspaceSymbol> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(symbol).collect()
}

fn symbol(item: &Value) -> Option<WorkspaceSymbol> {
    let location = item.get("location")?;
    let uri = location.get("uri")?.as_str()?;
    let start = location.pointer("/range/start");
    let line = start
        .and_then(|s| s.get("line"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let column = start
        .and_then(|s| s.get("character"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(WorkspaceSymbol {
        name: item.get("name")?.as_str()?.to_string(),
        kind: item.get("kind")?.as_u64()?,
        container: item
            .get("containerName")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_string),
        path: path_from_uri(uri)?,
        line: line as u32 + 1,
        column: column as u32,
    })
}

/// The server's symbols the index did not already list. `known` is
/// `(name, path, line)` of each index row shown; a server symbol on the same
/// line of the same file with the same name is the same definition.
pub fn beyond_index<'a>(
    known: impl IntoIterator<Item = (&'a str, &'a str, u32)>,
    server: Vec<WorkspaceSymbol>,
) -> Vec<WorkspaceSymbol> {
    let seen: HashSet<(&str, &str, u32)> = known.into_iter().collect();
    server
        .into_iter()
        .filter(|s| !seen.contains(&(s.name.as_str(), s.path.as_str(), s.line)))
        .collect()
}

impl LspManager {
    /// `workspace/symbol` for `query` across every language with a running
    /// server. A language whose servers fail or offer nothing contributes no
    /// rows; the error is returned only when no language answered at all.
    pub fn workspace_symbols(&self, query: &str) -> Result<Vec<WorkspaceSymbol>, LspError> {
        let mut symbols = Vec::new();
        let mut first_error = None;
        let mut answered = false;
        for language_id in self.running_languages() {
            match self.request_with_timeout(
                &language_id,
                "workspace/symbol",
                json!({"query": query}),
                WORKSPACE_SYMBOL_TIMEOUT,
            ) {
                Ok(result) => {
                    answered = true;
                    symbols.extend(parse_workspace_symbols(&result));
                }
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match first_error {
            Some(e) if !answered => Err(e),
            _ => Ok(symbols),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, kind: u64, uri: &str, line: u64) -> Value {
        json!({"name": name, "kind": kind, "containerName": "App",
               "location": {"uri": uri, "range": {
                   "start": {"line": line, "character": 6},
                   "end": {"line": line, "character": 9}}}})
    }

    #[test]
    fn symbol_information_and_rangeless_workspace_symbols_both_parse() {
        let symbols = parse_workspace_symbols(&json!([
            info("Foo", 5, "file:///a/Foo.php", 2),
            {"name": "Bar", "kind": 11, "location": {"uri": "file:///a/Bar.php"}},
            {"nope": true},
        ]));
        assert_eq!(symbols.len(), 2);
        assert_eq!(
            (symbols[0].line, symbols[0].column, symbols[0].path.as_str()),
            (3, 6, "/a/Foo.php")
        );
        assert_eq!(symbols[0].container.as_deref(), Some("App"));
        assert_eq!(symbols[1].line, 1);
        assert_eq!(parse_workspace_symbols(&Value::Null), vec![]);
    }

    #[test]
    fn go_to_class_lists_types_only() {
        let kinds: Vec<_> = [5, 6, 10, 11, 12, 23]
            .into_iter()
            .map(|k| {
                parse_workspace_symbols(&json!([info("X", k, "file:///x.php", 0)]))[0]
                    .is_class_like()
            })
            .collect();
        assert_eq!(kinds, [true, false, true, true, false, true]);
    }

    #[test]
    fn a_server_symbol_the_index_already_lists_is_not_repeated() {
        let server = parse_workspace_symbols(&json!([
            info("Foo", 5, "file:///a/Foo.php", 2),
            info("Foo", 5, "file:///b/Foo.php", 2),
            info("Baz", 5, "file:///a/Foo.php", 9),
        ]));
        let fresh = beyond_index([("Foo", "/a/Foo.php", 3)], server);
        let names: Vec<_> = fresh.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(names, ["/b/Foo.php", "/a/Foo.php"]);
    }
}
