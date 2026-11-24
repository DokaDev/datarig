//! What MySQL and MariaDB servers say of themselves, read the same way wherever it is read, and
//! the rules the two engines share.

use super::{
    CancelRules, CatalogueRules, CountInTx, DdlOfIndex, DdlRules, DdlToConsole, EngineFacts, EngineId, ExportRules,
    Fact, FilterCalls, FilterShape, FilterSql, FilterSyntax, GroupParts, InputRules, Like, NameMatch, NullSafe,
    OpenVerdict, READ_ONLY_NOT_PROVEN, ReadOnlyProof, ResultTables, ServerFacts, SessionRules, SortRules, Support,
    TextRules, TxAsk, TxNote, TxRefusal, TxRules, TxStatement, TxVerdict, ValueRules, Version, Why,
};
use crate::driver::{ColumnMeta, ReadIn, ServerMessage, SessionRole, SessionValues, ValueKind};
use crate::edit::input::{self, TypeName};
use crate::edit::{BitsForm, BytesForm, Invalid, TableColumn};
use crate::export::keyset::{NotKeyset, expanded};
use crate::export::{self, Kind, NotWhole, Refused, Rule, SqlCells, SqlColumn, fixed_offset};
use crate::query::{
    CaseMode, ColumnFacts, ColumnKind, Compares, GROUP_PREVIEW, LINE_BREAKS, NoEquality, Nulls, RenderError, Target,
    base_type, hex,
};
use crate::sql::dialect::{Dialect, DollarQuotes, Language};
use crate::sql::risk::mysql::Judged;
use crate::sql::risk::{Class, Classifier, Danger};
use std::ops::RangeInclusive;
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

    /// Where the text the server quotes from where it stopped reading (`… near 'x = = 1)' at
    /// line 1`) is, to its end, when it is found once in the statement: found again from the
    /// next character on (an overlapping match counts), it is not located.
    fn error_offset(&self, sql: &str, m: &ServerMessage) -> Option<(usize, Option<usize>)> {
        let near = mysql_near(&m.text)?;
        let at = sql.find(near)?;
        let next = at + sql[at..].chars().next().map_or(1, char::len_utf8);
        if sql[next..].contains(near) {
            return None;
        }
        Some((at, Some(at + near.len())))
    }
}

