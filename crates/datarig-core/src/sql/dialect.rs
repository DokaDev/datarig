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

/// The parts of a MySQL session's `sql_mode` that change how its text is read and how strings
/// are written. The default is a default MySQL 8 server's: `"x"` is a string and a backslash
/// escapes the next character in a string.
///
/// Until a session says otherwise (its driver reads the mode when it connects), text is read in
/// the default mode. On a server whose mode differs, a string the app writes
/// ([`Dialect::quote_literal`]) is read otherwise: under `NO_BACKSLASH_ESCAPES` a backslash
/// written doubled stays doubled. So a MySQL driver must set or read the mode before the app
/// writes SQL for its sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct MySqlMode {
    /// `ANSI_QUOTES`: `"x"` is a quoted name, not a string.
    pub ansi_quotes: bool,
    /// `NO_BACKSLASH_ESCAPES`: a backslash in a string is an ordinary character.
    pub no_backslash_escapes: bool,
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
        match self {
            Self::Postgres => format!("'{}'", s.replace('\'', "''")),
            Self::MySql(m) if m.no_backslash_escapes => format!("'{}'", s.replace('\'', "''")),
            Self::MySql(_) => {
                let mut out = String::with_capacity(s.len() + 2);
                out.push('\'');
                for c in s.chars() {
                    match c {
                        '\'' => out.push_str("''"),
                        '\\' => out.push_str("\\\\"),
                        '\0' => out.push_str("\\0"),
                        c => out.push(c),
                    }
                }
                out.push('\'');
                out
            }
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
