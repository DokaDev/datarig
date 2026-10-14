//! MySQL's lexer: where a token ends follows MySQL's own (`sql/sql_lex.cc`), and where a
//! statement ends follows its command-line client (`mysql`, its `add_line`), which splits a
//! script and sends one statement at a time:
//!
//! * `'…'` and `"…"` are strings (`"…"` a quoted name under `ANSI_QUOTES`), `x'…'`, `b'…'` and
//!   `N'…'` too (`_utf8mb4'…'` is a name and a string); in each a doubled quote and, unless
//!   `NO_BACKSLASH_ESCAPES`, a backslash escape the next character (as the client reads them,
//!   in a quoted name under `ANSI_QUOTES` and in hex and bit strings as well).
//!   `` `…` `` is a quoted name (a doubled backtick is one);
//! * `#` starts a comment, and so does `--` followed by a blank or the end of the line (`a--1`
//!   is `a - -1`); both end at `\n` only;
//! * `/* … */` does not nest; `/*!…*/` and `/*!80023 …*/` are executable comments: the server
//!   runs what is inside (when its version is at least the number: five digits, or six from
//!   MySQL 8.4 on), so
//!   their opening and closing are tokens of their own ([`Tok::ExecComment`]) and what lies
//!   between is code. A `/* … */` inside one is a comment; `/*+ … */` (optimizer hints) is a
//!   comment;
//! * names go on through letters, digits, `_`, `$` and any character outside ASCII, and may
//!   start with a digit (`1col`), unless the text is a number (`12`, `1e5`, `0x1F`, `0b01`);
//!   after a name and a `.` written together comes a name, whatever it looks like (`t.1e5`,
//!   `t.select`);
//! * `@x`, `@'x'` and `@@x` are variables ([`Tok::Variable`]), `?` a parameter;
//! * dollar quotes (`$$…$$`, `$tag$…$tag$`) only on a server whose client reads them
//!   (`MySqlMode::dollar_quotes`);
//! * the client's `DELIMITER` line: at the start of a line (only blanks and comments on that
//!   line before it, the line not starting inside a comment or a string) where no statement
//!   has begun, `DELIMITER` (any case) followed by a blank or the line's end sets the statement
//!   terminator to the word after it: up to a space (a backslash escapes the next character),
//!   or the text between quotes; at most [`MAX_DELIMITER`](super::MAX_DELIMITER) bytes; one
//!   that holds a backslash is refused. The line is a client command ([`Tok::Directive`]),
//!   never sent. The terminator ends a statement wherever it starts outside a string, a quoted
//!   name or a comment, at an ASCII character (the client passes other characters over), also
//!   in a name or a number (`end$$`), its case as set; inside an executable comment too, as the
//!   client does not read those as comments;
//! * `\g` and `\G` outside strings end a statement too (as the interactive client, and MySQL 8's
//!   in a script; 9.x's needs `--commands` there); a backslash outside strings takes the next
//!   character with it (`\'` opens no string), and at the end of a line it is dropped.
//!
//! Where the client's own reading is broken, the server's is followed: an optimizer hint over
//! several lines stays a comment (the client reads its next lines as code); a `DELIMITER` line is
//! no command after a statement on the same line or after an executable comment (the client then
//! drops text: those go to the server and fail there); `@$$` is a variable (the client opens a
//! dollar quote after the `@`); the terminator is looked for where a token starts and inside names,
//! numbers, variables and blanks, not inside a comment's two-character marks (`*/` then `/`, with
//! `//` as the terminator) or a string's prefix (`x'`); a terminator of more than 15 bytes is cut
//! at a character's end, not at the 15th byte. The client's other commands (`\c`, `\d`, `go`,
//! `source`, `use` without a terminator, …) are not read: such text goes to the server as it is and
//! fails there.

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