/// The text MySQL and MariaDB quote from where they stopped reading (`… near 'x = = 1)' at line
/// 1`), when they quote any.
pub(super) fn mysql_near(message: &str) -> Option<&str> {
    let from = message.find(" near '")? + " near '".len();
    let rest = &message[from..];
    let to = rest.rfind("' at line ")?;
    Some(&rest[..to]).filter(|s| !s.is_empty())
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

/// How MySQL's and MariaDB's values are typed and shown.
pub(super) struct FamilyValues {
    /// The first version of the engine whose servers read a time zone offset written after a
    /// `TIMESTAMP` value; `None` for an engine that reads none on any version.
    pub(super) offsets_since: Option<Version>,
}

impl ValueRules for FamilyValues {
    fn input(&self) -> &'static dyn InputRules {
        &FamilyInput
    }

    /// A `TIMESTAMP`: shown in the session's time zone, without an offset.
    fn shown_in_session_zone(&self, kind: ValueKind) -> bool {
        kind == ValueKind::TimestampTz
    }

    /// `0x`.
    fn bytes_shown_prefix(&self) -> &'static str {
        "0x"
    }

    /// `0x` and upper-case digits.
    fn bytes_shown(&self, b: &[u8]) -> String {
        format!("0x{}", b.iter().map(|b| format!("{b:02X}")).collect::<String>())
    }

    /// Bytes shown as `0x…` and bits as `b'…'` (every byte, every bit) as their hexadecimal and
    /// bit literals, which are the value itself (a string literal of that text would be the
    /// text); any other value as a string that reads the same whatever the session's
    /// `NO_BACKSLASH_ESCAPES`.
    fn literal(&self, kind: Kind, v: Option<&str>) -> String {
        match v {
            None => "NULL".to_string(),
            Some(v) if export::bare(kind, v) => v.to_string(),
            Some(v) if kind == Kind::Bytes && export::mysql_hex(v) => format!("X'{}'", &v[2..]),
            Some(v) if kind == Kind::Bit && export::mysql_bits(v) => v.to_string(),
            Some(v) => export::mysql_string(v),
        }
    }

    /// A `FLOAT` is refused (the server sends it rounded); text read in a character set that is
    /// not UTF-8 is refused; a `TIMESTAMP` is written with the offset of the time zone it was read
    /// in when that is fixed ([`fixed_offset`]), and refused otherwise; a `TIMESTAMP` or a text
    /// whose values are not known, or changed between the result's pages, is refused; bytes and
    /// bits must be in the driver's form; an `AUTO_INCREMENT` value of 0 is refused (checked per
    /// value, [`SqlCells::cell`]).
    fn sql_cells(
        &self,
        columns: &[SqlColumn],
        read_in: &ReadIn,
        server_zone: Option<&str>,
    ) -> Result<SqlCells, Refused> {
        use crate::driver::ValueKind as V;
        let names = columns.iter().map(|c| c.name.to_string()).collect();
        let refused = |c: &SqlColumn, why| Refused { column: c.name.to_string(), why };
        // Only `utf8mb4` is every character (`utf8mb3` turns an emoji into `?`).
        let utf8 = |cs: &str| cs.eq_ignore_ascii_case("utf8mb4");
        let mut rules = Vec::with_capacity(columns.len());
        for c in columns {
            let rule = match c.kind {
                V::Float if c.type_name.starts_with("float") => return Err(refused(c, NotWhole::Float)),
                V::Bytes | V::Bit => Rule::Shown,
                V::TimestampTz => match read_in {
                    ReadIn::Values(s) => match fixed_offset(&s.time_zone, server_zone) {
                        Some(offset) => Rule::Timestamp(offset),
                        None => return Err(refused(c, NotWhole::TimeZone(s.time_zone.clone()))),
                    },
                    ReadIn::Unknown => return Err(refused(c, NotWhole::SessionUnknown)),
                    ReadIn::Changed => return Err(refused(c, NotWhole::SessionChanged)),
                },
                V::Text | V::Json | V::Other => match read_in {
                    ReadIn::Values(s) if !utf8(&s.charset) => Rule::Refuse(NotWhole::Charset(s.charset.clone())),
                    ReadIn::Unknown => Rule::Refuse(NotWhole::SessionUnknown),
                    ReadIn::Changed => Rule::Refuse(NotWhole::SessionChanged),
                    _ if c.auto_increment => Rule::AutoIncrement,
                    _ => Rule::Plain,
                },
                _ if c.auto_increment => Rule::AutoIncrement,
                _ => Rule::Plain,
            };
            rules.push(rule);
        }
        Ok(SqlCells { names, rules })
    }

    /// None: the family's identity (`AUTO_INCREMENT`) takes a written value as it is.
    fn insert_overriding(&self, _: bool) -> &'static str {
        ""
    }

    /// From [`FamilyValues::offsets_since`] on, on a server of the engine whose version is
    /// known; never on a server the facts name as another engine. A version that is not known
    /// is not taken as new enough.
    fn zoned_literal(&self, f: &ServerFacts) -> Support {
        let Some(since) = self.offsets_since else { return Fact::No(Why::NotOnEngine) };
        match (&f.detail, &f.version) {
            (EngineFacts::MySql(_), Fact::Known(v)) if *v >= since => Fact::Known(()),
            (EngineFacts::MySql(_), Fact::Known(_)) => Fact::No(Why::VersionTooOld { needed: since }),
            (EngineFacts::MySql(_), Fact::No(why) | Fact::Unknown(why)) => Fact::Unknown(why.clone()),
            (EngineFacts::MariaDb(_), _) => Fact::No(Why::NotOnEngine),
            (EngineFacts::Postgres(_), _) => Fact::Unknown(Why::EnginesDisagree),
        }
    }
}

