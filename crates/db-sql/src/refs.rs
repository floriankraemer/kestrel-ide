//! Table-reference and alias extraction shared by `completion` and
//! `inspections` — a heuristic, not a parser: it walks the token stream
//! looking for `FROM`/`JOIN`/`UPDATE`/`INTO` and reads the
//! `[schema.]table [[AS] alias]` that follows, which is enough for
//! completion's and inspections' "what tables are in scope" question
//! without needing a full `sqlparser` AST to answer it (this crate's
//! scanner keeps working on SQL that does not fully parse; a full AST
//! walk would not).

use crate::scan::{Spanned, Tok};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub name: String,
    pub alias: Option<String>,
}

/// Words that end a table reference's alias slot rather than being one —
/// every clause/join keyword that can legally follow a table name.
const NOT_AN_ALIAS: &[&str] = &[
    "ON",
    "USING",
    "WHERE",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "NATURAL",
    "GROUP",
    "ORDER",
    "LIMIT",
    "SET",
    "VALUES",
    "AND",
    "OR",
    "WITH",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "RETURNING",
    "HAVING",
];

fn looks_like_alias(word: &str) -> bool {
    !NOT_AN_ALIAS.iter().any(|kw| word.eq_ignore_ascii_case(kw))
}

pub fn table_refs(tokens: &[Spanned]) -> Vec<TableRef> {
    let mut refs = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let Tok::Word(w) = &tokens[i].tok else {
            i += 1;
            continue;
        };
        if !matches!(
            w.to_ascii_uppercase().as_str(),
            "FROM" | "JOIN" | "UPDATE" | "INTO"
        ) {
            i += 1;
            continue;
        }
        i += 1;
        while let Some(Spanned {
            tok: Tok::Word(first),
            ..
        }) = tokens.get(i)
        {
            let mut name = first.clone();
            i += 1;
            while matches!(tokens.get(i).map(|s| &s.tok), Some(Tok::Symbol('.'))) {
                let Some(Spanned {
                    tok: Tok::Word(part),
                    ..
                }) = tokens.get(i + 1)
                else {
                    break;
                };
                name = part.clone();
                i += 2;
            }
            if matches!(tokens.get(i).map(|s| &s.tok), Some(Tok::Word(w)) if w.eq_ignore_ascii_case("AS"))
            {
                i += 1;
            }
            let mut alias = None;
            if let Some(Spanned {
                tok: Tok::Word(candidate),
                ..
            }) = tokens.get(i)
            {
                if looks_like_alias(candidate) {
                    alias = Some(candidate.clone());
                    i += 1;
                }
            }
            refs.push(TableRef { name, alias });
            if matches!(tokens.get(i).map(|s| &s.tok), Some(Tok::Symbol(','))) {
                i += 1;
                continue;
            }
            break;
        }
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialects::lex_options;
    use crate::scan::scan;
    use db_core::dialect::Dialect;

    fn refs(sql: &str) -> Vec<TableRef> {
        table_refs(&scan(sql, &lex_options(Dialect::Postgres)))
    }

    #[test]
    fn a_single_table_with_no_alias() {
        assert_eq!(
            refs("SELECT * FROM users"),
            vec![TableRef {
                name: "users".into(),
                alias: None
            }]
        );
    }

    #[test]
    fn a_table_with_an_implicit_alias() {
        assert_eq!(
            refs("SELECT * FROM users u"),
            vec![TableRef {
                name: "users".into(),
                alias: Some("u".into())
            }]
        );
    }

    #[test]
    fn a_table_with_an_explicit_as_alias() {
        assert_eq!(
            refs("SELECT * FROM users AS u"),
            vec![TableRef {
                name: "users".into(),
                alias: Some("u".into())
            }]
        );
    }

    #[test]
    fn a_schema_qualified_table_keeps_only_the_table_name() {
        assert_eq!(
            refs("SELECT * FROM public.users u"),
            vec![TableRef {
                name: "users".into(),
                alias: Some("u".into())
            }]
        );
    }

    #[test]
    fn a_join_adds_a_second_table_ref() {
        let found = refs("SELECT * FROM users u JOIN orders o ON u.id = o.user_id");
        assert_eq!(
            found,
            vec![
                TableRef {
                    name: "users".into(),
                    alias: Some("u".into())
                },
                TableRef {
                    name: "orders".into(),
                    alias: Some("o".into())
                },
            ]
        );
    }

    #[test]
    fn comma_joined_tables_in_one_from_clause() {
        assert_eq!(
            refs("SELECT * FROM users u, orders o"),
            vec![
                TableRef {
                    name: "users".into(),
                    alias: Some("u".into())
                },
                TableRef {
                    name: "orders".into(),
                    alias: Some("o".into())
                },
            ]
        );
    }

    #[test]
    fn update_and_into_are_recognised_positions() {
        assert_eq!(
            refs("UPDATE users SET a = 1"),
            vec![TableRef {
                name: "users".into(),
                alias: None
            }]
        );
        assert_eq!(
            refs("INSERT INTO users (a) VALUES (1)"),
            vec![TableRef {
                name: "users".into(),
                alias: None
            }]
        );
    }

    #[test]
    fn a_join_keyword_right_after_the_table_name_is_not_taken_as_an_alias() {
        assert_eq!(
            refs("SELECT * FROM users JOIN orders o ON true"),
            vec![
                TableRef {
                    name: "users".into(),
                    alias: None
                },
                TableRef {
                    name: "orders".into(),
                    alias: Some("o".into())
                },
            ]
        );
    }
}