/// What follows `--` makes it a comment, to the client: a blank, or the end of the line.
fn dash_comment(next: Option<char>) -> bool {
    next.is_none_or(is_space)
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
    /// The terminator starts at the cursor (at an ASCII character), where a name, a number or
    /// blanks would go on.
    fn at_delimiter(&self) -> bool {
        let rest = &self.cur.src[self.cur.pos..];
        self.custom && rest.as_bytes().first().is_some_and(u8::is_ascii) && rest.starts_with(self.delim.as_str())
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

    /// Eat characters while `f` holds and the terminator does not start.
    fn eat_while(&mut self, f: impl Fn(char) -> bool) {
        while self.cur.peek().is_some_and(&f) && !self.at_delimiter() {
            self.cur.bump();
        }
    }

    fn eat_digits(&mut self) {
        self.eat_while(|c| c.is_ascii_digit());
    }

    /// An exponent (`e5`, `E+5`, `e-5`) comes next: eat it.
    fn exponent(&mut self) -> bool {
        if !matches!(self.cur.peek(), Some('e' | 'E')) || self.at_delimiter() {
            return false;
        }
        let digit_at = if matches!(self.cur.peek_at(1), Some('+' | '-')) { 2 } else { 1 };
        if !self.cur.peek_at(digit_at).is_some_and(|c| c.is_ascii_digit()) {
            return false;
        }
        let save = self.cur.pos;
        self.cur.bump();
        if digit_at == 2 {
            if self.at_delimiter() {
                self.cur.pos = save;
                return false;
            }
            self.cur.bump();
        }
        if self.at_delimiter() {
            self.cur.pos = save;
            return false;
        }
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
            self.cur.bump();
            if !self.at_delimiter() {
                self.cur.bump();
                let from = self.cur.pos;
                self.eat_while(|c| if hex { c.is_ascii_hexdigit() } else { c == '0' || c == '1' });
                if self.cur.pos > from && !self.ident_next() {
                    return Tok::Number;
                }
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
        if self.cur.peek() == Some('@') && !self.at_delimiter() {
            self.cur.bump();
        } else if let Some(q @ ('\'' | '"' | '`')) = self.cur.peek()
            && !self.at_delimiter()
        {
            self.cur.bump();
            self.cur.quoted(q, q != '`' && !mode.no_backslash_escapes);
            return Tok::Variable;
        }
        let from = self.cur.pos;
        self.eat_while(|c| is_ident_char(c) || c == '.');
        if self.cur.pos == from {
            self.cur.pos = start + 1;
            return Tok::Op;
        }
        Tok::Variable
    }

    /// A dollar quote (`$$` or `$tag$`, the tag of name characters on the same line) starts at
    /// the cursor: eat it to its closing tag (as the client reads it, a backslash escaping the
    /// next character unless `NO_BACKSLASH_ESCAPES`) or the end.
    fn dollar_quote(&mut self, backslash: bool) -> bool {
        let rest = &self.cur.src[self.cur.pos..];
        let Some(close) = rest[1..].find(|c: char| c == '$' || !is_ident_char(c) || c == '\n') else { return false };
        if !rest[1 + close..].starts_with('$') {
            return false;
        }
        let tag = &rest[..close + 2];
        self.cur.pos += tag.len();
        while self.cur.pos < self.cur.src.len() {
            let here = &self.cur.src[self.cur.pos..];
            if here.starts_with(tag) {
                self.cur.pos += tag.len();
                return true;
            }
            let c = self.cur.bump();
            if backslash && c == Some('\\') {
                self.cur.bump();
            }
        }
        true
    }
}

/// The tokens of MySQL text `src` (in sql_mode `mode`) lexed from `state`.
pub(super) fn lex(src: &str, mode: MySqlMode, mut state: LexState) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut lx = Lexer { cur: Cursor { src, pos: 0 }, delim: state.delimiter, custom: false };
    let backslash = !mode.no_backslash_escapes;
    while let Some(c) = lx.cur.peek() {
        lx.delim = state.delimiter;
        lx.custom = lx.delim != Delimiter::default();
        let start = lx.cur.pos;
        let kind = if c.is_ascii() && src[start..].starts_with(lx.delim.as_str()) {
            lx.cur.pos += lx.delim.as_str().len();
            Tok::Semi
        } else if c == '\\' {
            // A client command: `\g` and `\G` send the statement; another takes its character
            // with it. At the end of a line the client drops it.
            lx.cur.bump();
            let rest = &src[lx.cur.pos..];
            match lx.cur.peek() {
                _ if rest.is_empty() || rest.starts_with('\n') || rest.starts_with("\r\n") || rest == "\r" => {
                    Tok::Whitespace
                }
                Some('g' | 'G') => {
                    lx.cur.bump();
                    Tok::Semi
                }
                _ => {
                    lx.cur.bump();
                    Tok::Op
                }
            }
        } else if is_space(c) {
            lx.cur.bump();
            lx.eat_while(is_space);
            Tok::Whitespace
        } else if c == '#' || (c == '-' && lx.cur.peek_at(1) == Some('-') && dash_comment(lx.cur.peek_at(2))) {
            lx.cur.eat_while(|c| c != '\n');
            Tok::LineComment
        } else if c == '/' && lx.cur.peek_at(1) == Some('*') {
            if lx.cur.peek_at(2) == Some('!') {
                lx.cur.pos += 3;
                // The version: six digits, or else five.
                let digits = src[lx.cur.pos..].bytes().take_while(u8::is_ascii_digit).count();
                lx.cur.pos += match digits {
                    6 => 6,
                    5.. => 5,
                    _ => 0,
                };
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
        } else if c == '\'' || c == '"' {
            lx.cur.bump();
            lx.cur.quoted(c, backslash);
            if c == '"' && mode.ansi_quotes { Tok::QuotedIdent } else { Tok::Str }
        } else if c == '`' {
            lx.cur.bump();
            lx.cur.quoted(c, false);
            Tok::QuotedIdent
        } else if c == '$' && mode.dollar_quotes && !glued_after_ident(src, start) && lx.dollar_quote(backslash) {
            Tok::Str
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
            if matches!(c, 'x' | 'X' | 'b' | 'B' | 'n' | 'N') && lx.cur.peek_at(1) == Some('\'') {
                lx.cur.pos += 2;
                lx.cur.quoted('\'', backslash);
                Tok::Str
            } else if state.line_start && !state.pending && !state.exec && directive_at(&src[start..]) {
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
        state = after(state, &t, src);
        out.push(t);
    }
    out
}

/// A name character is right before byte `at` of `src` (a `$` there goes on a name, as the
/// client sees it, and opens no dollar quote).
fn glued_after_ident(src: &str, at: usize) -> bool {
    src[..at].chars().next_back().is_some_and(is_ident_char)
}

/// A `DELIMITER` command starts `rest`: the word, then a space, a tab or the end of the line
/// (a `\r\n` one too: the client reads lines without their carriage return).
fn directive_at(rest: &str) -> bool {
    let after = rest.get(DIRECTIVE.len()..).unwrap_or("");
    rest.get(..DIRECTIVE.len()).is_some_and(|w| w.eq_ignore_ascii_case(DIRECTIVE))
        && (after.is_empty() || after.starts_with([' ', '\t', '\n']) || after.starts_with("\r\n") || after == "\r")
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

/// The terminator a `DELIMITER` line sets, as the client reads it: after the command and
/// blanks, the text between quotes, or up to a space (a backslash dropped, the character after
/// it kept); `None` when there is none or it holds a backslash (the client refuses it and keeps
/// the one it had).
pub(super) fn directive_delimiter(line: &str) -> Option<Delimiter> {
    // The client reads a line without its carriage return.
    let line = line.strip_suffix('\r').unwrap_or(line);
    let rest = line.get(DIRECTIVE.len()..)?.trim_start_matches(is_space);
    let arg: String = match rest.chars().next()? {
        q @ ('\'' | '"' | '`') => {
            let inner = &rest[1..];
            inner[..inner.find(q).unwrap_or(inner.len())].to_string()
        }
        _ => {
            let mut arg = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => match chars.next() {
                        Some(next) => arg.push(next),
                        None => arg.push(c),
                    },
                    ' ' => break,
                    c => arg.push(c),
                }
            }
            arg
        }
    };
    if arg.contains('\\') {
        return None;
    }
    Delimiter::new(&arg)
}

/// The state after token `t` of `src` ([`LexState::after`]).
pub(super) fn after(mut s: LexState, t: &Token, src: &str) -> LexState {
    let breaks = t.text(src).contains('\n');
    s.line_start = match t.kind {
        Tok::Whitespace => s.line_start || breaks,
        // A line that starts inside a comment holds no command.
        Tok::BlockComment => s.line_start && !breaks,
        Tok::LineComment => s.line_start,
        _ => false,
    };
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
