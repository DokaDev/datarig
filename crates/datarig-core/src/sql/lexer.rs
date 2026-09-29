//! Tolerant PostgreSQL lexer. Never fails: unterminated strings/comments/dollar bodies run to
//! the end of input. Shared by the highlighter, the statement splitter and the completer.
//!
//! Where a token ends follows PostgreSQL's own scanner (`src/backend/parser/scan.l`), so the
//! statements the splitter finds are the ones the server runs:
//!
//! * whitespace is ASCII only (space, tab, `\n`, `\r`, form feed, vertical tab); every other
//!   character outside ASCII (a no-break space, an emoji, a letter) is an identifier character;
//! * a `--` comment ends at `\n` or `\r` (a lone carriage return ends it too);
//! * an identifier goes on through letters, digits, `_`, `$` and any non-ASCII character, so a
//!   `$` right after an identifier (`x$$`) belongs to it and starts no dollar quote;
//! * a dollar-quote tag is `$$` or `$tag$` where `tag` starts with a letter, `_` or a non-ASCII
//!   character and goes on with those and digits.
//!
//! The differential tests in `split/tests.rs` hold the splitter to the boundaries of
//! PostgreSQL's parser (libpg_query) on a corpus of tricky texts.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Whitespace,
    LineComment,
    BlockComment,
    Str,
    Dollar,
    QuotedIdent,
    Number,
    Keyword,
    Ident,
    Param,
    Op,
    Semi,
    Dot,
    Comma,
    LParen,
    RParen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Tok,
    pub start: usize,
    pub end: usize,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
    pub fn is_trivia(&self) -> bool {
        matches!(self.kind, Tok::Whitespace | Tok::LineComment | Tok::BlockComment)
    }
    pub fn is_word(&self) -> bool {
        matches!(self.kind, Tok::Keyword | Tok::Ident)
    }
}

/// Sorted, upper-case. Used for highlighting and keyword completion.
pub const KEYWORDS: &[&str] = &[
    "ALL",
    "ALTER",
    "ANALYZE",
    "AND",
    "ANY",
    "ARRAY",
    "AS",
    "ASC",
    "BEGIN",
    "BETWEEN",
    "BIGINT",
    "BOOLEAN",
    "BOTH",
    "BY",
    "CASCADE",
    "CASE",
    "CAST",
    "CHECK",
    "COALESCE",
    "COLUMN",
    "COMMIT",
    "CONFLICT",
    "CONSTRAINT",
    "COPY",
    "CREATE",
    "CROSS",
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "DATABASE",
    "DECLARE",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DO",
    "DROP",
    "ELSE",
    "END",
    "EXCEPT",
    "EXISTS",
    "EXPLAIN",
    "EXTRACT",
    "FALSE",
    "FETCH",
    "FILTER",
    "FIRST",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "FUNCTION",
    "GRANT",
    "GROUP",
    "HAVING",
    "ILIKE",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTEGER",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LAST",
    "LATERAL",
    "LEFT",
    "LIKE",
    "LIMIT",
    "MATERIALIZED",
    "NATURAL",
    "NOT",
    "NOTHING",
    "NULL",
    "NULLS",
    "NUMERIC",
    "OFFSET",
    "ON",
    "ONLY",
    "OR",
    "ORDER",
    "OUTER",
    "OVER",
    "PARTITION",
    "PRIMARY",
    "REFERENCES",
    "REPLACE",
    "RETURNING",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "ROW",
    "ROWS",
    "SCHEMA",
    "SELECT",
    "SET",
    "SHOW",
    "SOME",
    "TABLE",
    "TEXT",
    "THEN",
    "TO",
    "TRUE",
    "TRUNCATE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "VACUUM",
    "VALUES",
    "VIEW",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
];

pub fn is_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    KEYWORDS.binary_search(&upper.as_str()).is_ok()
}

/// `ident_start` of scan.l: a letter, `_`, or any character outside ASCII.
fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic() || !c.is_ascii()
}

/// `ident_cont` of scan.l: `ident_start`, a digit or `$`.
fn is_ident_continue(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit() || c == '$'
}

/// `space` of scan.l: ASCII whitespace only.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\x0b')
}

