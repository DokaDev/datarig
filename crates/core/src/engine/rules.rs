//! The rules a tab's text and statements are judged by, as far as the app knows its server:
//! [`Rules`], one answer per question whatever the engine.

use super::{Expected, ServerFacts};
use crate::sql::dialect::{Dialect, Language};
use crate::sql::risk::Classifier;
use crate::sql::risk::mysql::Judged;
use std::sync::Arc;

/// The language of a tab bound to no profile: PostgreSQL's, for highlighting only (nothing runs
/// without a profile).
pub const UNBOUND_TAB_DIALECT: Dialect = Dialect::Postgres;

/// What the app knows of the server a tab's text is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rules {
    /// The tab is bound to no profile (or to one that is not there any more).
    Unbound,
    /// The profile is known, but no session of it said what its server is. Text is judged by
    /// the pre-connect judge: in `last`, the language a session of the profile last said (the
    /// tab's own, else the profile's), when it is of an engine the profile `expected`, else in
    /// `driver`, the language the profile's driver offers before any server answered.
    Expected { expected: Expected, last: Option<Language>, driver: Language },
    /// A session of the profile said what its server is (`facts`, of the session that is open
    /// now), and the text is read in `language`, chosen as before the server answered.
    Server { facts: Arc<ServerFacts>, language: Language },
}

impl Rules {
    /// The language the tab's text is read in.
    pub fn language(&self) -> Language {
        match self {
            Rules::Unbound => Language::Sql(UNBOUND_TAB_DIALECT),
            Rules::Expected { expected, last, driver } => {
                last.filter(|l| expected.admits(l.dialect().engine())).unwrap_or(*driver)
            }
            Rules::Server { language, .. } => *language,
        }
    }

    /// A fresh classifier of the tab's text ([`Rules::language`]), with nothing prepared: its
    /// engine's.
    pub fn classifier(&self) -> Classifier {
        let d = self.language().dialect();
        d.engine().spec().risk.new_classifier(d, Judged::Own)
    }
}