/// How MySQL's and MariaDB's catalogue names what a result reads.
pub(super) struct FamilyCatalogue;

impl CatalogueRules for FamilyCatalogue {
    /// By name: a result says the name of its column's table, looked up in the keys read at
    /// connect.
    fn result_tables(&self) -> ResultTables {
        ResultTables::ByName
    }

    /// As written.
    fn short_type(&self, ty: &str) -> String {
        ty.to_string()
    }
}

/// How the DDL of MySQL's and MariaDB's objects is shown and opened in a console.
pub(super) struct FamilyDdl;

impl DdlRules for FamilyDdl {
    /// Its table's: an index has no DDL of its own, its table's `CREATE TABLE` defines it.
    fn index_ddl(&self) -> DdlOfIndex {
        DdlOfIndex::Table
    }

    /// Requoted: the text is the server's own (`SHOW CREATE …`), its strings written for the
    /// sql mode it was read in, so they are written again for the console's, in the object's
    /// database.
    fn to_console(&self) -> DdlToConsole {
        DdlToConsole::Requoted
    }
}

/// How MySQL's and MariaDB's tables are read for an export.
pub(super) struct FamilyExport;

impl ExportRules for FamilyExport {
    /// Whole and decimal numbers (not `money`), text read whole but not an `ENUM` or a `SET`
    /// (they sort by their members' numbers, not as their text compares), dates and times
    /// without a time zone (a `TIMESTAMP`'s hour a clock goes back is two instants), booleans and
    /// bytes.
    fn key_kind(&self, c: &ColumnMeta, text_ok: bool) -> Result<Kind, NotKeyset> {
        let refused = || NotKeyset::Kind(c.name.clone());
        match c.kind {
            ValueKind::Integer => Ok(Kind::Number),
            ValueKind::Decimal if c.type_name != "money" => Ok(Kind::Number),
            ValueKind::Text if c.type_name.eq_ignore_ascii_case("enum") || c.type_name.eq_ignore_ascii_case("set") => {
                Err(refused())
            }
            ValueKind::Text if text_ok => Ok(Kind::Text),
            ValueKind::Date | ValueKind::Timestamp => Ok(Kind::Text),
            ValueKind::Bool => Ok(Kind::Bool),
            ValueKind::Bytes => Ok(Kind::Bytes),
            _ => Err(refused()),
        }
    }

    /// The row comparison written out (the server does not read an index range for a row
    /// comparison); `None` when a bytes value was not shown as `0x…`, as it would be compared as
    /// its text.
    fn after(&self, names: &[String], values: &[String], kinds: &[Kind]) -> Option<String> {
        if kinds.iter().zip(values).any(|(k, v)| *k == Kind::Bytes && !v.starts_with("X'")) {
            return None;
        }
        Some(expanded(names, values))
    }

    /// Only in a session whose results are `utf8mb4`, the one character set that is every
    /// character.
    fn text_read_whole(&self, values: Option<&SessionValues>) -> bool {
        values.is_some_and(|v| v.charset.eq_ignore_ascii_case("utf8mb4"))
    }

    /// The server ends a session that does not read what it sends within `net_write_timeout`.
    fn drops_slow_readers(&self) -> bool {
        true
    }
}

/// MySQL's and MariaDB's checks of a typed value.
struct FamilyInput;

/// The digits of a year the family shows.
const FAMILY_YEAR_DIGITS: RangeInclusive<usize> = 4..=4;

/// The digits of a second the family keeps when the type says none.
const FAMILY_FRACTION_DIGITS: u32 = 0;

