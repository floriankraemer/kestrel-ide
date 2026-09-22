//! Redis console support (F7.3): a RESP argv tokenizer for "one command
//! per line" console input, and a read-only classification table for
//! `db_core::readonly` (whether a command name is safe on a read-only
//! data source). Pure and Qt-free, like the rest of `db-sql` — the
//! `redis` crate itself, and the live connection `db_drivers::redis`
//! drives it over, stay entirely in `db-drivers`.

/// One line of RESP console input, tokenized into argv the way a shell
/// would: whitespace-separated, `'single'`/`"double"` quoting groups
/// spaces into one argument, and `\x` inside a double-quoted argument is a
/// literal escape (`\"`, `\\`, `\n`, `\t`, `\r`) — the same subset `sh`'s
/// own quoting rules cover, not the full RESP wire protocol (this is
/// human-typed console input, never a raw RESP frame).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenizeError {
    UnterminatedQuote,
    TrailingBackslash,
}

pub fn tokenize(line: &str) -> Result<Vec<String>, TokenizeError> {
    let mut args = Vec::new();
    let mut chars = line.chars().peekable();

    loop {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        let mut current = String::new();
        loop {
            match chars.peek() {
                None => break,
                Some(c) if c.is_whitespace() => break,
                Some('"') => {
                    chars.next();
                    loop {
                        match chars.next() {
                            None => return Err(TokenizeError::UnterminatedQuote),
                            Some('"') => break,
                            Some('\\') => match chars.next() {
                                None => return Err(TokenizeError::TrailingBackslash),
                                Some('n') => current.push('\n'),
                                Some('t') => current.push('\t'),
                                Some('r') => current.push('\r'),
                                Some(escaped) => current.push(escaped),
                            },
                            Some(other) => current.push(other),
                        }
                    }
                }
                Some('\'') => {
                    chars.next();
                    loop {
                        match chars.next() {
                            None => return Err(TokenizeError::UnterminatedQuote),
                            Some('\'') => break,
                            Some(other) => current.push(other),
                        }
                    }
                }
                Some(_) => {
                    // Safe: `peek` just proved a char is present.
                    current.push(chars.next().expect("peeked char"));
                }
            }
        }
        args.push(current);
    }

    Ok(args)
}

/// Splits a multi-line Redis console script into its per-line commands,
/// skipping blank lines and `#`-prefixed comment lines — the console's own
/// "one statement per line" convention, distinct from `crate::split`'s
/// SQL-statement splitter.
pub fn split_lines(script: &str) -> Vec<&str> {
    script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// Commands whose effect is confined to reading data — every other named
/// command, and every command this table has never heard of, is treated
/// as a write for a read-only data source's client-side refusal (fail
/// closed: an unrecognised command name is exactly the case where
/// silently allowing it would be the dangerous default).
const READ_ONLY_COMMANDS: &[&str] = &[
    "GET",
    "MGET",
    "STRLEN",
    "GETRANGE",
    "EXISTS",
    "TTL",
    "PTTL",
    "TYPE",
    "KEYS",
    "SCAN",
    "RANDOMKEY",
    "DBSIZE",
    "HGET",
    "HMGET",
    "HGETALL",
    "HKEYS",
    "HVALS",
    "HLEN",
    "HEXISTS",
    "HSCAN",
    "HSTRLEN",
    "LRANGE",
    "LLEN",
    "LINDEX",
    "LPOS",
    "SMEMBERS",
    "SISMEMBER",
    "SMISMEMBER",
    "SCARD",
    "SRANDMEMBER",
    "SSCAN",
    "SUNION",
    "SINTER",
    "SDIFF",
    "ZRANGE",
    "ZREVRANGE",
    "ZRANGEBYSCORE",
    "ZREVRANGEBYSCORE",
    "ZSCORE",
    "ZMSCORE",
    "ZCARD",
    "ZCOUNT",
    "ZRANK",
    "ZREVRANK",
    "ZSCAN",
    "XRANGE",
    "XREVRANGE",
    "XLEN",
    "XREAD",
    "PING",
    "ECHO",
    "COMMAND",
    "INFO",
    "CONFIG",
    "TIME",
    "OBJECT",
    "MEMORY",
    "LASTSAVE",
    "DEBUG",
    "SELECT",
    "AUTH",
    "HELLO",
    "CLIENT",
];

/// Whether `command` (case-insensitively) is safe to run against a
/// read-only data source — `false` for anything not in
/// [`READ_ONLY_COMMANDS`], including a command this table has never heard
/// of.
pub fn is_read_only_command(command: &str) -> bool {
    READ_ONLY_COMMANDS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(command))
}

