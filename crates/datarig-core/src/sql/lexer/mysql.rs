//! MySQL's lexer: where a token ends follows MySQL's own (`sql/sql_lex.cc`), and where a
//! statement ends follows its command-line client (`mysql`), which splits a script and sends
//! one statement at a time:
//!
//! * `'…'` and `"…"` are strings (`"…"` a quoted name under `ANSI_QUOTES`); a doubled quote and,
//!   unless `NO_BACKSLASH_ESCAPES`, a backslash escape the next character;
//!   `` `…` `` is a quoted name (a doubled backtick is one);
//! * `#` starts a comment, and so does `--` followed by a blank or a control character (`a--1`
//!   is `a - -1`); both end at `\n` only;
//! * `/* … */` does not nest; `/*!…*/` and `/*!80023 …*/` are executable comments: the server
//!   runs what is inside (when its version is at least the number), so their opening and closing
//!   are tokens of their own ([`Tok::ExecComment`]) and what lies between is code. A `/* … */`
//!   inside one is a comment; `/*+ … */` (optimizer hints) is a comment to the lexer;
//! * names go on through letters, digits, `_`, `$` and any character outside ASCII, and may
//!   start with a digit (`1col`), unless the text is a number (`12`, `1e5`, `0x1F`, `0b01`);
//!   after a name and a `.` written together comes a name, whatever it looks like (`t.1e5`,
//!   `t.select`); there are no dollar quotes;
//! * `x'…'`, `b'…'` and `N'…'` are strings (`_utf8mb4'…'` is a name and a string), `@x`, `@'x'`
//!   and `@@x` variables ([`Tok::Variable`]), `?` a parameter;
//! * the client's `DELIMITER` line: at the start of a line where no statement has begun (only
//!   blanks and comments since the last one), `DELIMITER` (any case) followed by a blank sets the
//!   statement terminator to the word after it (a quoted one may hold blanks; at most
//!   [`MAX_DELIMITER`](super::MAX_DELIMITER) bytes; one with a backslash is refused). The line is a client command
//!   ([`Tok::Directive`]), never sent. The terminator ends a statement wherever it is written
//!   outside a string, a quoted name or a comment, also in the middle of a name (`end$$`), its
//!   case as set; inside an executable comment too, as the client does not read those as
//!   comments.
//!
//! The client's other commands (`\g`, `\G`, `\d`, `go`, `source`, …) are not read: such text
//! goes to the server as it is and fails there.

use super::{Cursor, Delimiter, LexState, Tok, Token, is_space};
use crate::sql::dialect::{Dialect, MySqlMode};

