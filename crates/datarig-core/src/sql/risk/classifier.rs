//! The risk classifier of a language: what the app asks about a statement, whatever the
//! dialect. PostgreSQL's is the parse-tree classifier of this module ([`Prepared`],
//! [`repeat`]); MySQL's reads the text's tokens ([`mysql`]). Each call forwards to one of them
//! unchanged.

use super::mysql;
use super::repeat::{self, NotRepeatable};
use super::{Prepared, Risk};
use crate::sql::dialect::{Dialect, Language, MySqlMode};

/// The classifier of one session's statements, with what the session prepared so far (see
/// [`Prepared`]). Built for a language ([`Classifier::new`]); a tab whose language changes
/// gets a new one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Classifier {
    /// PostgreSQL, with the session's prepared statements.
    Pg(Prepared),
    /// MySQL, read in the session's sql mode. Prepared statements are not remembered: every
    /// `EXECUTE` is one of an unknown statement.
    MySql(MySqlMode),
}

impl Default for Classifier {
    /// The classifier of the default language, with nothing prepared.
    fn default() -> Self {
        Self::new(Language::default())
    }
}

impl Classifier {
    /// The classifier of `lang` for a session that prepared nothing.
    pub fn new(lang: Language) -> Self {
        match lang {
            Language::Sql(Dialect::Postgres) => Self::Pg(Prepared::default()),
            Language::Sql(Dialect::MySql(mode)) => Self::MySql(mode),
        }
    }

    /// The language it classifies.
    pub fn language(&self) -> Language {
        match self {
            Self::Pg(_) => Language::Sql(Dialect::Postgres),
            Self::MySql(mode) => Language::Sql(Dialect::MySql(*mode)),
        }
    }

    /// The risk of `sql` (one statement or several) on this session, and what it prepares
    /// or deallocates ([`Prepared::classify`]).
    pub fn classify(&mut self, sql: &str) -> Risk {
        match self {
            Self::Pg(p) => p.classify(sql),
            Self::MySql(mode) => mysql::classify(sql, *mode),
        }
    }

    /// The risk of `sql` in `lang` on a session that prepared nothing (a one-off look).
    pub fn classify_once(lang: Language, sql: &str) -> Risk {
        Self::new(lang).classify(sql)
    }

    /// Forget every name `sql` may have prepared, deallocated or discarded, when whether it
    /// succeeded is unknown ([`Prepared::forget`]).
    pub fn forget(&mut self, sql: &str) {
        match self {
            Self::Pg(p) => p.forget(sql),
            Self::MySql(_) => {}
        }
    }

    /// Whether the session has a statement prepared as `name` ([`Prepared::knows`]).
    pub fn knows(&self, name: &str) -> bool {
        match self {
            Self::Pg(p) => p.knows(name),
            Self::MySql(_) => false,
        }
    }

    /// Whether the app may run `sql` again or count its rows, as far as its text tells
    /// ([`repeat::repeatable`]).
    pub fn repeatable(&self, sql: &str) -> Result<(), NotRepeatable> {
        match self {
            Self::Pg(_) => repeat::repeatable(sql),
            Self::MySql(mode) => mysql::repeatable(sql, *mode),
        }
    }

    /// Whether the query `sql` sorts its rows at the top ([`repeat::ordered`]).
    pub fn ordered(&self, sql: &str) -> bool {
        match self {
            Self::Pg(_) => repeat::ordered(sql),
            Self::MySql(mode) => mysql::ordered(sql, *mode),
        }
    }

    /// The query that counts the rows of `sql` ([`repeat::count_query`]).
    pub fn count_query(&self, sql: &str) -> Result<String, NotRepeatable> {
        match self {
            Self::Pg(_) => repeat::count_query(sql),
            Self::MySql(mode) => mysql::count_query(sql, *mode),
        }
    }
}

#[cfg(test)]
mod tests;