/// Commands whose effect is a write — the completion popup's other half
/// (F7b): [`is_read_only_command`]'s table exists to gate the read-only
/// guard, but a console's completion needs every command name it might
/// offer, read and write alike, not the read-only subset alone.
const WRITE_COMMANDS: &[&str] = &[
    "SET",
    "SETEX",
    "PSETEX",
    "SETNX",
    "MSET",
    "MSETNX",
    "DEL",
    "UNLINK",
    "EXPIRE",
    "PEXPIRE",
    "EXPIREAT",
    "PERSIST",
    "RENAME",
    "RENAMENX",
    "APPEND",
    "INCR",
    "INCRBY",
    "INCRBYFLOAT",
    "DECR",
    "DECRBY",
    "HSET",
    "HMSET",
    "HSETNX",
    "HDEL",
    "HINCRBY",
    "HINCRBYFLOAT",
    "LPUSH",
    "RPUSH",
    "LPUSHX",
    "RPUSHX",
    "LPOP",
    "RPOP",
    "LSET",
    "LREM",
    "LTRIM",
    "LINSERT",
    "SADD",
    "SREM",
    "SPOP",
    "SMOVE",
    "ZADD",
    "ZREM",
    "ZINCRBY",
    "ZPOPMIN",
    "ZPOPMAX",
    "XADD",
    "XDEL",
    "XTRIM",
    "FLUSHDB",
    "FLUSHALL",
    "COPY",
    "MOVE",
    "RESTORE",
    "SETRANGE",
    "GETSET",
    "GETDEL",
];

/// Every command name this crate knows of, read and write alike, sorted
/// and de-duplicated — what the console's completion popup offers after a
/// bare prefix (F7b), distinct from [`is_read_only_command`]'s narrower
/// read-only-safety table.
pub fn command_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = READ_ONLY_COMMANDS
        .iter()
        .chain(WRITE_COMMANDS.iter())
        .copied()
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_plain_whitespace() {
        assert_eq!(tokenize("SET foo bar").unwrap(), vec!["SET", "foo", "bar"]);
    }

    #[test]
    fn collapses_repeated_whitespace() {
        assert_eq!(
            tokenize("  SET   foo    bar  ").unwrap(),
            vec!["SET", "foo", "bar"]
        );
    }

    #[test]
    fn double_quotes_group_spaces_into_one_argument() {
        assert_eq!(
            tokenize(r#"SET foo "hello world""#).unwrap(),
            vec!["SET", "foo", "hello world"]
        );
    }

    #[test]
    fn single_quotes_group_spaces_and_never_interpret_escapes() {
        assert_eq!(
            tokenize(r"SET foo 'a\nb'").unwrap(),
            vec!["SET", "foo", r"a\nb"]
        );
    }

    #[test]
    fn double_quoted_escapes_are_interpreted() {
        assert_eq!(
            tokenize(r#"SET foo "line1\nline2\ttab\"quote\"""#).unwrap(),
            vec!["SET", "foo", "line1\nline2\ttab\"quote\""]
        );
    }

    #[test]
    fn an_empty_line_tokenizes_to_no_arguments() {
        assert_eq!(tokenize("   ").unwrap(), Vec::<String>::new());
        assert_eq!(tokenize("").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn an_unterminated_double_quote_is_an_error() {
        assert_eq!(
            tokenize(r#"SET foo "unterminated"#).unwrap_err(),
            TokenizeError::UnterminatedQuote
        );
    }

    #[test]
    fn an_unterminated_single_quote_is_an_error() {
        assert_eq!(
            tokenize("SET foo 'unterminated").unwrap_err(),
            TokenizeError::UnterminatedQuote
        );
    }

    #[test]
    fn a_trailing_backslash_inside_quotes_is_an_error() {
        assert_eq!(
            tokenize("SET foo \"a\\").unwrap_err(),
            TokenizeError::TrailingBackslash
        );
    }

    #[test]
    fn adjacent_quoted_and_unquoted_text_join_into_one_argument() {
        assert_eq!(
            tokenize(r#"SET foo bar"baz""#).unwrap(),
            vec!["SET", "foo", "barbaz"]
        );
    }

    #[test]
    fn split_lines_skips_blanks_and_comments_and_trims_each_line() {
        let script = "  GET foo  \n\n# a comment\nSET bar baz\n";
        assert_eq!(split_lines(script), vec!["GET foo", "SET bar baz"]);
    }

    #[test]
    fn read_only_commands_are_recognised_case_insensitively() {
        assert!(is_read_only_command("GET"));
        assert!(is_read_only_command("get"));
        assert!(is_read_only_command("MGET"));
        assert!(is_read_only_command("scan"));
    }

    #[test]
    fn write_commands_are_not_read_only() {
        assert!(!is_read_only_command("SET"));
        assert!(!is_read_only_command("DEL"));
        assert!(!is_read_only_command("EXPIRE"));
        assert!(!is_read_only_command("FLUSHALL"));
        assert!(!is_read_only_command("HSET"));
        assert!(!is_read_only_command("LPUSH"));
        assert!(!is_read_only_command("SADD"));
        assert!(!is_read_only_command("ZADD"));
    }

    #[test]
    fn an_unrecognised_command_is_not_read_only_fail_closed() {
        assert!(!is_read_only_command("SOME.FUTURE.MODULE.COMMAND"));
    }

    #[test]
    fn command_names_includes_both_read_and_write_commands_sorted_and_deduped() {
        let names = command_names();
        assert!(names.contains(&"GET"));
        assert!(names.contains(&"SET"));
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        let mut deduped = names.clone();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len());
    }
}