/// MySQL's keywords the highlighter shows and completion offers ([`Dialect::keywords`]): the
/// common ones of MySQL 8.0, 8.4 and 9.x, reserved or not, without the non-reserved words that
/// are often names (`status`, `date`, `text`, `comment`, …). Sorted, upper case. Whether a name
/// needs quotes is decided by the reserved words alone ([`crate::sql::ident::mysql`]).
pub const KEYWORDS: &[&str] = &[
    "ADD",
    "AFTER",
    "ALL",
    "ALTER",
    "ANALYZE",
    "AND",
    "ANY",
    "AS",
    "ASC",
    "AUTO_INCREMENT",
    "BEGIN",
    "BETWEEN",
    "BIGINT",
    "BINARY",
    "BLOB",
    "BOTH",
    "BY",
    "CALL",
    "CASCADE",
    "CASE",
    "CHANGE",
    "CHAR",
    "CHARACTER",
    "CHARSET",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "COLUMNS",
    "COMMIT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "DATABASE",
    "DATABASES",
    "DECIMAL",
    "DECLARE",
    "DEFAULT",
    "DELAYED",
    "DELETE",
    "DESC",
    "DESCRIBE",
    "DISTINCT",
    "DIV",
    "DO",
    "DOUBLE",
    "DROP",
    "DUAL",
    "DUPLICATE",
    "ELSE",
    "ELSEIF",
    "END",
    "ENGINE",
    "EXCEPT",
    "EXISTS",
    "EXPLAIN",
    "FALSE",
    "FIRST",
    "FLOAT",
    "FOR",
    "FORCE",
    "FOREIGN",
    "FROM",
    "FULL",
    "FULLTEXT",
    "FUNCTION",
    "GRANT",
    "GROUP",
    "HAVING",
    "HIGH_PRIORITY",
    "IF",
    "IGNORE",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INT",
    "INTEGER",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "KEYS",
    "KILL",
    "LAST",
    "LATERAL",
    "LEADING",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LOCK",
    "LONGTEXT",
    "LOW_PRIORITY",
    "MEDIUMINT",
    "MEDIUMTEXT",
    "MOD",
    "MODIFY",
    "NATURAL",
    "NOT",
    "NULL",
    "NULLS",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "OVER",
    "PARTITION",
    "PRIMARY",
    "PROCEDURE",
    "PROCESSLIST",
    "RECURSIVE",
    "REFERENCES",
    "REGEXP",
    "RENAME",
    "REPEAT",
    "REPLACE",
    "RETURN",
    "RETURNS",
    "REVOKE",
    "RIGHT",
    "RLIKE",
    "ROLLBACK",
    "ROW",
    "ROWS",
    "SAVEPOINT",
    "SCHEMA",
    "SCHEMAS",
    "SELECT",
    "SET",
    "SHOW",
    "SIGNAL",
    "SMALLINT",
    "SQL_CALC_FOUND_ROWS",
    "STRAIGHT_JOIN",
    "TABLE",
    "TABLES",
    "TEMPORARY",
    "THEN",
    "TINYINT",
    "TINYTEXT",
    "TO",
    "TRAILING",
    "TRANSACTION",
    "TRIGGER",
    "TRUE",
    "TRUNCATE",
    "UNION",
    "UNIQUE",
    "UNSIGNED",
    "UPDATE",
    "USE",
    "USING",
    "VALUES",
    "VARBINARY",
    "VARCHAR",
    "VARIABLES",
    "VIEW",
    "WARNINGS",
    "WHEN",
    "WHERE",
    "WHILE",
    "WINDOW",
    "WITH",
    "XOR",
    "ZEROFILL",
];

/// A character a name goes on with: an ASCII letter or digit, `_`, `$`, or any character
/// outside ASCII.
fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || !c.is_ascii()
}

/// What follows `--` makes it a comment: a blank, a control character, or the end.
fn dash_comment(next: Option<char>) -> bool {
    next.is_none_or(|c| c <= ' ' || c == '\x7f')
}

/// The client command word.
const DIRECTIVE: &str = "delimiter";

struct Lexer<'a> {
    cur: Cursor<'a>,
    /// The terminator in effect.
    delim: Delimiter,
    /// It is not `;` (no name or number holds a `;`, so they need not look for it).
    custom: bool,
}