struct Cursor<'a> {
    src: &'a str,
    pos: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }
    fn peek_at(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }
    fn eat_while(&mut self, f: impl Fn(char) -> bool) {
        while let Some(c) = self.peek() {
            if !f(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
    }
    /// Quoted run ending in `q`; a doubled `q` is an escape. With `backslash`, `\x` escapes too.
    fn quoted(&mut self, q: char, backslash: bool) {
        while let Some(c) = self.bump() {
            if backslash && c == '\\' {
                self.bump();
            } else if c == q {
                if self.peek() == Some(q) {
                    self.bump();
                } else {
                    return;
                }
            }
        }
    }
    /// If a dollar-quote tag (`$$` or `$tag$`) starts here, return its byte length.
    fn dollar_tag_len(&self) -> Option<usize> {
        let rest = &self.src[self.pos..];
        let mut it = rest.char_indices().skip(1);
        match it.next() {
            Some((i, '$')) => return Some(i + 1),
            Some((_, c)) if is_ident_start(c) => {}
            _ => return None,
        }
        for (i, c) in it {
            if c == '$' {
                return Some(i + 1);
            }
            if !(is_ident_start(c) || c.is_ascii_digit()) {
                return None;
            }
        }
        None
    }
}

pub fn lex(src: &str) -> Vec<Token> {
    lex_with(src, false)
}

/// [`lex`] as a server with `standard_conforming_strings = off` reads the text: a backslash
/// escapes the next character in a plain `'…'` string too. Only the safety checks use it
/// (`risk`), to see what such a server would run.
pub fn lex_backslash_strings(src: &str) -> Vec<Token> {
    lex_with(src, true)
}

fn lex_with(src: &str, backslash_strings: bool) -> Vec<Token> {
    let mut out = Vec::new();
    let mut cur = Cursor { src, pos: 0 };
    while let Some(c) = cur.peek() {
        let start = cur.pos;
        let kind = if is_space(c) {
            cur.eat_while(is_space);
            Tok::Whitespace
        } else if c == '-' && cur.peek_at(1) == Some('-') {
            cur.eat_while(|c| c != '\n' && c != '\r');
            Tok::LineComment
        } else if c == '/' && cur.peek_at(1) == Some('*') {
            cur.pos += 2;
            let mut depth = 1;
            while depth > 0 {
                match cur.bump() {
                    None => break,
                    Some('*') if cur.peek() == Some('/') => {
                        cur.bump();
                        depth -= 1;
                    }
                    Some('/') if cur.peek() == Some('*') => {
                        cur.bump();
                        depth += 1;
                    }
                    Some(_) => {}
                }
            }
            Tok::BlockComment
        } else if c == '\'' {
            cur.bump();
            cur.quoted('\'', backslash_strings);
            Tok::Str
        } else if c == '"' {
            cur.bump();
            cur.quoted('"', false);
            Tok::QuotedIdent
        } else if c == '$' {
            if let Some(len) = cur.dollar_tag_len() {
                let tag = &src[cur.pos..cur.pos + len];
                cur.pos += len;
                match src[cur.pos..].find(tag) {
                    Some(i) => cur.pos += i + len,
                    None => cur.pos = src.len(),
                }
                Tok::Dollar
            } else if cur.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                cur.bump();
                cur.eat_while(|c| c.is_ascii_digit());
                Tok::Param
            } else {
                cur.bump();
                Tok::Op
            }
        } else if c.is_ascii_digit()
            || (c == '.'
                && cur.peek_at(1).is_some_and(|c| c.is_ascii_digit())
                && !matches!(out.last(), Some(Token { kind: Tok::Ident | Tok::QuotedIdent | Tok::Keyword, end, .. }) if *end == start))
        {
            cur.eat_while(|c| c.is_ascii_digit());
            if cur.peek() == Some('.') && cur.peek_at(1) != Some('.') {
                cur.bump();
                cur.eat_while(|c| c.is_ascii_digit());
            }
            if matches!(cur.peek(), Some('e' | 'E')) {
                let sign = matches!(cur.peek_at(1), Some('+' | '-'));
                let digit_at = if sign { 2 } else { 1 };
                if cur.peek_at(digit_at).is_some_and(|c| c.is_ascii_digit()) {
                    cur.pos += digit_at;
                    cur.eat_while(|c| c.is_ascii_digit());
                }
            }
            Tok::Number
        } else if is_ident_start(c) {
            // E'..', B'..', X'..', N'..' string prefixes
            if matches!(c, 'e' | 'E' | 'b' | 'B' | 'x' | 'X' | 'n' | 'N') && cur.peek_at(1) == Some('\'') {
                cur.pos += 2;
                cur.quoted('\'', matches!(c, 'e' | 'E'));
                Tok::Str
            } else {
                cur.eat_while(is_ident_continue);
                if is_keyword(&src[start..cur.pos]) { Tok::Keyword } else { Tok::Ident }
            }
        } else {
            cur.bump();
            match c {
                ';' => Tok::Semi,
                '.' => Tok::Dot,
                ',' => Tok::Comma,
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                _ => Tok::Op,
            }
        };
        out.push(Token { kind, start, end: cur.pos });
    }
    out
}

/// Whether statement `sql` may change the schema, so cached metadata such as the key columns
/// may be out of date after it runs: DDL (its first word is `CREATE`, `ALTER` or `DROP`), and
/// `DO` and `CALL`, whose bodies can run any DDL the lexer cannot see. Comments before it are
/// skipped.
pub fn changes_schema(sql: &str) -> bool {
    const WORDS: [&str; 5] = ["CREATE", "ALTER", "DROP", "DO", "CALL"];
    lex(sql)
        .iter()
        .find(|t| !t.is_trivia())
        .is_some_and(|t| t.is_word() && WORDS.iter().any(|w| t.text(sql).eq_ignore_ascii_case(w)))
}

#[cfg(test)]
mod tests;