impl InputRules for FamilyInput {
    /// Integer widths and `UNSIGNED`; a `YEAR` is 1901 to 2155 or 0 (two-digit years and years
    /// before 1901 are read as other years).
    fn integer(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        let ty = TypeName::read(&c.column.type_name);
        let type_name = &c.column.type_name;
        let digits = input::whole_digits(s)?;
        let bits = match ty.name.as_str() {
            "tinyint" | "bool" | "boolean" => 8,
            "smallint" => 16,
            "mediumint" => 24,
            "int" | "integer" => 32,
            "bigint" => 64,
            "year" => {
                let year = (digits.len() <= 10).then(|| s.parse::<i64>().ok()).flatten();
                let fits = year.is_some_and(|y| y == 0 || (1901..=2155).contains(&y));
                return fits.then_some(()).ok_or_else(|| input::out_of_range(type_name));
            }
            _ => 0,
        };
        let range: Option<(i128, i128)> = (bits > 0)
            .then(|| if ty.unsigned { (0, (1 << bits) - 1) } else { (-(1 << (bits - 1)), (1 << (bits - 1)) - 1) });
        input::whole_in(s, digits, range, type_name)
    }

    /// `DECIMAL` and its other names; an unsigned one takes no negative number but zero.
    fn decimal(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        let ty = TypeName::read(&c.column.type_name);
        if !matches!(ty.name.as_str(), "decimal" | "numeric" | "dec" | "fixed") {
            return Ok(());
        }
        let (negative, int, frac) = input::plain_decimal(s).ok_or(Invalid::NotNumber)?;
        if ty.unsigned && negative && !(int.bytes().chain(frac.bytes()).all(|b| b == b'0')) {
            return Err(input::out_of_range(&c.column.type_name));
        }
        input::decimal_digits(&ty, int, frac)
    }

    /// `FLOAT(p)` is single precision up to 24 bits; a type this code does not know is taken as
    /// the narrower one. `FLOAT(M,D)` and `DOUBLE(M,D)` keep `D` digits after the point, and
    /// take the text the server shows for a value too (its digits there, of the binary value);
    /// an unsigned one no negative number.
    fn float(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        let ty = TypeName::read(&c.column.type_name);
        let type_name = &c.column.type_name;
        input::float_form(s)?;
        let single = match (ty.name.as_str(), &ty.args[..]) {
            ("float", [p]) => *p <= 24,
            ("double" | "double precision" | "real", _) => false,
            _ => true,
        };
        let (value, stored) = input::float_read(s, single, type_name)?;
        let typed = input::digits_of(s);
        if ty.unsigned && value < 0.0 {
            return Err(input::out_of_range(type_name));
        }
        let rounded = |stored: String| Invalid::Rounded { type_name: type_name.to_string(), stored };
        let Some((_, digits, exp)) = &typed else { return Err(rounded(input::plain_float(&stored))) };
        let exact = typed == input::digits_of(&stored);
        let &[m, d] = &ty.args[..] else {
            return if exact { Ok(()) } else { Err(rounded(input::plain_float(&stored))) };
        };
        // Shown with `D` digits after the point, as many as the binary value has there (0.1 in
        // a `DOUBLE(30,20)` is `0.10000000000000000555`): the text read back is taken when it
        // is what the server shows for the number it is read as.
        let shown = format!("{value:.*}", d as usize);
        let after = (digits.len() as i64 - 1 - exp).max(0);
        if after > i64::from(d) || !(exact || typed == input::digits_of(&shown)) {
            return Err(rounded(shown));
        }
        if (exp + 1).max(0) > i64::from(m.saturating_sub(d)) {
            return Err(input::out_of_range(type_name));
        }
        Ok(())
    }

    /// Also the zero date and a zero month or day, which the server takes or refuses by its
    /// mode.
    fn date(&self, s: &str) -> Result<(), Invalid> {
        input::ymd(s, false, FAMILY_YEAR_DIGITS, true).then_some(()).ok_or(Invalid::NotDate)
    }

    /// Without an offset, with or without a time zone: a `TIMESTAMP` is shown in the session's
    /// time zone.
    fn timestamp(&self, c: &TableColumn, _: bool, s: &str) -> Result<(), Invalid> {
        let ty = TypeName::read(&c.column.type_name);
        let (date, rest) = s.split_once(' ').ok_or(Invalid::NotTimestamp)?;
        let (time, offset) = input::split_offset(rest);
        if offset.is_some() || !input::ymd(date, false, FAMILY_YEAR_DIGITS, true) {
            return Err(Invalid::NotTimestamp);
        }
        input::hms(time, input::fraction_digits(&ty, FAMILY_FRACTION_DIGITS), false).ok_or(Invalid::NotTimestamp)?
    }

