//! Which language a tab's text is written in. The SQL tools (lexing, splitting, completion,
//! formatting, quoting, the risk classifier) pick their rules from it; a driver says which one
//! its sessions speak (`driver::Capabilities::language`).
//!
//! A dialect holds what each tool needs to know about it: its keywords, the search path a
//! session starts with ([`Dialect::default_path`]), the comment markers the editor writes and
//! strips, the `sqlformat` dialect it is laid out as, and the SQL the app writes to ask for a
//! statement's plan ([`Dialect::explain_sql`]).
//!
//! A dialect also says how names and strings are written in its SQL ([`Dialect::quote_ident`],
//! [`Dialect::quote_literal`]) and how the server reads a name back ([`Dialect::fold`],
//! [`Dialect::unquote`]): every piece of SQL the app writes, and every name it reads out of
//! SQL text, goes through these.

use super::{ident, lexer, plan};

/// The SQL dialect a tab's text is in. An exhaustive `match` makes every tool decide what to do
/// with each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    #[default]
    Postgres,
    /// MySQL, read as a session in that sql mode reads it.
    MySql(MySqlMode),
}

/// What of a MySQL session changes how its text is read and how strings are written: parts of
/// its `sql_mode`, and whether its server reads dollar quotes. The default is a default MySQL
/// 8.0 server's: `"x"` is a string, a backslash escapes the next character in a string, and
/// `$$` is no quote.
///
/// Until a session says otherwise, text is read in the default mode. On a session whose mode
/// differs, a string the app writes ([`Dialect::quote_literal`]) is read otherwise (under
/// `NO_BACKSLASH_ESCAPES` a backslash written doubled stays doubled), and statements may be cut
/// elsewhere. So a MySQL driver must give the dialect the mode its session has before the app
/// writes or splits SQL for it, and again whenever it changes: MySQL's client reads
/// `NO_BACKSLASH_ESCAPES` from the server's status flags after every statement, as a `SET
/// sql_mode` the user runs changes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct MySqlMode {
    /// `ANSI_QUOTES`: `"x"` is a quoted name, not a string.
    pub ansi_quotes: bool,
    /// `NO_BACKSLASH_ESCAPES`: a backslash in a string is an ordinary character.
    pub no_backslash_escapes: bool,
    /// The server reads dollar quotes (`$$…$$`, `$tag$…$tag$`): MySQL's client asks it with
    /// `select $$` (a syntax error there means yes: MySQL 8.4 and 9.x; 8.0 reads `$$` as a name)
    /// and then does not end a statement inside one.
    pub dollar_quotes: bool,
}

impl Dialect {
    /// The character the app quotes an identifier with (PostgreSQL: `"`). Inside a quoted name
    /// it is written twice.
    pub fn ident_quote(self) -> char {
        match self {
            Self::Postgres => '"',
            Self::MySql(_) => '`',
        }
    }

