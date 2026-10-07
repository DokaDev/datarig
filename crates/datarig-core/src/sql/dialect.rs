//! Which language a tab's text is written in. The SQL tools (lexing, quoting, the risk
//! classifier) pick their rules from it; a driver says which one its sessions speak
//! (`driver::Capabilities::language`).
//!
//! A dialect also says how names and strings are written in its SQL ([`Dialect::quote_ident`],
//! [`Dialect::quote_literal`]) and how the server reads a name back ([`Dialect::fold`],
//! [`Dialect::unquote`]): every piece of SQL the app writes, and every name it reads out of
//! SQL text, goes through these.

use super::ident;

/// The SQL dialect a tab's text is in. PostgreSQL is the only one for now; a MySQL variant
/// will join it, and an exhaustive `match` makes every tool decide what to do with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    #[default]
    Postgres,
}

impl Dialect {
    /// The character that quotes an identifier (PostgreSQL: `"`). Inside a quoted name it is
    /// written twice.
    pub fn ident_quote(self) -> char {
        match self {
            Self::Postgres => '"',
        }
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

    /// The name a quoted identifier `token` stands for (`"Mixed ""Q"""` → `Mixed "Q"`), or
    /// `None` when it is not one closed quoted identifier (no closing quote, as while it is
    /// still being typed).
    pub fn unquote(self, token: &str) -> Option<String> {
        let q = self.ident_quote();
        let closed = token.len() >= 2 && token.starts_with(q) && token.ends_with(q);
        closed.then(|| self.unescape_ident(&token[q.len_utf8()..token.len() - q.len_utf8()]))
    }

    /// [`Dialect::unquote`] that takes what it gets: the quote characters at either end are
    /// dropped, however many there are (a name being typed has no closing one yet).
    pub fn unquote_lenient(self, token: &str) -> String {
        self.unescape_ident(token.trim_matches(self.ident_quote()))
    }

    /// The text inside a quoted identifier as the name it stands for: each doubled quote
    /// character is one.
    pub fn unescape_ident(self, inner: &str) -> String {
        let q = self.ident_quote();
        inner.replace(&format!("{q}{q}"), &q.to_string())
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