    /// `0x` and upper-case hexadecimal digits in pairs (other text would be written as a
    /// string of that text).
    fn bytes(&self, s: &str) -> Result<(), Invalid> {
        let hex = s.strip_prefix("0x").filter(|h| h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'A'..=b'F')));
        hex.is_some_and(|h| h.len() % 2 == 0).then_some(()).ok_or(Invalid::NotBytes { form: BytesForm::ZeroX })
    }

    /// `b'…'`, no more significant digits than `bit(n)` has.
    fn bits(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        let ty = TypeName::read(&c.column.type_name);
        let digits =
            s.strip_prefix("b'").and_then(|r| r.strip_suffix('\'')).filter(|d| !d.is_empty() && input::binary(d));
        let digits = digits.ok_or(Invalid::NotBits { form: BitsForm::BQuoted })?;
        match ty.args[..] {
            [n] if digits.trim_start_matches('0').len() > n as usize => {
                Err(Invalid::BitCount { count: n, exact: false })
            }
            _ => Ok(()),
        }
    }

    /// For `CHAR`, which drops trailing spaces when it reads a value back, without them.
    fn text(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        let ty = TypeName::read(&c.column.type_name);
        if input::fixed_text(&ty) && s.ends_with(' ') {
            return Err(Invalid::TrailingSpaces);
        }
        input::text_length(&ty, s)
    }

    /// Left to the server.
    fn other(&self, _: &TableColumn, _: &str) -> Result<(), Invalid> {
        Ok(())
    }

    /// A `TIME`, in the forms the server reads exactly.
    fn interval(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        input::mysql_time(&TypeName::read(&c.column.type_name), s)
    }
}

/// How MySQL and MariaDB order and page a table's rows.
pub(super) struct FamilySort;

impl SortRules for FamilySort {
    /// Yes: NULL sorts as smaller than any value.
    fn nulls_low(&self) -> bool {
        true
    }

    /// There is no `NULLS FIRST`/`LAST`: the key sorts on whether the value is NULL first.
    fn nulls_key(&self, col: &str, dir: &str, n: Nulls) -> String {
        match n {
            Nulls::First => format!("{col} IS NULL DESC, {col} {dir}"),
            Nulls::Last => format!("{col} IS NULL ASC, {col} {dir}"),
        }
    }

    /// No: the servers have no such syntax.
    fn typed_nulls(&self) -> bool {
        false
    }

    /// Column names compare without regard to case.
    fn names(&self) -> NameMatch {
        NameMatch::AnyCase
    }

    /// There is no `OFFSET` without a `LIMIT`: the largest one stands for none.
    fn offset_alone(&self, offset: u64) -> String {
        format!(" LIMIT {} OFFSET {offset}", u64::MAX)
    }
}

/// How MySQL's and MariaDB's filter text is written.
pub(super) struct FamilyFilter;

static FAMILY_FILTER_SYNTAX: FilterSyntax = FilterSyntax {
    numbers_as_doubles: true,
    zoned_instants: true,
    null_safe: NullSafe::Spaceship,
    ilike: false,
    hex_strings: true,
    pg_strings: false,
    loose_ops: true,
    not_caution: true,
    interval_and: true,
    plans_subqueries: true,
    m_bang: true,
    shape: FilterShape::Tokens,
    ci_collations: true,
};

