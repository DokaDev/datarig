//! The risk classifier of a language: what the app asks about a statement, whatever the
//! dialect. PostgreSQL's is the parse-tree classifier of this module ([`Prepared`],
//! [`repeat`]); MySQL's reads the text's tokens ([`mysql`]). Each call forwards to one of them
//! unchanged.

use super::mysql::{self, Judge, Judged};
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
    /// MySQL, read in the session's sql mode, its calls judged on the engines `Judged` says
    /// ([`Judge`]). Prepared statements are not remembered: every `EXECUTE` is one of an
    /// unknown statement.
    MySql(MySqlMode, Judged),
    /// MariaDB, read as MySQL's classifier reads MySQL, its calls judged on the engines `Judged`
    /// says: MariaDB's own lists ([`mysql::MARIADB_TABLES`]). Prepared statements are not
    /// remembered.
    MariaDb(MySqlMode, Judged),
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
            Language::Sql(Dialect::MySql(mode)) => Self::MySql(mode, Judged::Own),
            Language::Sql(Dialect::MariaDb(mode)) => Self::MariaDb(mode, Judged::Own),
        }
    }

    /// The language it classifies.
    pub fn language(&self) -> Language {
        match self {
            Self::Pg(_) => Language::Sql(Dialect::Postgres),
            Self::MySql(mode, _) => Language::Sql(Dialect::MySql(*mode)),
            Self::MariaDb(mode, _) => Language::Sql(Dialect::MariaDb(*mode)),
        }
    }

    /// The risk of `sql` (one statement or several) on this session, and what it prepares
    /// or deallocates ([`Prepared::classify`]).
    pub fn classify(&mut self, sql: &str) -> Risk {
        match self {
            Self::Pg(p) => p.classify(sql),
            Self::MySql(mode, Judged::Own) => mysql::classify(sql, Judge::mysql(*mode)),
            Self::MariaDb(mode, Judged::Own) => mysql::classify(sql, Judge::mariadb(*mode)),
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
            Self::MySql(..) | Self::MariaDb(..) => {}
        }
    }

    /// Whether the session has a statement prepared as `name` ([`Prepared::knows`]).
    pub fn knows(&self, name: &str) -> bool {
        match self {
            Self::Pg(p) => p.knows(name),
            Self::MySql(..) | Self::MariaDb(..) => false,
        }
    }

    /// Whether the app may run `sql` again or count its rows, as far as its text tells
    /// ([`repeat::repeatable`]).
    pub fn repeatable(&self, sql: &str) -> Result<(), NotRepeatable> {
        match self {
            Self::Pg(_) => repeat::repeatable(sql),
            Self::MySql(mode, Judged::Own) => mysql::repeatable(sql, Judge::mysql(*mode)),
            Self::MariaDb(mode, Judged::Own) => mysql::repeatable(sql, Judge::mariadb(*mode)),
        }
    }

    /// [`Classifier::repeatable`] without PostgreSQL's rule on `standard_conforming_strings`
    /// ([`repeat::plain_read`]), for [`Classifier::tx_step`].
    fn plain_read(&self, sql: &str) -> Result<(), NotRepeatable> {
        match self {
            Self::Pg(_) => repeat::plain_read(sql),
            Self::MySql(mode, Judged::Own) => mysql::repeatable(sql, Judge::mysql(*mode)),
            Self::MariaDb(mode, Judged::Own) => mysql::repeatable(sql, Judge::mariadb(*mode)),
        }
    }

    /// Whether `sql` reads the session's time zone ([`mysql::reads_zone`]): run again, its rows
    /// are those of the first run only while the zone is the same. Never on PostgreSQL.
    pub fn reads_zone(&self, sql: &str) -> bool {
        match self {
            Self::Pg(_) => false,
            Self::MySql(mode, Judged::Own) => mysql::reads_zone(sql, Judge::mysql(*mode)),
            Self::MariaDb(mode, Judged::Own) => mysql::reads_zone(sql, Judge::mariadb(*mode)),
        }
    }

    /// Whether the query `sql` sorts its rows at the top ([`repeat::ordered`]).
    pub fn ordered(&self, sql: &str) -> bool {
        match self {
            Self::Pg(_) => repeat::ordered(sql),
            Self::MySql(mode, Judged::Own) => mysql::ordered(sql, Judge::mysql(*mode)),
            Self::MariaDb(mode, Judged::Own) => mysql::ordered(sql, Judge::mariadb(*mode)),
        }
    }

    /// Where the statement `sql` runs on a session that commits by hand while no transaction
    /// is open ([`TxStep`], [`super::tx_step`]), with what the session prepared so far (an
    /// `EXECUTE` has its statement's risk; one of a name not known to be prepared opens the
    /// transaction). A read runs outside only when it is one statement that the plain-`SELECT`
    /// allowlist takes, but for its rule on `standard_conforming_strings`
    /// ([`Classifier::repeatable`]; on MySQL also one that only reads a
    /// variable), or a `SHOW`, a `HELP`, a `DESCRIBE` or an `EXPLAIN` without `ANALYZE` that
    /// calls nothing. Several statements, a text that cannot be read and anything unknown open
    /// it.
    pub fn tx_step(&self, sql: &str) -> TxStep {
        let (form, risk) = match self {
            Self::Pg(p) => (super::form(sql), p.clone().classify(sql)),
            Self::MySql(mode, Judged::Own) => {
                (mysql::form(sql, Judge::mysql(*mode)), mysql::classify(sql, Judge::mysql(*mode)))
            }
            Self::MariaDb(mode, Judged::Own) => {
                (mysql::form(sql, Judge::mariadb(*mode)), mysql::classify(sql, Judge::mariadb(*mode)))
            }
        };
        let plain_read = match form {
            None => return TxStep::Opens,
            // The allowlist without its rule on standard_conforming_strings: a read runs outside
            // a transaction as it did, and the classifier reads it both ways for what it writes.
            Some(Form::Query) => match self.plain_read(sql) {
                Ok(()) => true,
                // A variable's value may differ on the next run; reading it changes nothing.
                Err(NotRepeatable::Variable(_)) => matches!(self, Self::MySql(..) | Self::MariaDb(..)),
                Err(_) => false,
            },
            Some(Form::Plain) => true,
            Some(Form::Opens | Form::Other) => false,
        };
        super::tx_step(&risk, plain_read, form == Some(Form::Opens))
    }

    /// Whether `sql` in `lang` may be a statement a query session starts with (a session
    /// preset's, a profile's own): one statement that sets the session's settings and nothing
    /// else ([`super::session_set`]). MySQL text is read in the default sql mode and with
    /// `ANSI_QUOTES`, as on MySQL and on MariaDB (the server and its mode are not known before
    /// the session opens): the worse counts.
    pub fn session_set(lang: Language, sql: &str) -> Result<super::SessionSet, super::SetProblem> {
        match lang {
            Language::Sql(Dialect::Postgres) => super::session_set::session_set_pg(sql),
            Language::Sql(Dialect::MySql(_) | Dialect::MariaDb(_)) => mysql::session_set(sql),
        }
    }

    /// The query that counts the rows of `sql` ([`repeat::count_query`]).
    pub fn count_query(&self, sql: &str) -> Result<String, NotRepeatable> {
        match self {
            Self::Pg(_) => repeat::count_query(sql),
            Self::MySql(mode, Judged::Own) => mysql::count_query(sql, Judge::mysql(*mode)),
            Self::MariaDb(mode, Judged::Own) => mysql::count_query(sql, Judge::mariadb(*mode)),
        }
    }
}

#[cfg(test)]
mod tests;