    /// Every character an identifier may be quoted with when it is read (PostgreSQL: `"` only;
    /// MySQL: `` ` ``, and `"` under `ANSI_QUOTES`; a dialect may accept more than the one it
    /// writes with, [`Dialect::ident_quote`]).
    pub fn ident_quotes(self) -> &'static [char] {
        match self {
            Self::Postgres => &['"'],
            Self::MySql(m) if m.ansi_quotes => &['`', '"'],
            Self::MySql(_) => &['`'],
        }
    }

    /// The quote character `token` opens with, if it opens a quoted identifier
    /// ([`Dialect::ident_quotes`]).
    pub fn opening_quote(self, token: &str) -> Option<char> {
        token.chars().next().filter(|c| self.ident_quotes().contains(c))
    }

    /// Whether `name` must be quoted to be read back as that same name: it is not what the
    /// server folds a bare name to, or it is a keyword that cannot stand bare
    /// (PostgreSQL: [`ident::needs_quotes`]; MySQL: [`ident::mysql::needs_quotes`]).
    pub fn needs_quotes(self, name: &str) -> bool {
        match self {
            Self::Postgres => ident::needs_quotes(name),
            Self::MySql(_) => ident::mysql::needs_quotes(name),
        }
    }

    /// `name` as SQL: bare when that reads back as the same name ([`Dialect::needs_quotes`]),
    /// else quoted ([`Dialect::force_quote_ident`]): `Mixed Col` → `"Mixed Col"`.
    pub fn quote_ident(self, name: &str) -> String {
        if self.needs_quotes(name) { self.force_quote_ident(name) } else { name.to_string() }
    }

    /// `name` as SQL, always quoted (any name, any case, keywords), the quote character
    /// doubled inside: `Order "Items"` → `"Order ""Items"""` (MySQL: `` `Order "Items"` ``).
    pub fn force_quote_ident(self, name: &str) -> String {
        match self {
            Self::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
            Self::MySql(_) => format!("`{}`", name.replace('`', "``")),
        }
    }

    /// `s` as a string literal. PostgreSQL: `'` doubled, written for
    /// `standard_conforming_strings = on` (the default since 9.1), where a backslash is an
    /// ordinary character. MySQL: `'` doubled, and (unless `NO_BACKSLASH_ESCAPES`, see
    /// [`MySqlMode`]) a backslash doubled and a NUL written `\0`, as the server reads a
    /// backslash as an escape.
    pub fn quote_literal(self, s: &str) -> String {
        let open = LiteralOpen { prefix: 0, quote: '\'', backslash: self.backslash_strings() };
        format!("'{}'", self.literal_body(s, open))
    }

    /// A backslash escapes the next character in a plain `'…'` string (MySQL unless
    /// `NO_BACKSLASH_ESCAPES`; never in PostgreSQL, see [`Dialect::quote_literal`]).
    fn backslash_strings(self) -> bool {
        match self {
            Self::Postgres => false,
            Self::MySql(m) => !m.no_backslash_escapes,
        }
    }

    /// The characters a string literal may be quoted with (PostgreSQL: `'`; MySQL: `'`, and
    /// `"` unless `ANSI_QUOTES` makes it a name's quote).
    pub fn string_quotes(self) -> &'static [char] {
        match self {
            Self::Postgres => &['\''],
            Self::MySql(m) if m.ansi_quotes => &['\''],
            Self::MySql(_) => &['\'', '"'],
        }
    }

    /// How string literal `token` (a lexer's string token) opens, when it holds a plain value:
    /// its prefix, its quote and whether a backslash escapes in it. `before` is the text right
    /// before the token. `None` for the strings that hold no such value: bit and hex strings
    /// (`B'…'`, `X'…'`), PostgreSQL's Unicode escapes (`U&'…'`), dollar quotes, and anything
    /// that is not a string. PostgreSQL: `'…'` and `N'…'` (no backslash escapes), `E'…'`
    /// (backslash escapes). MySQL: `'…'`, `"…"` (without `ANSI_QUOTES`) and `N'…'`, a backslash
    /// escaping unless `NO_BACKSLASH_ESCAPES`.
    pub fn literal_open(self, before: &str, token: &str) -> Option<LiteralOpen> {
        let mut chars = token.chars();
        let first = chars.next()?;
        let (prefix, quote) = if self.string_quotes().contains(&first) {
            (0, first)
        } else {
            (first.len_utf8(), chars.next().filter(|&q| q == '\'')?)
        };
        let backslash = match (self, prefix, first.to_ascii_uppercase()) {
            (_, 0, _) | (_, _, 'N') => self.backslash_strings(),
            (Self::Postgres, _, 'E') => true,
            _ => return None,
        };
        // PostgreSQL's `U&'…'` is `U`, `&` and a string to the lexer.
        if self == Self::Postgres
            && prefix == 0
            && let Some(rest) = before.strip_suffix('&')
            && rest.ends_with(['U', 'u'])
            && !rest[..rest.len() - 1].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
        {
            return None;
        }
        Some(LiteralOpen { prefix, quote, backslash })
    }

    /// `s` written inside a string literal that opens as `open` (without its quotes): its
    /// quote doubled, and with backslash escapes a backslash doubled and a NUL written `\0`.
    /// [`Dialect::quote_literal`] is a plain literal's.
    pub fn literal_body(self, s: &str, open: LiteralOpen) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                c if c == open.quote => {
                    out.push(c);
                    out.push(c);
                }
                '\\' if open.backslash => out.push_str("\\\\"),
                '\0' if open.backslash => out.push_str("\\0"),
                c => out.push(c),
            }
        }
        out
    }

    /// The value the text `body` between the quotes of a literal that opens as `open` stands
    /// for: a doubled quote is one, and with backslash escapes each escape is its character
    /// (PostgreSQL's `E'…'`: `\n`, `\t`, octal, `\x`, `\u` and the others; MySQL's: `\0`, `\n`,
    /// `\Z`, … and `\%`, `\_` kept as written). Lenient: an escape cut short (a literal being
    /// typed) is kept as written.
    pub fn unescape_literal_body(self, body: &str, open: LiteralOpen) -> String {
        let mut out = String::with_capacity(body.len());
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            if c == open.quote {
                out.push(c);
                if chars.peek() == Some(&c) {
                    chars.next();
                }
            } else if c == '\\' && open.backslash {
                let Some(e) = chars.next() else {
                    out.push(c);
                    break;
                };
                match self {
                    Self::Postgres => unescape_pg(e, &mut chars, &mut out),
                    Self::MySql(_) => match e {
                        '0' => out.push('\0'),
                        'b' => out.push('\x08'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'Z' => out.push('\x1a'),
                        '%' | '_' => {
                            out.push('\\');
                            out.push(e);
                        }
                        e => out.push(e),
                    },
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// The value string literal `token` holds ([`Dialect::literal_open`], with nothing before
    /// it), `None` unless it is such a literal and closed.
    pub fn unquote_literal(self, token: &str) -> Option<String> {
        let open = self.literal_open("", token)?;
        let body = token.get(open.prefix + open.quote.len_utf8()..)?.strip_suffix(open.quote)?;
        // The last quote must close it: not one half of a doubled quote, nor escaped.
        let lexed = super::lexer::lex_in(&format!("{token} "), self);
        lexed.first().filter(|t| t.end == token.len())?;
        Some(self.unescape_literal_body(body, open))
    }

    /// The dialect of the SQL a server shows of its own (a check's condition, a column's
    /// type): MySQL writes it as its default mode reads it (a quote in a string escaped `\'`,
    /// names in backticks), whatever the session's mode.
    pub fn printed(self) -> Self {
        match self {
            Self::Postgres => Self::Postgres,
            Self::MySql(_) => Self::MySql(MySqlMode::default()),
        }
    }

    /// The name an unquoted identifier stands for: PostgreSQL folds ASCII letters to lower
    /// case (`Shop` is `shop`; other letters stay as written). MySQL keeps it as written
    /// (whether names that differ in case are the same table is the server's
    /// `lower_case_table_names`).
    pub fn fold(self, name: &str) -> String {
        match self {
            Self::Postgres => name.to_ascii_lowercase(),
            Self::MySql(_) => name.to_string(),
        }
    }

    /// The name a quoted identifier `token` stands for (`"Mixed ""Q"""` → `Mixed "Q"`): the text
    /// between its first and last character, each doubled quote one. `None` unless `token` is at
    /// least two characters long and starts and ends with the same quote character (no closing
    /// one, as while it is being typed). What lies between is not checked: `"a""` (whose last
    /// quote is escaped, so it is not closed) is read as `a"`.
    pub fn unquote(self, token: &str) -> Option<String> {
        let q = self.opening_quote(token)?;
        let closed = token.len() >= 2 && token.ends_with(q);
        closed.then(|| self.unescape_ident(q, &token[q.len_utf8()..token.len() - q.len_utf8()]))
    }

    /// [`Dialect::unquote`] that takes what it gets: the opening quote character is dropped at
    /// either end, however many times it is there (a name being typed has no closing one yet);
    /// another quote character the dialect reads stays (`` `say "hi"` `` is `say "hi"`).
    pub fn unquote_lenient(self, token: &str) -> String {
        let q = self.opening_quote(token).unwrap_or(self.ident_quote());
        self.unescape_ident(q, token.trim_matches(q))
    }

    /// The text inside identifier quotes `quote` as the name it stands for: each doubled quote
    /// character is one.
    pub fn unescape_ident(self, quote: char, inner: &str) -> String {
        inner.replace(&format!("{quote}{quote}"), &quote.to_string())
    }

    /// The words the lexer reads as keywords (the highlighter shows them, completion offers
    /// them): sorted, upper case. PostgreSQL: [`lexer::KEYWORDS`]; MySQL:
    /// [`lexer::mysql::KEYWORDS`].
    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Postgres => lexer::KEYWORDS,
            Self::MySql(_) => lexer::mysql::KEYWORDS,
        }
    }

    /// Whether `word` is one of [`Dialect::keywords`], whatever its case.
    pub fn is_keyword(self, word: &str) -> bool {
        let upper = word.to_ascii_uppercase();
        self.keywords().binary_search(&upper.as_str()).is_ok()
    }

    /// The schemas an unqualified name is looked up in, in order, for a session in `schema`
    /// (`None`: the server's default). PostgreSQL: the search path, `public` (its default
    /// without the `$user` schema) after the session's schema, as an extension's objects there
    /// resolve unqualified; `pg_catalog` is implicitly first on the server and is not listed.
    /// MySQL: the session's database (a database is the schema), none without one.
    pub fn default_path(self, schema: Option<&str>) -> Vec<String> {
        match self {
            Self::Postgres => {
                let mut path: Vec<String> = schema.into_iter().map(str::to_string).collect();
                if schema != Some("public") {
                    path.push("public".to_string());
                }
                path
            }
            Self::MySql(_) => schema.into_iter().map(str::to_string).collect(),
        }
    }

    /// What starts a line comment the editor writes: `--` (followed by a blank, or the end of
    /// the line, which MySQL needs).
    pub fn comment_marker(self) -> &'static str {
        match self {
            Self::Postgres | Self::MySql(_) => "--",
        }
    }

    /// The line comment marker `text` starts with, when a line comment starts there: what the
    /// editor strips to uncomment a line. PostgreSQL: `--`. MySQL: `#`, or `--` followed by a
    /// blank, a control character or the end (`--1` is code: minus minus one).
    pub fn line_comment_at(self, text: &str) -> Option<&'static str> {
        match self {
            Self::Postgres => text.starts_with("--").then_some("--"),
            Self::MySql(_) if text.starts_with('#') => Some("#"),
            Self::MySql(_) => text
                .strip_prefix("--")
                .is_some_and(|rest| rest.chars().next().is_none_or(|c| c <= ' ' || c == '\x7f'))
                .then_some("--"),
        }
    }

    /// The dialect `sqlformat` lays text of this dialect out as (MySQL: the generic one; it has
    /// none of MySQL's own).
    pub fn sqlformat_dialect(self) -> sqlformat::Dialect {
        match self {
            Self::Postgres => sqlformat::Dialect::PostgreSql,
            Self::MySql(_) => sqlformat::Dialect::Generic,
        }
    }

    /// The SQL that asks the server for `statement`'s plan in the format the plan view reads
    /// (`analyze`: run it and measure), `None` when the dialect has no such statement.
    /// PostgreSQL: [`plan::explain_sql`]. MySQL: none yet, as the plan view does not read
    /// MySQL's plans.
    pub fn explain_sql(self, statement: &str, analyze: bool) -> Option<String> {
        match self {
            Self::Postgres => Some(plan::explain_sql(statement, analyze)),
            Self::MySql(_) => None,
        }
    }
}

