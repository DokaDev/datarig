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

/// The SQL dialect a tab's text is in. PostgreSQL is the only one for now; a MySQL variant
/// will join it, and an exhaustive `match` makes every tool decide what to do with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    #[default]
    Postgres,
}

impl Dialect {
    /// The character the app quotes an identifier with (PostgreSQL: `"`). Inside a quoted name
    /// it is written twice.
    pub fn ident_quote(self) -> char {
        match self {
            Self::Postgres => '"',
        }
    }

    /// Every character an identifier may be quoted with when it is read (PostgreSQL: `"` only;
    /// a dialect may accept more than the one it writes with, [`Dialect::ident_quote`]).
    pub fn ident_quotes(self) -> &'static [char] {
        match self {
            Self::Postgres => &['"'],
        }
    }

    /// The quote character `token` opens with, if it opens a quoted identifier
    /// ([`Dialect::ident_quotes`]).
    pub fn opening_quote(self, token: &str) -> Option<char> {
        token.chars().next().filter(|c| self.ident_quotes().contains(c))
    }

    /// Whether `name` must be quoted to be read back as that same name: it is not what the
    /// server folds a bare name to, or it is a keyword that cannot stand bare
    /// (PostgreSQL: [`ident::needs_quotes`]).
    pub fn needs_quotes(self, name: &str) -> bool {
        match self {
            Self::Postgres => ident::needs_quotes(name),
        }
    }

    /// `name` as SQL: bare when that reads back as the same name ([`Dialect::needs_quotes`]),
    /// else quoted ([`Dialect::force_quote_ident`]): `Mixed Col` → `"Mixed Col"`.
    pub fn quote_ident(self, name: &str) -> String {
        if self.needs_quotes(name) { self.force_quote_ident(name) } else { name.to_string() }
    }

    /// `name` as SQL, always quoted (any name, any case, keywords), the quote character
    /// doubled inside: `Order "Items"` → `"Order ""Items"""`.
    pub fn force_quote_ident(self, name: &str) -> String {
        match self {
            Self::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// `s` as a string literal. PostgreSQL: `'` doubled, written for
    /// `standard_conforming_strings = on` (the default since 9.1), where a backslash is an
    /// ordinary character.
    pub fn quote_literal(self, s: &str) -> String {
        match self {
            Self::Postgres => format!("'{}'", s.replace('\'', "''")),
        }
    }

    /// The name an unquoted identifier stands for: PostgreSQL folds ASCII letters to lower
    /// case (`Shop` is `shop`; other letters stay as written).
    pub fn fold(self, name: &str) -> String {
        match self {
            Self::Postgres => name.to_ascii_lowercase(),
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

    /// [`Dialect::unquote`] that takes what it gets: the quote characters at either end are
    /// dropped, however many there are (a name being typed has no closing one yet).
    pub fn unquote_lenient(self, token: &str) -> String {
        let q = self.opening_quote(token).unwrap_or(self.ident_quote());
        self.unescape_ident(q, token.trim_matches(self.ident_quotes()))
    }

    /// The text inside identifier quotes `quote` as the name it stands for: each doubled quote
    /// character is one.
    pub fn unescape_ident(self, quote: char, inner: &str) -> String {
        inner.replace(&format!("{quote}{quote}"), &quote.to_string())
    }

    /// The words the lexer reads as keywords (the highlighter shows them, completion offers
    /// them): sorted, upper case. PostgreSQL: [`lexer::KEYWORDS`].
    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Postgres => lexer::KEYWORDS,
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
    pub fn default_path(self, schema: Option<&str>) -> Vec<String> {
        match self {
            Self::Postgres => {
                let mut path: Vec<String> = schema.into_iter().map(str::to_string).collect();
                if schema != Some("public") {
                    path.push("public".to_string());
                }
                path
            }
        }
    }

    /// What starts a line comment the editor writes (PostgreSQL: `--`).
    pub fn comment_marker(self) -> &'static str {
        match self {
            Self::Postgres => "--",
        }
    }

    /// The line comment markers the editor strips to uncomment a line (PostgreSQL: `--`).
    pub fn uncomment_markers(self) -> &'static [&'static str] {
        match self {
            Self::Postgres => &["--"],
        }
    }

    /// The dialect `sqlformat` lays text of this dialect out as.
    pub fn sqlformat_dialect(self) -> sqlformat::Dialect {
        match self {
            Self::Postgres => sqlformat::Dialect::PostgreSql,
        }
    }

    /// The SQL that asks the server for `statement`'s plan in the format the plan view reads
    /// (`analyze`: run it and measure), `None` when the dialect has no such statement.
    /// PostgreSQL: [`plan::explain_sql`].
    pub fn explain_sql(self, statement: &str, analyze: bool) -> Option<String> {
        match self {
            Self::Postgres => Some(plan::explain_sql(statement, analyze)),
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
