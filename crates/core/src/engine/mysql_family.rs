//! What MySQL and MariaDB servers say of themselves, read the same way wherever it is read, and
//! the rules the two engines share.

use super::{EngineFacts, EngineId, MyFacts, RiskRules, ServerFacts, TextRules, Version};
use crate::sql::dialect::{Dialect, DollarQuotes, Language};
use crate::sql::risk::Classifier;
use crate::sql::risk::mysql::Judged;

/// Whether a server's version line (`VERSION()`, `@@version`, a greeting's version text) names
/// MariaDB: `11.4.13-MariaDB-ubu2404`, `5.5.5-10.6.12-MariaDB`. Any other line is MySQL's.
pub fn line_names_mariadb(line: &str) -> bool {
    line.contains("MariaDB")
}

/// How MySQL's and MariaDB's text is read.
pub(super) struct FamilyText;

/// The oldest MySQL that reads dollar quotes.
const DOLLAR_QUOTES_FROM: Version = Version::new(8, 1, 0);

impl TextRules for FamilyText {
    /// As the server's greeting says: read from MySQL 8.1.0 on (MySQL's client asks with
    /// `select $$`, which MySQL 8.4 and 9.x refuse as a syntax error, while 8.0 reads `$$` as a
    /// name), never on MariaDB (it reads `$$` as a name).
    fn dollar_quotes(&self, f: &ServerFacts) -> DollarQuotes {
        match &f.detail {
            EngineFacts::MySql(my) | EngineFacts::MariaDb(my) => greeting_dollar_quotes(my),
            // Never the facts of a MySQL or MariaDB server: the reading before any server said.
            EngineFacts::Postgres(_) => DollarQuotes::NotRead,
        }
    }
}

/// Whether the server whose greeting `my` keeps reads dollar quotes. A greeting that names no
/// MariaDB is read as MySQL's, as the driver reads it.
fn greeting_dollar_quotes(my: &MyFacts) -> DollarQuotes {
    match my.greeting {
        EngineId::MariaDb => DollarQuotes::NotRead,
        EngineId::MySql | EngineId::Postgres if my.handshake >= DOLLAR_QUOTES_FROM => DollarQuotes::Read,
        EngineId::MySql | EngineId::Postgres => DollarQuotes::NotRead,
    }
}

/// How MySQL's and MariaDB's statements are judged.
pub(super) struct FamilyRisk;

impl RiskRules for FamilyRisk {
    fn new_classifier(&self, d: Dialect, j: Judged) -> Classifier {
        match j {
            Judged::Own => Classifier::new(Language::Sql(d)),
        }
    }
}
