//! What MySQL and MariaDB servers say of themselves, read the same way wherever it is read, and
//! the rules the two engines share.

use super::{
    CancelRules, CountInTx, EngineFacts, EngineId, Fact, FilterCalls, OpenVerdict, READ_ONLY_NOT_PROVEN, ReadOnlyProof,
    ServerFacts, SessionRules, Support, TextRules, TxAsk, TxNote, TxRefusal, TxRules, TxStatement, TxVerdict, Version,
    Why,
};
use crate::driver::SessionRole;
use crate::sql::dialect::{Dialect, DollarQuotes, Language};
use crate::sql::risk::mysql::Judged;
use crate::sql::risk::{Class, Classifier, Danger};
use std::time::Duration;

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

/// A fresh classifier of text in dialect `d` of a family session, judged on the engines `j`
/// says: on its own engine's lists, or with the sibling's too. Each engine's
/// [`RiskRules`](super::RiskRules) builds its classifiers with it.
pub(super) fn family_classifier(d: Dialect, j: Judged) -> Classifier {
    match j {
        Judged::Own => Classifier::new(Language::Sql(d)),
        Judged::WithSibling => match d {
            Dialect::MySql(mode) => Classifier::MySql(mode, j),
            Dialect::MariaDb(mode) => Classifier::MariaDb(mode, j),
            // Never a dialect of the family's sessions: its own classifier.
            Dialect::Postgres => Classifier::new(Language::Sql(d)),
        },
    }
}

/// How long a MySQL or MariaDB session may take to answer the rollback of an idle transaction:
/// the rollback undoes the transaction's rows one by one and may take about as long as its
/// writes did (closing the session would not make that faster: the server still undoes them,
/// holding the locks).
const TX_ROLLBACK_ANSWER_WAIT: Duration = Duration::from_secs(60);

/// How MySQL's and MariaDB's statements stand to the transaction open on a session.
pub(super) struct FamilyTx;

impl TxRules for FamilyTx {
    /// With manual commit `LOCK TABLES` is refused. A statement that commits the open
    /// transaction before it runs ends that transaction; with manual commit it asks while one
    /// is open, and DDL is noted while none is (it applies at once and cannot be rolled back).
    fn statement(&self, s: &TxStatement) -> TxVerdict {
        let mut v = TxVerdict::default();
        let r = s.risk;
        if s.manual && r.danger == Some(Danger::Locks) && r.implicit_commit {
            v.refused = Some(TxRefusal::LockTables);
            return v;
        }
        if r.implicit_commit {
            if s.open && s.manual {
                v.ask = Some(TxAsk::ImplicitCommit);
            } else if s.manual && r.class == Class::Ddl {
                v.notes.push(TxNote::MysqlDdl);
            }
            v.ends_tx = true;
        }
        v
    }

    fn rollback_wait(&self) -> Duration {
        TX_ROLLBACK_ANSWER_WAIT
    }

    /// Always: the limit is for the statement alone.
    fn count_in_tx(&self) -> CountInTx {
        CountInTx::Limited
    }

    /// At `SERIALIZABLE`, or with autocommit off, plain reads lock the rows they read.
    fn reads_lock_rows(&self) -> bool {
        true
    }
}

/// What MySQL's and MariaDB's sessions may be given.
pub(super) struct FamilySession;

impl SessionRules for FamilySession {
    /// None: the engines have no idle limit for one transaction (the app's own timer rolls
    /// back).
    fn server_idle_limit(&self) -> Support {
        Fact::No(Why::NotOnEngine)
    }

    /// Only the server's answer makes a MySQL or MariaDB session read-only. A query session the
    /// server did not make read-only is refused; a metadata or monitor session opens anyway and
    /// the user is told what the server answered (a reader that does not keep the setting would
    /// otherwise never connect). A session that proved nothing else opens on no other answer.
    fn read_only_open(&self, role: SessionRole, proof: ReadOnlyProof) -> OpenVerdict {
        match (role, proof) {
            (SessionRole::Query | SessionRole::Meta | SessionRole::Monitor, ReadOnlyProof::Confirmed) => {
                OpenVerdict::Open
            }
            (SessionRole::Query, ReadOnlyProof::Unconfirmed(detail)) => OpenVerdict::Refused(detail),
            (SessionRole::Meta | SessionRole::Monitor, ReadOnlyProof::Unconfirmed(detail)) => {
                OpenVerdict::OpenUnconfirmed(detail)
            }
            (
                SessionRole::Query | SessionRole::Meta | SessionRole::Monitor,
                ReadOnlyProof::PerTransaction | ReadOnlyProof::NotAsked,
            ) => OpenVerdict::Refused(READ_ONLY_NOT_PROVEN.to_string()),
        }
    }
}

