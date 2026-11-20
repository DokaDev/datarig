//! What MySQL and MariaDB servers say of themselves, read the same way wherever it is read, and
//! the rules the two engines share.

use super::{EngineFacts, EngineId, RiskRules, ServerFacts, TextRules, Version};
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
    /// As the server's two reports say, each giving an answer: read from MySQL 8.1.0 on (MySQL's
    /// client asks with `select $$`, which MySQL 8.4 and 9.x refuse as a syntax error, while 8.0
    /// reads `$$` as a name), never on MariaDB (it reads `$$` as a name). The greeting answers
    /// by its engine and number; the `VERSION()` line by the engine it names and the number it
    /// starts with (MariaDB's whatever its number), and not at all when it is MySQL's without a
    /// number. When the answers agree, or only the greeting gives one, that one; when they
    /// differ, not known. A line of the other engine that gives no answer leaves it not known
    /// too: the greeting's is another engine's. How the session reaches the server plays no
    /// part.
    fn dollar_quotes(&self, f: &ServerFacts) -> DollarQuotes {
        match &f.detail {
            EngineFacts::MySql(my) | EngineFacts::MariaDb(my) => {
                let greeting = reads_dollar_quotes(my.greeting, my.handshake);
                let line = match f.engine() {
                    EngineId::MariaDb => Some(DollarQuotes::NotRead),
                    e @ (EngineId::MySql | EngineId::Postgres) => {
                        f.version_text.as_deref().and_then(Version::parse).map(|v| reads_dollar_quotes(e, v))
                    }
                };
                match line {
                    Some(line) if line != greeting => DollarQuotes::NotKnown,
                    Some(_) => greeting,
                    None if f.engine() != my.greeting => DollarQuotes::NotKnown,
                    None => greeting,
                }
            }
            // Never the facts of a MySQL or MariaDB server: the reading before any server said.
            EngineFacts::Postgres(_) => DollarQuotes::NotRead,
        }
    }
}

/// Whether a server of `engine` at version `v` reads dollar quotes, as a report of it says. A
/// report that names no MariaDB is read as MySQL's, as the driver reads it.
fn reads_dollar_quotes(engine: EngineId, v: Version) -> DollarQuotes {
    match engine {
        EngineId::MariaDb => DollarQuotes::NotRead,
        EngineId::MySql | EngineId::Postgres if v >= DOLLAR_QUOTES_FROM => DollarQuotes::Read,
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