/// How a string literal opens ([`Dialect::literal_open`]): the bytes of its prefix before the
/// quote (`E'…'`: 1), its quote character, and whether a backslash escapes in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiteralOpen {
    pub prefix: usize,
    pub quote: char,
    pub backslash: bool,
}

/// PostgreSQL's escape `\e` in an `E'…'` string (the backslash read), the characters after it
/// taken from `chars` as it needs them.
fn unescape_pg(e: char, chars: &mut std::iter::Peekable<std::str::Chars>, out: &mut String) {
    let mut digits = |radix: u32, max: usize| {
        let mut n = 0u32;
        let mut read = String::new();
        while read.len() < max
            && let Some(d) = chars.peek().and_then(|c| c.to_digit(radix))
        {
            n = n * radix + d;
            read.push(chars.next().unwrap_or_default());
        }
        (n, read)
    };
    match e {
        'b' => out.push('\x08'),
        'f' => out.push('\x0c'),
        'n' => out.push('\n'),
        'r' => out.push('\r'),
        't' => out.push('\t'),
        '0'..='7' => {
            let (n, read) = digits(8, 2);
            let n = (e.to_digit(8).unwrap_or(0) << (3 * read.len() as u32)) + n;
            out.extend(char::from_u32(n));
        }
        'x' | 'u' | 'U' => {
            let max = match e {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            match digits(16, max) {
                (_, read) if read.is_empty() => out.push(e),
                (n, read) => match char::from_u32(n) {
                    Some(c) => out.push(c),
                    None => {
                        out.push('\\');
                        out.push(e);
                        out.push_str(&read);
                    }
                },
            }
        }
        e => out.push(e),
    }
}

/// The language of an editor's text: SQL of some dialect (other kinds of consoles may join
/// it later).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Sql(Dialect),
}

impl Default for Language {
    /// PostgreSQL SQL: what a tab without a profile (or with one whose driver is unknown) is
    /// written in.
    fn default() -> Self {
        Self::Sql(Dialect::default())
    }
}

impl Language {
    /// The SQL dialect of the text.
    pub fn dialect(self) -> Dialect {
        match self {
            Self::Sql(d) => d,
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mysql_tests;