/// How MySQL and MariaDB stop a statement.
pub(super) struct FamilyCancel;

impl CancelRules for FamilyCancel {
    /// As the session's marker is: the driver stops a statement only once the server listed
    /// the marker on the session's connection (in `performance_schema`); with that off, it never
    /// is, which is the reason said first.
    fn server_cancel(&self, f: &ServerFacts) -> Support {
        match &f.detail {
            EngineFacts::MySql(my) | EngineFacts::MariaDb(my) => match &my.performance_schema {
                Fact::No(why) => Fact::No(why.clone()),
                Fact::Known(()) | Fact::Unknown(_) => my.marker.clone(),
            },
            EngineFacts::Postgres(_) => Fact::Unknown(Why::NotChecked),
        }
    }
}

/// MySQL built-ins a filter may call without asking, upper case: they read their arguments and
/// nothing else. Sorted.
pub const MYSQL_FILTER_CALLS: &[&str] = &[
    "ABS",
    "CAST",
    "CEIL",
    "CEILING",
    "CHARACTER_LENGTH",
    "CHAR_LENGTH",
    "COALESCE",
    "CONCAT",
    "CONCAT_WS",
    "CONVERT",
    "CONVERT_TZ",
    "CURDATE",
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "CURTIME",
    "DATE",
    "DATEDIFF",
    "DATE_ADD",
    "DATE_FORMAT",
    "DATE_SUB",
    "DAY",
    "DAYOFMONTH",
    "DAYOFWEEK",
    "EXTRACT",
    "FIND_IN_SET",
    "FLOOR",
    "GREATEST",
    "HEX",
    "HOUR",
    "IF",
    "IFNULL",
    "INSTR",
    "JSON_CONTAINS",
    "JSON_EXTRACT",
    "JSON_LENGTH",
    "JSON_TYPE",
    "JSON_UNQUOTE",
    "LCASE",
    "LEAST",
    "LEFT",
    "LENGTH",
    "LOCATE",
    "LOWER",
    "LPAD",
    "LTRIM",
    "MD5",
    "MID",
    "MINUTE",
    "MOD",
    "MONTH",
    "NOW",
    "NULLIF",
    "OCTET_LENGTH",
    "POSITION",
    "REPLACE",
    "REVERSE",
    "RIGHT",
    "ROUND",
    "RPAD",
    "RTRIM",
    "SECOND",
    "SIGN",
    "STR_TO_DATE",
    "SUBSTR",
    "SUBSTRING",
    "TIME",
    "TIMESTAMPDIFF",
    "TRIM",
    "TRUNCATE",
    "UCASE",
    "UNHEX",
    "UPPER",
    "UTC_TIMESTAMP",
    "YEAR",
];

/// MySQL built-ins a filter may not call: they wait, take or test user locks, read a file of the
/// server, or advance or set a sequence (MariaDB's `NEXTVAL`, `SETVAL`); or they read the
/// outcome of the statement before (`FOUND_ROWS`, `ROW_COUNT`), which for a table's statement
/// is one the app sent. Sorted.
pub const MYSQL_REFUSED_CALLS: &[&str] = &[
    "BENCHMARK",
    "FOUND_ROWS",
    "GET_LOCK",
    "IS_FREE_LOCK",
    "IS_USED_LOCK",
    "LOAD_FILE",
    "MASTER_POS_WAIT",
    "NEXTVAL",
    "RELEASE_ALL_LOCKS",
    "RELEASE_LOCK",
    "ROW_COUNT",
    "SETVAL",
    "SLEEP",
    "SOURCE_POS_WAIT",
    "WAIT_FOR_EXECUTED_GTID_SET",
    "WAIT_UNTIL_SQL_THREAD_AFTER_GTIDS",
];

/// The family's answers of [`RiskRules`](super::RiskRules) beside the classifier: the
/// environment's read-only functions are PostgreSQL's alone, no result of the family is checked
/// for its text style, and both engines' filters use the family's call lists.
pub(super) fn family_read_only_calls() -> Support {
    Fact::No(Why::NotOnEngine)
}

/// See [`family_read_only_calls`].
pub(super) fn family_filter_calls() -> FilterCalls {
    FilterCalls { allowed: MYSQL_FILTER_CALLS, refused: MYSQL_REFUSED_CALLS }
}
