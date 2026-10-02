//! Q6 — the "suppress this finding" quick fix (ADR-0070): a comment on its
//! own line above the finding, built from the analyzer's `suppress-comment`
//! template. Pure text rules; the editor splices the edit in.

/// Insert `new_text` at the start of `line` (0-based): a zero-width edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineInsertion {
    pub line: u32,
    pub new_text: String,
}

/// The comment for `code`: `template` with `{code}` filled in.
pub fn comment(template: &str, code: &str) -> String {
    template.replace("{code}", code)
}

/// The text a "suppress" quick fix inserts above `line` of `buffer`: the
/// comment, indented like the line it silences, ending with the buffer's own
/// line terminator (`\r\n` when the finding's line has one).
///
/// A `line` past the end of the buffer is clamped to the last line, so the
/// answer is always a legal insertion point.
pub fn suppress_insertion(template: &str, code: &str, buffer: &str, line: u32) -> LineInsertion {
    let lines: Vec<&str> = buffer.split_inclusive('\n').collect();
    let line = (line as usize).min(lines.len().saturating_sub(1));
    let target = lines.get(line).copied().unwrap_or("");
    let indent: String = target
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let eol = if target.ends_with("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    LineInsertion {
        line: line as u32,
        new_text: format!("{indent}{}{eol}", comment(template, code)),
    }
}

/// The quick fix's menu title.
pub fn title(analyzer_name: &str, code: &str) -> String {
    format!("Suppress {code} ({analyzer_name})")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHPSTAN: &str = "// @phpstan-ignore {code}";

    #[test]
    fn the_comment_is_inserted_above_with_the_lines_indentation() {
        let buffer = "<?php\nclass A {\n    function f() {\n\t\techo $x;\n    }\n}\n";
        let at_spaces = suppress_insertion(PHPSTAN, "variable.undefined", buffer, 2);
        assert_eq!(at_spaces.line, 2);
        assert_eq!(
            at_spaces.new_text,
            "    // @phpstan-ignore variable.undefined\n"
        );
        let at_tabs = suppress_insertion(PHPSTAN, "variable.undefined", buffer, 3);
        assert_eq!(
            at_tabs.new_text,
            "\t\t// @phpstan-ignore variable.undefined\n"
        );
    }

    #[test]
    fn a_crlf_buffer_gets_a_crlf_comment() {
        let insertion = suppress_insertion(PHPSTAN, "a.b", "<?php\r\necho 1;\r\n", 1);
        assert_eq!(insertion.new_text, "// @phpstan-ignore a.b\r\n");
    }

    #[test]
    fn a_line_past_the_end_is_clamped_and_an_empty_buffer_works() {
        let insertion = suppress_insertion(PHPSTAN, "a.b", "<?php\n  echo 1;", 99);
        assert_eq!(insertion.line, 1);
        assert_eq!(insertion.new_text, "  // @phpstan-ignore a.b\n");
        let empty = suppress_insertion(PHPSTAN, "a.b", "", 0);
        assert_eq!(
            (empty.line, empty.new_text.as_str()),
            (0, "// @phpstan-ignore a.b\n")
        );
    }

    #[test]
    fn templates_and_titles_fill_in_the_code() {
        assert_eq!(
            comment("/** @psalm-suppress {code} */", "UndefinedVariable"),
            "/** @psalm-suppress UndefinedVariable */"
        );
        assert_eq!(title("PHPStan", "a.b"), "Suppress a.b (PHPStan)");
    }
}
