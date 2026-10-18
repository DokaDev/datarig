//! The risk classifier of a language: what the app asks about a statement, whatever the
//! dialect. PostgreSQL's is the parse-tree classifier of this module ([`Prepared`],
//! [`repeat`]); MySQL's reads the text's tokens ([`mysql`]). Each call forwards to one of them
//! unchanged.

use super::mysql;
use super::repeat::{self, NotRepeatable};
use super::{Form, Prepared, Risk, TxStep};
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

    /// Where the statement `sql` runs on a session that commits by hand while no transaction
    /// is open ([`TxStep`], [`super::tx_step`]), with what the session prepared so far (an
    /// `EXECUTE` has its statement's risk; one of a name not known to be prepared opens the
    /// transaction). A read runs outside only when it is one statement that the plain-`SELECT`
    /// allowlist takes ([`Classifier::repeatable`]; on MySQL also one that only reads a
    /// variable), or a `SHOW`, a `HELP`, a `DESCRIBE` or an `EXPLAIN` without `ANALYZE` that
    /// calls nothing. Several statements, a text that cannot be read and anything unknown open
    /// it.
    pub fn tx_step(&self, sql: &str) -> TxStep {
        let (form, risk) = match self {
            Self::Pg(p) => (super::form(sql), p.clone().classify(sql)),
            Self::MySql(mode) => (mysql::form(sql, *mode), mysql::classify(sql, *mode)),
        };
        let plain_read = match form {
            None => return TxStep::Opens,
            Some(Form::Query) => match self.repeatable(sql) {
                Ok(()) => true,
                // A variable's value may differ on the next run; reading it changes nothing.
                Err(NotRepeatable::Variable(_)) => matches!(self, Self::MySql(_)),
                Err(_) => false,
            },
            Some(Form::Plain) => true,
            Some(Form::Opens | Form::Other) => false,
        };
        super::tx_step(&risk, plain_read, form == Some(Form::Opens))
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