impl Lexer<'_> {
    /// The terminator starts at the cursor, where a name or a number would go on.
    fn at_delimiter(&self) -> bool {
        self.custom && self.cur.src[self.cur.pos..].starts_with(self.delim.as_str())
    }

    /// A name character comes next, and the terminator does not start there.
    fn ident_next(&self) -> bool {
        self.cur.peek().is_some_and(is_ident_char) && !self.at_delimiter()
    }

    fn eat_ident(&mut self) {
        while self.ident_next() {
            self.cur.bump();
        }
    }

    fn eat_digits(&mut self) {
        while self.cur.peek().is_some_and(|c| c.is_ascii_digit()) && !self.at_delimiter() {
            self.cur.bump();
        }
    }

    /// An exponent (`e5`, `E+5`, `e-5`) comes next: eat it.
    fn exponent(&mut self) -> bool {
        if !matches!(self.cur.peek(), Some('e' | 'E')) {
            return false;
        }
        let digit_at = if matches!(self.cur.peek_at(1), Some('+' | '-')) { 2 } else { 1 };
        if !self.cur.peek_at(digit_at).is_some_and(|c| c.is_ascii_digit()) {
            return false;
        }
        self.cur.pos += digit_at;
        self.eat_digits();
        true
    }

    /// A number, or a name that starts with a digit.
    fn number_or_ident(&mut self) -> Tok {
        let c = self.cur.peek();
        if c == Some('.') {
            self.cur.bump();
            self.eat_digits();
            self.exponent();
            return Tok::Number;
        }
        if c == Some('0') && matches!(self.cur.peek_at(1), Some('x' | 'b')) {
            let save = self.cur.pos;
            let hex = self.cur.peek_at(1) == Some('x');
            self.cur.pos += 2;
            let from = self.cur.pos;
            self.cur.eat_while(|c| if hex { c.is_ascii_hexdigit() } else { c == '0' || c == '1' });
            if self.cur.pos > from && !self.ident_next() {
                return Tok::Number;
            }
            self.cur.pos = save;
        }
        self.eat_digits();
        if self.exponent() {
            return Tok::Number;
        }
        if self.ident_next() {
            self.eat_ident();
            return Tok::Ident;
        }
        if self.cur.peek() == Some('.') && !self.at_delimiter() {
            self.cur.bump();
            self.eat_digits();
            self.exponent();
        }
        Tok::Number
    }

    /// A variable after `@` (at the cursor): `@name`, `@'name'`, `@@name`; a lone `@` is an
    /// operator.
    fn variable(&mut self, mode: MySqlMode) -> Tok {
        let start = self.cur.pos;
        self.cur.bump();
        if self.cur.peek() == Some('@') {
            self.cur.bump();
        } else if let Some(q @ ('\'' | '"' | '`')) = self.cur.peek() {
            self.cur.bump();
            self.cur.quoted(q, q != '`' && !mode.no_backslash_escapes);
            return Tok::Variable;
        }
        let from = self.cur.pos;
        while (self.ident_next() || self.cur.peek() == Some('.')) && !self.at_delimiter() {
            self.cur.bump();
        }
        if self.cur.pos == from {
            self.cur.pos = start + 1;
            return Tok::Op;
        }
        Tok::Variable
    }
}

/// The tokens of MySQL text `src` (in sql_mode `mode`) lexed from `state`.
pub(super) fn lex(src: &str, mode: MySqlMode, mut state: LexState) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut lx = Lexer { cur: Cursor { src, pos: 0 }, delim: state.delimiter, custom: false };
    let backslash = !mode.no_backslash_escapes;
    // Only blanks and comments since the start of the line.
    let mut line_clean = true;
    while let Some(c) = lx.cur.peek() {
        lx.delim = state.delimiter;
        lx.custom = lx.delim != Delimiter::default();
        let start = lx.cur.pos;
        let delim_len = lx.delim.as_str().len();
        let kind = if src[start..].starts_with(lx.delim.as_str()) {
            lx.cur.pos += delim_len;
            Tok::Semi
        } else if is_space(c) {
            lx.cur.eat_while(is_space);
            Tok::Whitespace
        } else if c == '#' || (c == '-' && lx.cur.peek_at(1) == Some('-') && dash_comment(lx.cur.peek_at(2))) {
            lx.cur.eat_while(|c| c != '\n');
            Tok::LineComment
        } else if c == '/' && lx.cur.peek_at(1) == Some('*') {
            if lx.cur.peek_at(2) == Some('!') {
                lx.cur.pos += 3;
                let digits = src[lx.cur.pos..].bytes().take_while(u8::is_ascii_digit).count();
                if digits >= 5 {
                    lx.cur.pos += digits.min(6);
                }
                Tok::ExecComment
            } else {
                lx.cur.pos += 2;
                match src[lx.cur.pos..].find("*/") {
                    Some(i) => lx.cur.pos += i + 2,
                    None => lx.cur.pos = src.len(),
                }
                Tok::BlockComment
            }
        } else if state.exec && c == '*' && lx.cur.peek_at(1) == Some('/') {
            lx.cur.pos += 2;
            Tok::ExecComment
        } else if c == '\'' || (c == '"' && !mode.ansi_quotes) {
            lx.cur.bump();
            lx.cur.quoted(c, backslash);
            Tok::Str
        } else if c == '"' || c == '`' {
            lx.cur.bump();
            lx.cur.quoted(c, false);
            Tok::QuotedIdent
        } else if c == '@' {
            lx.variable(mode)
        } else if c == '?' {
            lx.cur.bump();
            Tok::Param
        } else if is_ident_char(c) && after_qualifier(&out, start) {
            lx.eat_ident();
            Tok::Ident
        } else if c.is_ascii_digit()
            || (c == '.' && lx.cur.peek_at(1).is_some_and(|c| c.is_ascii_digit()) && !glued_name(&out, start))
        {
            lx.number_or_ident()
        } else if is_ident_char(c) {
            if matches!(c, 'x' | 'X' | 'b' | 'B') && lx.cur.peek_at(1) == Some('\'') {
                lx.cur.pos += 2;
                lx.cur.quoted('\'', false);
                Tok::Str
            } else if matches!(c, 'n' | 'N') && lx.cur.peek_at(1) == Some('\'') {
                lx.cur.pos += 2;
                lx.cur.quoted('\'', backslash);
                Tok::Str
            } else if line_clean && !state.pending && directive_at(&src[start..]) {
                lx.cur.eat_while(|c| c != '\n');
                Tok::Directive
            } else {
                lx.eat_ident();
                if Dialect::MySql(mode).is_keyword(&src[start..lx.cur.pos]) { Tok::Keyword } else { Tok::Ident }
            }
        } else {
            lx.cur.bump();
            match c {
                '.' => Tok::Dot,
                ',' => Tok::Comma,
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                _ => Tok::Op,
            }
        };
        let t = Token { kind, start, end: lx.cur.pos };
        line_clean = match t.kind {
            Tok::Whitespace | Tok::BlockComment => line_clean || t.text(src).contains('\n'),
            Tok::LineComment => line_clean,
            _ => false,
        };
        state = after(state, &t, src);
        out.push(t);
    }
    out
}

