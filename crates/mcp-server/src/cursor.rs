use serde::Serialize;

/// A tab's last-reported cursor position (M4's `get_cursor_position`).
/// `line` is 1-based like every other line the MCP tools report; `column`
/// is 0-based.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CursorPosition {
    pub line: u32,
    pub column: u32,
}

impl CursorPosition {
    /// From the editor's own 0-based (line, column).
    pub fn from_zero_based(line: u32, column: u32) -> Self {
        Self {
            line: line + 1,
            column,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_one_based_line_and_zero_based_column() {
        let wire = serde_json::to_value(CursorPosition::from_zero_based(0, 4)).unwrap();
        assert_eq!(wire, serde_json::json!({"line": 1, "column": 4}));
    }
}
