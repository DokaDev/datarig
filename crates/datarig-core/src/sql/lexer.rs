//! Tolerant SQL lexer. Never fails: unterminated strings/comments/dollar bodies run to the end
//! of input. Shared by the highlighter, the statement splitter, the completer and the
//! formatter, which lex in the dialect of their text ([`lex_in`]); [`lex`] reads PostgreSQL.
//! A text lexed from somewhere other than its start starts from the lexer's state there
//! ([`lex_from`], [`LexState`]); only MySQL text has a state that matters (see [`mysql`]).
//!
//! PostgreSQL: where a token ends follows PostgreSQL's own scanner (`src/backend/parser/scan.l`), so the
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

use super::dialect::Dialect;

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
    /// A user or system variable (MySQL: `@name`, `@'name'`, `@@name`, `@@session.name`).
    Variable,
    /// The opening (`/*!`, `/*!80023`) or the closing `*/` of an executable comment (MySQL):
    /// what lies between is code, lexed as such, not a comment.
    ExecComment,
    /// A client command line that is not sent to the server (MySQL: `DELIMITER //`), from its
    /// word to the end of its line.
    Directive,
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
    /// What a statement ends at: its terminator, or a client command (which comes only between
    /// statements).
    pub fn ends_statement(&self) -> bool {
        matches!(self.kind, Tok::Semi | Tok::Directive)
    }
}

/// The longest statement terminator a client `DELIMITER` sets, in bytes (MySQL's client keeps
/// that many and drops the rest).
pub const MAX_DELIMITER: usize = 15;

/// A statement terminator: `;`, or what a client `DELIMITER` line set (MySQL). Kept inline, so
/// a [`LexState`] is `Copy` and cheap to keep for every line.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Delimiter {
    len: u8,
    bytes: [u8; MAX_DELIMITER],
}

impl Default for Delimiter {
    fn default() -> Self {
        let mut bytes = [0; MAX_DELIMITER];
        bytes[0] = b';';
        Self { len: 1, bytes }
    }
}

impl Delimiter {
    /// `s` as a terminator: its first [`MAX_DELIMITER`] bytes (whole characters), `None` when
    /// empty.
    pub fn new(s: &str) -> Option<Self> {
        let mut len = s.len().min(MAX_DELIMITER);
        while !s.is_char_boundary(len) {
            len -= 1;
        }
        if len == 0 {
            return None;
        }
        let mut bytes = [0; MAX_DELIMITER];
        bytes[..len].copy_from_slice(&s.as_bytes()[..len]);
        Some(Self { len: len as u8, bytes })
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or(";")
    }
}

impl std::fmt::Debug for Delimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

/// What the lexer knows at a place in a text that changes how it reads what follows. Lexing
/// from a place other than the text's start ([`lex_from`]) starts from the state there, which
/// [`LexState::after`] carries over each token. PostgreSQL's is always the default: its tokens
/// never depend on the text before them beyond the token they are in.
///
/// MySQL's: the statement terminator in effect (a client `DELIMITER` line changes it), whether
/// a statement has begun since the last one ended (a `DELIMITER` line counts only between
/// statements) and whether the place is inside an executable comment (`/*! … */`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LexState {
    pub delimiter: Delimiter,
    /// A statement has begun and not ended yet.
    pub pending: bool,
    /// Inside an executable comment: the next `*/` closes it.
    pub exec: bool,
}

impl LexState {
    /// The state after token `t` of `src`, lexed in dialect `d` from this state.
    pub fn after(self, t: &Token, src: &str, d: Dialect) -> Self {
        match d {
            Dialect::Postgres => self,
            Dialect::MySql(_) => mysql::after(self, t, src),
        }
    }
}

/// PostgreSQL's keywords ([`Dialect::keywords`]): sorted, upper-case. Used for highlighting
/// and keyword completion.
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

/// Whether `word` is one of PostgreSQL's [`KEYWORDS`] ([`Dialect::is_keyword`]).
pub fn is_keyword(word: &str) -> bool {
    Dialect::Postgres.is_keyword(word)
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
pub fn is_space(c: char) -> bool {
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

/// `src` as PostgreSQL reads it ([`lex_in`]).
pub fn lex(src: &str) -> Vec<Token> {
    lex_in(src, Dialect::Postgres)
}

/// The tokens of `src` as dialect `d` reads it.
pub fn lex_in(src: &str, d: Dialect) -> Vec<Token> {
    lex_from(src, d, LexState::default())
}

/// The tokens of `src`, a text in dialect `d` that starts where the lexer's state is `state`
/// (a line of a longer text: [`LexState::after`] the tokens before it).
pub fn lex_from(src: &str, d: Dialect, state: LexState) -> Vec<Token> {
    match d {
        Dialect::Postgres => lex_with(src, d, false),
        Dialect::MySql(mode) => mysql::lex(src, mode, state),
    }
}

/// [`lex`] as a server with `standard_conforming_strings = off` reads the text: a backslash
/// escapes the next character in a plain `'…'` string too. Only the safety checks use it
/// (`risk`), to see what such a server would run.
pub fn lex_backslash_strings(src: &str) -> Vec<Token> {
    lex_with(src, Dialect::Postgres, true)
}

/// PostgreSQL's scanner, its words told apart by `d`'s keywords.
fn lex_with(src: &str, d: Dialect, backslash_strings: bool) -> Vec<Token> {
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
                if d.is_keyword(&src[start..cur.pos]) { Tok::Keyword } else { Tok::Ident }
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
/// skipped. PostgreSQL ([`changes_schema_in`]).
pub fn changes_schema(sql: &str) -> bool {
    changes_schema_in(sql, Dialect::Postgres)
}

/// [`changes_schema`] for statement `sql` in dialect `d`. MySQL: DDL (`CREATE`, `ALTER`,
/// `DROP`, `RENAME`, `TRUNCATE`) and `CALL` (its `DO` evaluates expressions), the opening of an
/// executable comment before it skipped too (`/*!40101 ALTER … */`).
pub fn changes_schema_in(sql: &str, d: Dialect) -> bool {
    let words: &[&str] = match d {
        Dialect::Postgres => &["CREATE", "ALTER", "DROP", "DO", "CALL"],
        Dialect::MySql(_) => &["CREATE", "ALTER", "DROP", "RENAME", "TRUNCATE", "CALL"],
    };
    lex_in(sql, d)
        .iter()
        .filter(|t| !t.is_trivia())
        .find(|t| !(t.kind == Tok::ExecComment && t.text(sql).starts_with("/*")))
        .is_some_and(|t| t.is_word() && words.iter().any(|w| t.text(sql).eq_ignore_ascii_case(w)))
}

pub mod mysql;

#[cfg(test)]
mod tests;