impl FilterSql for FamilyFilter {
    fn syntax(&self) -> &'static FilterSyntax {
        &FAMILY_FILTER_SYNTAX
    }

    /// Plain only when the session's character set is known to be `utf8mb4` and it holds no
    /// backslash or NUL (which `NO_BACKSLASH_ESCAPES` reads otherwise) and no line break; else
    /// its UTF-8 bytes, `_utf8mb4 X'…'`. A value holding the characters `/*M!` (MariaDB's own
    /// executable comment) is written as its bytes too: a MariaDB session refuses text that
    /// holds them as typed.
    fn text_literal(&self, t: &Target, s: &str) -> String {
        if t.utf8mb4 && !s.contains(['\\', '\0']) && !s.contains(LINE_BREAKS) && !s.contains("/*M!") {
            t.dialect.quote_literal(s)
        } else {
            format!("_utf8mb4 X'{}'", hex(s.as_bytes(), true))
        }
    }

    fn bytes_literal(&self, _: &Target, b: &[u8]) -> String {
        format!("X'{}'", hex(b, true))
    }

    fn null_safe(&self, col: &str, lit: &str, negated: bool) -> String {
        match negated {
            false => format!("{col} <=> {lit}"),
            true => format!("NOT ({col} <=> {lit})"),
        }
    }

    /// Without case: `LIKE` on a column of a `_ci` collation, which compares without case already
    /// (and keeps an index usable), else `LOWER(…) LIKE LOWER(…)`.
    fn like(&self, t: &Target, c: &Like<'_>) -> Result<String, RenderError> {
        let d = t.dialect;
        let p = self.text_literal(t, c.pattern);
        let e = d.quote_literal(&c.escape.to_string());
        let col = d.quote_ident(c.col);
        let not = if c.negated { "NOT " } else { "" };
        Ok(match c.case {
            CaseMode::Server => format!("{col} {not}LIKE {p} ESCAPE {e}"),
            CaseMode::Insensitive if c.facts.is_some_and(case_insensitive) => {
                format!("{col} {not}LIKE {p} ESCAPE {e}")
            }
            CaseMode::Insensitive => format!("LOWER({col}) {not}LIKE LOWER({p}) ESCAPE {e}"),
        })
    }

    /// JSON (compared with a string as a JSON string), the spatial and vector types (which come
    /// as bytes but do not compare as such), bits (shown as `b'…'`, no string literal of them),
    /// arrays and the types of no kind of their own have no equality a filter's literal reaches.
    fn compares(&self, m: &ColumnMeta, _: bool) -> Result<Compares, NoEquality> {
        let ty = m.type_name.to_ascii_lowercase();
        match m.kind {
            ValueKind::Integer
            | ValueKind::Decimal
            | ValueKind::Float
            | ValueKind::Text
            | ValueKind::Bool
            | ValueKind::Date
            | ValueKind::Time
            | ValueKind::Timestamp
            | ValueKind::TimestampTz
            | ValueKind::Interval => Ok(Compares::Order),
            ValueKind::Bytes if ty == "geometry" || ty == "vector" => Err(NoEquality),
            ValueKind::Bytes => Ok(Compares::Order),
            ValueKind::Bit | ValueKind::Json | ValueKind::Array(_) | ValueKind::Other => Err(NoEquality),
        }
    }

    /// Every value measured, whatever its type is said to be (`OCTET_LENGTH` of a number or a
    /// date is that of its text, which the server sends): what is sent is bounded by the
    /// statement itself, and the value keeps its column's type. Bytes in every mode: `LENGTH`
    /// counts characters under MariaDB's `sql_mode=ORACLE`.
    fn group_parts(&self, _: &ColumnMeta, quoted: &str) -> GroupParts {
        let c = quoted;
        let len = format!("OCTET_LENGTH({c})");
        GroupParts { preview: format!("IF({len} <= {GROUP_PREVIEW}, {c}, NULL)"), length: len, count: "COUNT(*)" }
    }

    /// When the session's character set is known to be `utf8mb4`, which holds every character.
    fn plain_text(&self, values: Option<&SessionValues>) -> bool {
        values.is_some_and(|v| v.charset.eq_ignore_ascii_case("utf8mb4"))
    }

    fn keywords(&self) -> &'static [&'static str] {
        MYSQL_CONDITION_KEYWORDS
    }

    /// The catalogue type (`COLUMN_TYPE`) named and of the kind the MySQL driver gives a result
    /// column of that type: for a column no result describes (an `INVISIBLE` one, which `SELECT
    /// *` leaves out). A type not named here is of no kind of its own ([`ValueKind::Other`]),
    /// whose values are not compared.
    fn catalogue_meta(&self, name: &str, type_name: &str) -> Option<ColumnMeta> {
        let lower = type_name.to_ascii_lowercase();
        let base = base_type(type_name);
        let unsigned = lower.split_whitespace().any(|w| w == "unsigned");
        let numeric = |n: &str| if unsigned { format!("{n} unsigned") } else { n.to_string() };
        let (ty, kind) = match base.as_str() {
            "tinyint" if lower.starts_with("tinyint(1)") => (numeric("tinyint(1)"), ValueKind::Integer),
            "tinyint" | "smallint" | "mediumint" | "int" | "bigint" => (numeric(&base), ValueKind::Integer),
            "year" => (base.clone(), ValueKind::Integer),
            "decimal" => (numeric("decimal"), ValueKind::Decimal),
            "float" | "double" => (numeric(&base), ValueKind::Float),
            "bit" => (base.clone(), ValueKind::Bit),
            "date" => (base.clone(), ValueKind::Date),
            "time" => (base.clone(), ValueKind::Interval),
            "datetime" => (base.clone(), ValueKind::Timestamp),
            "timestamp" => (base.clone(), ValueKind::TimestampTz),
            "json" => (base.clone(), ValueKind::Json),
            "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" | "enum" | "set" => {
                (base.clone(), ValueKind::Text)
            }
            "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob" => {
                (base.clone(), ValueKind::Bytes)
            }
            "geometry" | "point" | "linestring" | "polygon" | "multipoint" | "multilinestring" | "multipolygon"
            | "geometrycollection" | "geomcollection" => ("geometry".to_string(), ValueKind::Bytes),
            "vector" => (base.clone(), ValueKind::Bytes),
            _ => (base.clone(), ValueKind::Other),
        };
        Some(ColumnMeta::new(name.to_string(), ty, kind, None))
    }

    /// A `TIMESTAMP`.
    fn catalogue_zoned(&self, type_name: &str) -> bool {
        base_type(type_name) == "timestamp"
    }

    /// By the catalogue type (`COLUMN_TYPE`); an array (no column of the engine has one), dates
    /// and a type not named here are [`ColumnKind::Other`].
    fn catalogue_kind(&self, type_name: &str) -> ColumnKind {
        let base = base_type(type_name);
        if base.ends_with("[]") {
            return ColumnKind::Other;
        }
        match base.as_str() {
            "float" => ColumnKind::Real,
            "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint" | "year" | "decimal" | "numeric"
            | "dec" | "fixed" | "double" | "double precision" | "real" => ColumnKind::Number,
            "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" | "enum" | "set" => ColumnKind::Text,
            "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob" => ColumnKind::Bytes,
            _ => ColumnKind::Other,
        }
    }
}

/// The keywords a condition is written with on MySQL and MariaDB, sorted
/// ([`FilterSql::keywords`]).
const MYSQL_CONDITION_KEYWORDS: &[&str] = &[
    "ALL",
    "AND",
    "ANY",
    "AS",
    "BETWEEN",
    "BINARY",
    "CASE",
    "CAST",
    "COLLATE",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "DIV",
    "ELSE",
    "END",
    "ESCAPE",
    "EXISTS",
    "FALSE",
    "FROM",
    "IN",
    "INTERVAL",
    "IS",
    "LIKE",
    "MOD",
    "NOT",
    "NULL",
    "OR",
    "REGEXP",
    "RLIKE",
    "SOME",
    "THEN",
    "TRUE",
    "UNKNOWN",
    "WHEN",
    "XOR",
];

/// A collation that compares without case (`utf8mb4_0900_ai_ci`).
fn case_insensitive(c: &ColumnFacts) -> bool {
    c.collation.as_deref().is_some_and(|n| n.to_ascii_lowercase().ends_with("_ci"))
}