/// A `DELIMITER` command starts `rest`: the word, then a blank or the end of the line.
fn directive_at(rest: &str) -> bool {
    rest.get(..DIRECTIVE.len()).is_some_and(|w| w.eq_ignore_ascii_case(DIRECTIVE))
        && rest[DIRECTIVE.len()..].chars().next().is_none_or(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

/// The last token is a `.` written right after a name (or a keyword read as one): what follows
/// at `at` is a name.
fn after_qualifier(out: &[Token], at: usize) -> bool {
    match out {
        [.., name, dot] => {
            dot.kind == Tok::Dot
                && dot.end == at
                && name.end == dot.start
                && matches!(name.kind, Tok::Ident | Tok::QuotedIdent | Tok::Keyword)
        }
        _ => false,
    }
}

/// The last token is a name that ends at `at` (a `.` there qualifies it).
fn glued_name(out: &[Token], at: usize) -> bool {
    out.last().is_some_and(|t| t.end == at && matches!(t.kind, Tok::Ident | Tok::QuotedIdent | Tok::Keyword))
}

/// The terminator a `DELIMITER` line sets: the word after the command (up to a blank, or the
/// text between quotes), `None` when there is none or it holds a backslash (the client
/// refuses it and keeps the one it had).
pub(super) fn directive_delimiter(line: &str) -> Option<Delimiter> {
    let rest = line.get(DIRECTIVE.len()..)?.trim_start_matches([' ', '\t']);
    let arg = match rest.chars().next()? {
        q @ ('\'' | '"' | '`') => {
            let inner = &rest[1..];
            &inner[..inner.find(q).unwrap_or(inner.len())]
        }
        _ => &rest[..rest.find([' ', '\t', '\r', '\n']).unwrap_or(rest.len())],
    };
    if arg.contains('\\') {
        return None;
    }
    Delimiter::new(arg)
}

/// The state after token `t` of `src` ([`LexState::after`]).
pub(super) fn after(mut s: LexState, t: &Token, src: &str) -> LexState {
    match t.kind {
        Tok::Whitespace | Tok::LineComment | Tok::BlockComment => {}
        // A statement's text ends here: the client sends it, and the server reads what comes
        // after as a new text, outside any executable comment.
        Tok::Semi => {
            s.pending = false;
            s.exec = false;
        }
        Tok::Directive => {
            if let Some(d) = directive_delimiter(t.text(src)) {
                s.delimiter = d;
            }
            s.pending = false;
        }
        Tok::ExecComment => {
            s.exec = t.text(src).starts_with("/*");
            s.pending = true;
        }
        _ => s.pending = true,
    }
    s
}

#[cfg(test)]
mod tests;
