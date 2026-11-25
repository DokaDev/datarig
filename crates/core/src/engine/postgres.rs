//! PostgreSQL's row of the registry.

use super::{
    CancelRules, CatalogueRules, CountInTx, DdlLock, DdlOfIndex, DdlRules, DdlToConsole, EditRules, EngineId,
    EngineSpec, ExportRules, Fact, FilterCalls, FilterShape, FilterSql, FilterSyntax, GroupParts, InputRules, Like,
    MonitorRules, NameMatch, NullSafe, OpenVerdict, PlanRules, Provider, READ_ONLY_NOT_PROVEN, ReadOnlyProof,
    ResultTables, RiskRules, Routes, ServerFacts, SessionRules, SortRules, Support, TextRules, Transport, TxAsk,
    TxNote, TxRefusal, TxRules, TxStatement, TxVerdict, ValueRules,
};
use crate::driver::{
    ArrayElement, ColumnMeta, EditFormat, ExplainFormat, Hierarchy, ReadIn, ServerMessage, SessionRole, SessionValues,
    ValueKind,
};
use crate::edit::input::{self, TypeName};
use crate::edit::{BitsForm, BytesForm, Invalid, TableColumn};
use crate::export::keyset::NotKeyset;
use crate::export::{self, Kind, Refused, Rule, SqlCells, SqlColumn};
use crate::monitor::action::{
    ActionKind, ActionRoute, ActionTarget, Ask, Denied, Guidance, NativeError, PG_INSUFFICIENT_PRIVILEGE, Via, Warning,
};
use crate::monitor::{MonitorFormat, Route, SessionRow, Visibility};
use crate::profile::dsn::Scheme;
use crate::query::{
    CaseMode, ColumnKind, Compares, GROUP_PREVIEW, LINE_BREAKS, NoEquality, Nulls, RenderError, Target, base_type, hex,
};
use crate::sql::dialect::{Dialect, DollarQuotes, Language, LiteralOpen};
use crate::sql::risk::mysql::Judged;
use crate::sql::risk::{Class, Classifier, TxStep};
use std::ops::RangeInclusive;
use std::time::Duration;

pub static SPEC: EngineSpec = EngineSpec {
    id: EngineId::Postgres,
    key: "postgres",
    aliases: &["postgresql", "pg"],
    family: &[EngineId::Postgres],
    url_schemes: &["postgres", "postgresql"],
    transport: Transport::Network { scheme: Scheme::Postgres, ssh: true },
    preset_key: "postgres",
    form_key: "postgres",
    hierarchy: Hierarchy::DatabaseSchema,
    text: &PgText,
    risk: &PgRisk,
    tx: &PgTx,
    session: &PgSession,
    cancel: &PgCancel,
    values: &PgValues,
    export: Fact::Known(&PgExport),
    edit: Fact::Known(&PgEdits),
    catalogue: &PgCatalogue,
    ddl: Fact::Known(&PgDdl),
    sort: &PgSort,
    filter: Fact::Known(&PgFilter),
    plan: Fact::Known(&PgPlan),
    monitor: Fact::Known(&PgMonitorRules),
};

/// The plan format PostgreSQL's servers give: `EXPLAIN (FORMAT JSON)`, on every version.
pub const PG_PLAN: ExplainFormat = ExplainFormat::PostgresJson;

/// Which plan format PostgreSQL's servers give.
struct PgPlan;

impl PlanRules for PgPlan {
    /// [`PG_PLAN`], whatever the server's version and however it is reached.
    fn format(&self, _: &ServerFacts) -> Fact<ExplainFormat> {
        Fact::Known(PG_PLAN)
    }
}

/// What a monitor may do to the other sessions of PostgreSQL's servers.
struct PgMonitorRules;

const PG_STANDARD: Guidance = Guidance { grants: &["pg_signal_backend"], service_users: &[] };

/// PostgreSQL's action routes: the signal functions on every provider, each naming its own grants
/// and service accounts.
static PG_ROUTES: Routes = Routes {
    standard: ActionRoute { flavour: Provider::Standard, fallback: None, guidance: PG_STANDARD },
    providers: &[
        ActionRoute {
            flavour: Provider::AmazonRds,
            fallback: None,
            guidance: Guidance { grants: &["rds_superuser", "pg_signal_backend"], service_users: &["rdsadmin"] },
        },
        ActionRoute {
            flavour: Provider::AmazonAurora,
            fallback: None,
            guidance: Guidance { grants: &["rds_superuser", "pg_signal_backend"], service_users: &["rdsadmin"] },
        },
        ActionRoute {
            flavour: Provider::GoogleCloudSql,
            fallback: None,
            guidance: Guidance {
                grants: &["cloudsqlsuperuser", "pg_signal_backend"],
                service_users: &["cloudsqladmin"],
            },
        },
        ActionRoute {
            flavour: Provider::GoogleAlloyDb,
            fallback: None,
            guidance: Guidance { grants: &["alloydbsuperuser", "pg_signal_backend"], service_users: &[] },
        },
        ActionRoute {
            flavour: Provider::AzureDatabase,
            fallback: None,
            guidance: Guidance { grants: &["azure_pg_admin", "pg_signal_backend"], service_users: &["azuresu"] },
        },
    ],
};

impl MonitorRules for PgMonitorRules {
    fn reads(&self, f: &MonitorFormat) -> bool {
        match f {
            MonitorFormat::Postgres(_) => true,
            MonitorFormat::MySql(_) => false,
        }
    }

    /// Never: a pooled connection is told by its route ([`MonitorRules::pooled`]).
    fn unconfirmed(&self, f: &MonitorFormat) -> bool {
        match f {
            MonitorFormat::Postgres(_) => false,
            MonitorFormat::MySql(_) => true,
        }
    }

    /// When its state and its start are shown (without `pg_read_all_stats`, other roles'
    /// sessions show neither).
    fn identifies(&self, f: &MonitorFormat, row: &SessionRow) -> bool {
        match f {
            MonitorFormat::Postgres(_) => row.visible != Visibility::Hidden && row.key.started.is_some(),
            MonitorFormat::MySql(_) => false,
        }
    }

    /// Yes: behind a pooler, a session's statement start is what tells its client apart.
    fn end_needs_start(&self) -> bool {
        true
    }

    /// When the connection's route was not verified direct.
    fn pooled(&self, f: &MonitorFormat) -> bool {
        match f {
            MonitorFormat::Postgres(pg) => pg.route != Route::Direct,
            MonitorFormat::MySql(_) => true,
        }
    }

    /// A line for a cancel; a dialog for an end, which warns behind a pooler.
    fn ask(&self, f: &MonitorFormat, _: &SessionRow, kind: ActionKind) -> Ask {
        let pooled = vec![Warning::PoolerSharedBackend];
        match (f, kind) {
            (MonitorFormat::Postgres(_), ActionKind::Cancel) => Ask::Line,
            (MonitorFormat::Postgres(_), ActionKind::Terminate) => {
                Ask::Modal { warnings: if self.pooled(f) { pooled } else { Vec::new() } }
            }
            (MonitorFormat::MySql(_), ActionKind::Cancel | ActionKind::Terminate) => Ask::Modal { warnings: pooled },
        }
    }

    fn routes(&self) -> &'static Routes {
        &PG_ROUTES
    }

    /// The missing privilege (`insufficient_privilege`): the provider's own session, a
    /// superuser's (when the account may signal other roles'), or the grants that allow it.
    fn denied(&self, f: &MonitorFormat, target: &ActionTarget, _: Via, e: NativeError) -> Option<Denied> {
        let pg = match f {
            MonitorFormat::Postgres(pg) => pg,
            MonitorFormat::MySql(_) => return None,
        };
        let sqlstate = match e {
            NativeError::Pg { sqlstate } => sqlstate,
            NativeError::MySql { .. } => return None,
        };
        if sqlstate != PG_INSUFFICIENT_PRIVILEGE {
            return None;
        }
        let guidance = self.routes().of(pg.flavour).guidance;
        let service = target.user.as_deref().is_some_and(|u| guidance.service_users.contains(&u));
        Some(if service {
            Denied::ServiceSession
        } else if pg.signal_backend || pg.superuser {
            // The account may signal other roles' sessions: what is left is a superuser's.
            Denied::SuperuserTarget
        } else {
            Denied::Grant(guidance.grants)
        })
    }
}

/// How PostgreSQL's text is read.
struct PgText;

impl TextRules for PgText {
    /// Always: `$$…$$` and `$tag$…$tag$` are string constants on every PostgreSQL.
    fn dollar_quotes(&self, _: &ServerFacts) -> DollarQuotes {
        DollarQuotes::Read
    }

    /// The character the message's position counts to (1-based, characters of the statement as
    /// sent); how far the error goes is not said.
    fn error_offset(&self, sql: &str, m: &ServerMessage) -> Option<(usize, Option<usize>)> {
        let at = ServerMessage::byte_offset(sql, m.position?)?;
        Some((at, None))
    }
}

/// How PostgreSQL's statements are judged.
struct PgRisk;

impl RiskRules for PgRisk {
    fn new_classifier(&self, d: Dialect, j: Judged) -> Classifier {
        match j {
            // PostgreSQL has no sibling engine: its sessions are judged on its own rules.
            Judged::Own | Judged::WithSibling => Classifier::new(Language::Sql(d)),
        }
    }

    /// Always: a read-only profile may call what its environment allows.
    fn read_only_calls(&self) -> Support {
        Fact::Known(())
    }

    /// A statement on the re-run allowlist.
    fn rerun_style(&self, sql: &str) -> Option<bool> {
        Some(crate::sql::risk::repeat::repeatable(sql).is_ok())
    }

    fn filter_calls(&self) -> FilterCalls {
        FilterCalls { allowed: PG_FILTER_CALLS, refused: PG_REFUSED_CALLS }
    }

    /// Never: no sibling engine runs it.
    fn refuses_m_bang(&self, _: Judged) -> bool {
        false
    }
}

/// How long the session may take to answer the `ROLLBACK` the app sends for an idle
/// transaction before the app closes it instead (PostgreSQL's rollback only marks the
/// transaction aborted: it answers at once).
pub const TX_ROLLBACK_ANSWER_WAIT: Duration = Duration::from_secs(10);

/// How PostgreSQL's statements stand to the transaction open on a session.
struct PgTx;

impl TxRules for PgTx {
    /// A statement that cannot run in a transaction block (`VACUUM`, `CREATE INDEX
    /// CONCURRENTLY`, …) is refused while one is open, and with manual commit and none open it
    /// asks (it runs on its own and cannot be rolled back). In a transaction (open, or the one
    /// manual commit opens for it) an enum value added is noted, and so is the lock a DDL
    /// statement holds until the transaction ends ([`ddl_lock`]). Nothing commits the open
    /// transaction by itself.
    fn statement(&self, s: &TxStatement) -> TxVerdict {
        let mut v = TxVerdict::default();
        let r = s.risk;
        if r.no_tx_block {
            if s.open {
                v.refused = Some(TxRefusal::NoTxBlock);
                return v;
            }
            if s.manual {
                v.ask = Some(TxAsk::OutsideTx);
            }
        }
        let in_tx = s.open || (s.manual && s.step == TxStep::Opens);
        if r.adds_enum_value && in_tx {
            v.notes.push(TxNote::EnumValue);
        }
        if r.class == Class::Ddl
            && in_tx
            && let Some(lock) = ddl_lock(s.sql)
        {
            v.notes.push(TxNote::DdlLock(lock));
        }
        v
    }

    fn rollback_wait(&self) -> Duration {
        TX_ROLLBACK_ANSWER_WAIT
    }

    /// Left out while a transaction is open: inside the user's, or a portal's, the count keeps
    /// to the transaction's own settings.
    fn count_in_tx(&self) -> CountInTx {
        CountInTx::LeftOut
    }

    /// Never: PostgreSQL's reads take no row locks.
    fn reads_lock_rows(&self) -> bool {
        false
    }
}

/// What PostgreSQL's sessions may be given.
struct PgSession;

impl SessionRules for PgSession {
    /// Offered on every version (`idle_in_transaction_session_timeout`, set for the
    /// transaction only): a server that has no such setting refuses it, and the write fails,
    /// which is the safe side.
    fn server_idle_limit(&self) -> Support {
        Fact::Known(())
    }

    /// Every transaction a PostgreSQL session opens is `READ ONLY` itself, so a session whose
    /// default the server did not make read-only (a pooler may drop the startup option) still
    /// opens, and the user is told. A monitor session asks nothing: each of its reads is read-only
    /// by itself. A session that proved nothing else opens on no other answer.
    fn read_only_open(&self, role: SessionRole, proof: ReadOnlyProof) -> OpenVerdict {
        match (role, proof) {
            (SessionRole::Query | SessionRole::Meta | SessionRole::Monitor, ReadOnlyProof::Confirmed) => {
                OpenVerdict::Open
            }
            (SessionRole::Query | SessionRole::Meta | SessionRole::Monitor, ReadOnlyProof::PerTransaction) => {
                OpenVerdict::OpenPerTransaction
            }
            (SessionRole::Monitor, ReadOnlyProof::NotAsked) => OpenVerdict::Open,
            (SessionRole::Query | SessionRole::Meta, ReadOnlyProof::NotAsked) => {
                OpenVerdict::Refused(READ_ONLY_NOT_PROVEN.to_string())
            }
            (SessionRole::Query | SessionRole::Meta | SessionRole::Monitor, ReadOnlyProof::Unconfirmed(detail)) => {
                OpenVerdict::Refused(detail)
            }
        }
    }
}

/// The lock PostgreSQL's DDL statement `sql` holds on what it changes, by its kind, as
/// PostgreSQL's documentation gives it (no catalog is read). Only forms known to take
/// `ACCESS EXCLUSIVE` say so: `ALTER TABLE` with one action of `ADD [COLUMN]`, `DROP [COLUMN]`,
/// `DROP CONSTRAINT`, `ALTER [COLUMN] … TYPE`/`SET DATA TYPE`/`SET NOT NULL`/`DROP NOT NULL`,
/// `RENAME`, `OWNER TO`, `SET TABLESPACE`, `SET LOGGED`/`UNLOGGED`, `DETACH PARTITION` (not
/// `CONCURRENTLY`); `DROP TABLE`, `VIEW`, `MATERIALIZED VIEW`, `SEQUENCE`; `TRUNCATE`.
/// `DROP INDEX` locks its table so; `CREATE INDEX` takes `SHARE`. Other `ALTER` forms keep some
/// lock ([`DdlLock::Kept`]); anything else (a `CREATE` of something new, `… ALL IN
/// TABLESPACE`, a name not read) has no note.
pub(super) fn ddl_lock(sql: &str) -> Option<DdlLock> {
    use crate::sql::lexer::Tok;
    let toks: Vec<_> =
        crate::sql::lexer::lex_in(sql, Dialect::Postgres).into_iter().filter(|t| !t.is_trivia()).collect();
    let up = |i: usize| toks.get(i).filter(|t| t.is_word()).map(|t| t.text(sql).to_ascii_uppercase());
    let is = |i: usize, w: &str| up(i).as_deref() == Some(w);
    // The name at `k` (past `IF EXISTS`, `ONLY`): the words and dots that touch; where it ends.
    let name_at = |mut k: usize| -> Option<(String, usize)> {
        while up(k).is_some_and(|w| ["IF", "EXISTS", "ONLY"].contains(&w.as_str())) {
            k += 1;
        }
        let mut name = String::new();
        let mut end = None;
        let mut next = k;
        for t in toks.iter().skip(k) {
            let part = t.is_word() || matches!(t.kind, Tok::QuotedIdent | Tok::Dot);
            if !part || end.is_some_and(|e| e != t.start) {
                break;
            }
            name.push_str(t.text(sql));
            end = Some(t.end);
            next += 1;
        }
        (!name.is_empty() && name != "ALL" && !name.eq_ignore_ascii_case("all")).then_some((name, next))
    };
    // One action: no comma outside parentheses (several actions take the strongest of their
    // locks; `numeric(10,2)`, `DEFAULT f(a, b)` are one).
    let mut depth = 0i32;
    let one_action = !toks.iter().any(|t| {
        match t.kind {
            Tok::LParen => depth += 1,
            Tok::RParen => depth -= 1,
            _ => {}
        }
        t.kind == Tok::Comma && depth == 0
    });
    match (up(0)?.as_str(), up(1).as_deref()) {
        ("CREATE", Some("INDEX" | "UNIQUE")) => {
            if toks.iter().any(|t| t.is_word() && t.text(sql).eq_ignore_ascii_case("CONCURRENTLY")) {
                return None;
            }
            let on = toks.iter().position(|t| t.is_word() && t.text(sql).eq_ignore_ascii_case("ON"))?;
            name_at(on + 1).map(|(n, _)| DdlLock::Share(n))
        }
        ("DROP", Some("INDEX")) => name_at(2).map(|(n, _)| DdlLock::DropIndex(n)),
        ("DROP", Some("TABLE" | "VIEW" | "SEQUENCE")) => name_at(2).map(|(n, _)| DdlLock::Exclusive(n)),
        ("DROP", Some("MATERIALIZED")) if is(2, "VIEW") => name_at(3).map(|(n, _)| DdlLock::Exclusive(n)),
        ("TRUNCATE", _) => name_at(if is(1, "TABLE") { 2 } else { 1 }).map(|(n, _)| DdlLock::Exclusive(n)),
        ("ALTER", Some("TABLE")) => {
            let (name, a) = name_at(2)?;
            let exclusive = one_action
                && match up(a).as_deref() {
                    Some("ADD") => !matches!(
                        up(a + 1).as_deref(),
                        Some("CONSTRAINT" | "PRIMARY" | "UNIQUE" | "FOREIGN" | "CHECK" | "EXCLUDE")
                    ),
                    Some("DROP") => true,
                    Some("ALTER") => {
                        let c = if is(a + 1, "COLUMN") { a + 3 } else { a + 2 };
                        is(c, "TYPE")
                            || (is(c, "SET") && (is(c + 1, "DATA") || is(c + 1, "NOT")))
                            || (is(c, "DROP") && is(c + 1, "NOT"))
                    }
                    Some("RENAME" | "OWNER") => true,
                    Some("SET") => matches!(up(a + 1).as_deref(), Some("TABLESPACE" | "LOGGED" | "UNLOGGED")),
                    Some("DETACH") => !toks.iter().any(|t| {
                        t.is_word() && ["CONCURRENTLY", "FINALIZE"].contains(&t.text(sql).to_ascii_uppercase().as_str())
                    }),
                    _ => false,
                };
            Some(if exclusive { DdlLock::Exclusive(name) } else { DdlLock::Kept(name) })
        }
        ("ALTER", Some("INDEX" | "VIEW" | "SEQUENCE")) => name_at(2).map(|(n, _)| DdlLock::Kept(n)),
        ("ALTER", Some("MATERIALIZED")) if is(2, "VIEW") => name_at(3).map(|(n, _)| DdlLock::Kept(n)),
        _ => None,
    }
}

/// How PostgreSQL stops a statement.
struct PgCancel;

impl CancelRules for PgCancel {
    /// Always: a cancel request on a connection of its own, by the session's key.
    fn server_cancel(&self, _: &ServerFacts) -> Support {
        Fact::Known(())
    }
}

/// PostgreSQL built-ins a filter may call without asking, by name: they read their arguments
/// and nothing else. Sorted.
pub const PG_FILTER_CALLS: &[&str] = &[
    "abs",
    "age",
    "array_length",
    "btrim",
    "cardinality",
    "ceil",
    "ceiling",
    "char_length",
    "character_length",
    "concat",
    "concat_ws",
    "date_part",
    "date_trunc",
    "extract",
    "floor",
    "initcap",
    "isfinite",
    "json_array_length",
    "json_extract_path",
    "json_extract_path_text",
    "json_typeof",
    "jsonb_array_length",
    "jsonb_extract_path",
    "jsonb_extract_path_text",
    "jsonb_typeof",
    "left",
    "length",
    "like_escape",
    "lower",
    "lpad",
    "ltrim",
    "make_date",
    "md5",
    "mod",
    "now",
    "octet_length",
    "overlay",
    "position",
    "regexp_replace",
    "replace",
    "reverse",
    "right",
    "round",
    "rpad",
    "rtrim",
    "sign",
    "split_part",
    "starts_with",
    "strpos",
    "substr",
    "substring",
    "timezone",
    "to_char",
    "to_date",
    "to_number",
    "to_timestamp",
    "trunc",
    "upper",
];

/// PostgreSQL built-ins a filter may not call, by name however qualified: they wait, take
/// locks, change the session or a sequence, notify, reach another server, or read, list or write
/// the server's files or its large objects (a filter is one boolean expression with no effect).
/// Sorted.
pub const PG_REFUSED_CALLS: &[&str] = &[
    "dblink",
    "dblink_connect",
    "dblink_connect_u",
    "dblink_exec",
    "dblink_send_query",
    "lo_close",
    "lo_creat",
    "lo_create",
    "lo_export",
    "lo_from_bytea",
    "lo_get",
    "lo_import",
    "lo_lseek",
    "lo_lseek64",
    "lo_open",
    "lo_put",
    "lo_tell",
    "lo_tell64",
    "lo_truncate",
    "lo_truncate64",
    "lo_unlink",
    "loread",
    "lowrite",
    "nextval",
    "pg_advisory_lock",
    "pg_advisory_lock_shared",
    "pg_advisory_unlock",
    "pg_advisory_unlock_all",
    "pg_advisory_unlock_shared",
    "pg_advisory_xact_lock",
    "pg_advisory_xact_lock_shared",
    "pg_ls_archive_statusdir",
    "pg_ls_dir",
    "pg_ls_logdir",
    "pg_ls_logicalmapdir",
    "pg_ls_logicalsnapdir",
    "pg_ls_replslotdir",
    "pg_ls_summariesdir",
    "pg_ls_tmpdir",
    "pg_ls_waldir",
    "pg_notify",
    "pg_read_binary_file",
    "pg_read_file",
    "pg_sleep",
    "pg_sleep_for",
    "pg_sleep_until",
    "pg_stat_file",
    "pg_try_advisory_lock",
    "pg_try_advisory_lock_shared",
    "pg_try_advisory_xact_lock",
    "pg_try_advisory_xact_lock_shared",
    "set_config",
    "setseed",
    "setval",
];

/// How PostgreSQL's values are typed and shown.
struct PgValues;

impl ValueRules for PgValues {
    fn input(&self) -> &'static dyn InputRules {
        &PgInput
    }

    /// Never: a `timestamp with time zone` is shown with its offset.
    fn shown_in_session_zone(&self, _: ValueKind) -> bool {
        false
    }

    /// `\x`.
    fn bytes_shown_prefix(&self) -> &'static str {
        "\\x"
    }

    /// `\x` and lower-case digits.
    fn bytes_shown(&self, b: &[u8]) -> String {
        format!("\\x{}", b.iter().map(|b| format!("{b:02x}")).collect::<String>())
    }

    /// Any other value as a string literal, which PostgreSQL reads in the column's type: bytes
    /// (`\x…`) and bits too.
    fn literal(&self, kind: Kind, v: Option<&str>) -> String {
        match v {
            None => "NULL".to_string(),
            Some(v) if export::bare(kind, v) => v.to_string(),
            Some(v) => Dialect::Postgres.quote_literal(v),
        }
    }

    /// Every value as it is.
    fn sql_cells(&self, columns: &[SqlColumn], _: &ReadIn, _: Option<&str>) -> Result<SqlCells, Refused> {
        let names = columns.iter().map(|c| c.name.to_string()).collect();
        Ok(SqlCells { names, rules: vec![Rule::Plain; columns.len()] })
    }

    /// `OVERRIDING SYSTEM VALUE`: the server refuses a value for a `GENERATED ALWAYS` identity
    /// without it.
    fn insert_overriding(&self, overriding: bool) -> &'static str {
        if overriding { " OVERRIDING SYSTEM VALUE" } else { "" }
    }

    /// On every version (a `timestamp with time zone` is written as shown, with its offset).
    fn zoned_literal(&self, _: &ServerFacts) -> Support {
        Fact::Known(())
    }
}

/// Which edits of a result's rows PostgreSQL's servers take.
struct PgEdits;

impl EditRules for PgEdits {
    /// PostgreSQL's, on every version.
    fn writer(&self, _: &ServerFacts) -> Fact<EditFormat> {
        Fact::Known(EditFormat::Postgres)
    }

    /// Always: the format is the same on every version, and the structure is read from the
    /// catalogue's own columns (a column grant never hides one) or not at all.
    fn duplicates_choice(&self, _: Option<&ServerFacts>, _: Option<&ServerFacts>, _: EditFormat) -> Support {
        Fact::Known(())
    }
}

/// How PostgreSQL's catalogue names what a result reads.
struct PgCatalogue;

impl CatalogueRules for PgCatalogue {
    /// By the table's number (a result column's table OID).
    fn result_tables(&self) -> ResultTables {
        ResultTables::ByNumber
    }

    /// A type by its short name where it has one (`timestamptz`, `varchar(20)`, `float8`), so a
    /// cut does not take what tells types apart; anything else as written.
    fn short_type(&self, ty: &str) -> String {
        let base = ty.trim_end_matches("[]");
        let arrays = &ty[base.len()..];
        for head in ["timestamp", "time"] {
            let Some(rest) = base.strip_prefix(head) else { continue };
            let (precision, zone) = rest.split_at(rest.find(' ').unwrap_or(rest.len()));
            if !(precision.is_empty() || precision.starts_with('(')) {
                continue;
            }
            match zone {
                " with time zone" => return format!("{head}tz{precision}{arrays}"),
                " without time zone" => return format!("{head}{precision}{arrays}"),
                _ => {}
            }
        }
        for (long, short) in [
            ("character varying", "varchar"),
            ("character", "char"),
            ("double precision", "float8"),
            ("bit varying", "varbit"),
        ] {
            if let Some(rest) = base.strip_prefix(long)
                && (rest.is_empty() || rest.starts_with('('))
            {
                return format!("{short}{rest}{arrays}");
            }
        }
        ty.to_string()
    }
}

/// How the DDL of PostgreSQL's objects is shown and opened in a console.
struct PgDdl;

impl DdlRules for PgDdl {
    /// Its own: `CREATE INDEX`, or the constraint it backs.
    fn index_ddl(&self) -> DdlOfIndex {
        DdlOfIndex::Own
    }

    /// As read: the text is rebuilt from the catalogue's parts.
    fn to_console(&self) -> DdlToConsole {
        DdlToConsole::AsRead
    }
}

/// How PostgreSQL's tables are read for an export.
struct PgExport;

impl ExportRules for PgExport {
    /// Whole and decimal numbers (not `money`), text, dates and times (with a time zone too),
    /// booleans, bytes and `uuid`.
    fn key_kind(&self, c: &ColumnMeta, _: bool) -> Result<Kind, NotKeyset> {
        match c.kind {
            ValueKind::Integer => Ok(Kind::Number),
            ValueKind::Decimal if c.type_name != "money" => Ok(Kind::Number),
            ValueKind::Text | ValueKind::Date | ValueKind::Timestamp | ValueKind::TimestampTz => Ok(Kind::Text),
            ValueKind::Bool => Ok(Kind::Bool),
            ValueKind::Bytes => Ok(Kind::Bytes),
            ValueKind::Other if c.type_name == "uuid" => Ok(Kind::Text),
            _ => Err(NotKeyset::Kind(c.name.clone())),
        }
    }

    /// The operator named with its schema (a `search_path` of the user's may list its own `>`
    /// first); a row comparison for several columns, which the btree of the key reads from on.
    fn after(&self, names: &[String], values: &[String], _: &[Kind]) -> Option<String> {
        Some(match (names, values) {
            ([n], [v]) => format!("{n} OPERATOR(pg_catalog.>) {v}"),
            _ => format!("({}) OPERATOR(pg_catalog.>) ({})", names.join(", "), values.join(", ")),
        })
    }

    /// Always.
    fn text_read_whole(&self, _: Option<&SessionValues>) -> bool {
        true
    }

    /// Never.
    fn drops_slow_readers(&self) -> bool {
        false
    }
}

/// PostgreSQL's checks of a typed value.
struct PgInput;

/// The digits of a year PostgreSQL shows (past 9999 too).
const PG_YEAR_DIGITS: RangeInclusive<usize> = 4..=7;

/// The digits of a second PostgreSQL keeps when the type says none.
const PG_FRACTION_DIGITS: u32 = 6;

impl InputRules for PgInput {
    fn integer(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        let ty = TypeName::read(&c.column.type_name);
        let digits = input::whole_digits(s)?;
        let range: Option<(i128, i128)> = match ty.name.as_str() {
            "smallint" | "int2" => Some((i16::MIN.into(), i16::MAX.into())),
            "integer" | "int" | "int4" => Some((i32::MIN.into(), i32::MAX.into())),
            "bigint" | "int8" => Some((i64::MIN.into(), i64::MAX.into())),
            "oid" => Some((0, u32::MAX.into())),
            _ => None,
        };
        input::whole_in(s, digits, range, &c.column.type_name)
    }

    /// `numeric` and `decimal`; `money` and types this code does not know are left to the
    /// server. A `numeric` shows `NaN` and, without a precision, `Infinity` and `-Infinity`.
    fn decimal(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        let ty = TypeName::read(&c.column.type_name);
        if !matches!(ty.name.as_str(), "numeric" | "decimal") {
            return Ok(());
        }
        if s == "NaN" || (ty.args.is_empty() && matches!(s, "Infinity" | "-Infinity")) {
            return Ok(());
        }
        let (_, int, frac) = input::plain_decimal(s).ok_or(Invalid::NotNumber)?;
        input::decimal_digits(&ty, int, frac)
    }

    /// Also `NaN`, `Infinity` and `-Infinity`. A `float(p)` is `real` up to 24 bits; a type
    /// this code does not know is taken as the narrower one.
    fn float(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        if matches!(s, "NaN" | "Infinity" | "-Infinity") {
            return Ok(());
        }
        let ty = TypeName::read(&c.column.type_name);
        let type_name = &c.column.type_name;
        input::float_form(s)?;
        let single = match (ty.name.as_str(), &ty.args[..]) {
            ("float", [p]) => *p <= 24,
            ("float" | "double precision" | "float8", _) => false,
            _ => true,
        };
        let (_, stored) = input::float_read(s, single, type_name)?;
        let typed = input::digits_of(s);
        let rounded = |stored: String| Invalid::Rounded { type_name: type_name.to_string(), stored };
        if typed.is_none() || typed != input::digits_of(&stored) {
            return Err(rounded(input::plain_float(&stored)));
        }
        Ok(())
    }

    /// Also `infinity`, `-infinity` and years `BC` (and past 9999); no year 0.
    fn date(&self, s: &str) -> Result<(), Invalid> {
        if matches!(s, "infinity" | "-infinity") {
            return Ok(());
        }
        let (s, bc) = s.strip_suffix(" BC").map_or((s, false), |d| (d, true));
        input::ymd(s, bc, PG_YEAR_DIGITS, false).then_some(()).ok_or(Invalid::NotDate)
    }

    /// A `timestamp with time zone` with the offset it is shown in, a year `BC` after it,
    /// `infinity` and `-infinity`.
    fn timestamp(&self, c: &TableColumn, zoned: bool, s: &str) -> Result<(), Invalid> {
        if matches!(s, "infinity" | "-infinity") {
            return Ok(());
        }
        let ty = TypeName::read(&c.column.type_name);
        let (s, bc) = s.strip_suffix(" BC").map_or((s, false), |t| (t, true));
        let (date, rest) = s.split_once(' ').ok_or(Invalid::NotTimestamp)?;
        let (time, offset) = input::split_offset(rest);
        let offset_ok = match offset {
            Some(o) => zoned && input::utc_offset(o),
            None => !zoned,
        };
        if !offset_ok || !input::ymd(date, bc, PG_YEAR_DIGITS, false) {
            return Err(Invalid::NotTimestamp);
        }
        input::hms(time, input::fraction_digits(&ty, PG_FRACTION_DIGITS), true).ok_or(Invalid::NotTimestamp)?
    }

    /// `\x` and hexadecimal digits in pairs, either case.
    fn bytes(&self, s: &str) -> Result<(), Invalid> {
        let hex = s.strip_prefix("\\x").filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()));
        hex.is_some_and(|h| h.len() % 2 == 0).then_some(()).ok_or(Invalid::NotBytes { form: BytesForm::BackslashX })
    }

    /// Binary digits, exactly `n` for `bit(n)` and at most `n` for `bit varying(n)`.
    fn bits(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        let ty = TypeName::read(&c.column.type_name);
        if !input::binary(s) {
            return Err(Invalid::NotBits { form: BitsForm::Digits });
        }
        let exact = ty.name == "bit";
        match ty.args[..] {
            [n] if (exact && s.len() != n as usize) || s.len() > n as usize => {
                Err(Invalid::BitCount { count: n, exact })
            }
            _ => Ok(()),
        }
    }

    fn text(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        input::text_length(&TypeName::read(&c.column.type_name), s)
    }

    /// A `uuid`: `8-4-4-4-12` hexadecimal digits. Other types are left to the server.
    fn other(&self, c: &TableColumn, s: &str) -> Result<(), Invalid> {
        if TypeName::read(&c.column.type_name).name != "uuid" {
            return Ok(());
        }
        input::uuid(s).then_some(()).ok_or(Invalid::NotUuid)
    }

    /// Left to the server.
    fn interval(&self, _: &TableColumn, _: &str) -> Result<(), Invalid> {
        Ok(())
    }
}

/// How PostgreSQL orders and pages a table's rows.
struct PgSort;

impl SortRules for PgSort {
    /// No: NULL sorts as larger than any value.
    fn nulls_low(&self) -> bool {
        false
    }

    fn nulls_key(&self, col: &str, dir: &str, n: Nulls) -> String {
        match n {
            Nulls::First => format!("{col} {dir} NULLS FIRST"),
            Nulls::Last => format!("{col} {dir} NULLS LAST"),
        }
    }

    fn typed_nulls(&self) -> bool {
        true
    }

    /// A bare name is the column of the name it folds to.
    fn names(&self) -> NameMatch {
        NameMatch::Exact
    }

    fn offset_alone(&self, offset: u64) -> String {
        format!(" OFFSET {offset}")
    }

    /// Quoted only when a bare name would not read back as the same column.
    fn name(&self, d: Dialect, name: &str) -> String {
        d.quote_ident(name)
    }

    /// No: a name the reader takes bare stays the column's, as it always has.
    fn bare_call(&self, _: &str) -> bool {
        false
    }
}

/// How PostgreSQL's filter text is written.
struct PgFilter;

static PG_FILTER_SYNTAX: FilterSyntax = FilterSyntax {
    numbers_as_doubles: false,
    zoned_instants: false,
    null_safe: NullSafe::DistinctFrom,
    ilike: true,
    hex_strings: false,
    pg_strings: true,
    loose_ops: false,
    not_caution: false,
    interval_and: false,
    plans_subqueries: false,
    m_bang: false,
    shape: FilterShape::ParseTree,
    ci_collations: false,
};

impl FilterSql for PgFilter {
    fn syntax(&self) -> &'static FilterSyntax {
        &PG_FILTER_SYNTAX
    }

    /// `'…'`, or an escape string (`E'…'`) when it holds a backslash or a line break: it reads
    /// the same whatever `standard_conforming_strings` is, and a line break is written as its
    /// escape.
    fn text_literal(&self, t: &Target, s: &str) -> String {
        let d = t.dialect;
        if s.contains('\\') || s.contains(LINE_BREAKS) {
            let open = LiteralOpen { prefix: 1, quote: '\'', backslash: true };
            // The backslashes are doubled first: the escapes written after them stay escapes.
            let body = d.literal_body(s, open).replace('\n', "\\n").replace('\r', "\\r");
            format!("E'{body}'")
        } else {
            d.quote_literal(s)
        }
    }

    /// `'\x…'::pg_catalog.bytea`, the type named with its schema so that a domain of that name
    /// earlier on the search path is not the one cast to.
    fn bytes_literal(&self, t: &Target, b: &[u8]) -> String {
        format!("{}::pg_catalog.bytea", self.text_literal(t, &format!("\\x{}", hex(b, false))))
    }

    fn null_safe(&self, col: &str, lit: &str, negated: bool) -> String {
        match negated {
            false => format!("{col} IS NOT DISTINCT FROM {lit}"),
            true => format!("{col} IS DISTINCT FROM {lit}"),
        }
    }

    /// `ILIKE` without case, only on a column whose collation is known to be deterministic (the
    /// server refuses it on a nondeterministic one); `LIKE` on such a column, or on a server
    /// known to take it on any collation (18 and later; before, it refuses a nondeterministic
    /// one).
    fn like(&self, t: &Target, c: &Like<'_>) -> Result<String, RenderError> {
        let d = t.dialect;
        let p = self.text_literal(t, c.pattern);
        let e = d.quote_literal(&c.escape.to_string());
        let col = d.quote_ident(c.col);
        let not = if c.negated { "NOT " } else { "" };
        let deterministic = c.facts.and_then(|f| f.deterministic);
        match c.case {
            CaseMode::Server => {
                match (deterministic, t.like_nondeterministic) {
                    (Some(true), _) | (_, Some(true)) => {}
                    (Some(false), Some(false)) => return Err(RenderError::LikeRefused(c.col.to_string())),
                    _ => return Err(RenderError::LikeUnknown(c.col.to_string())),
                }
                Ok(format!("{col} {not}LIKE {p} ESCAPE {e}"))
            }
            CaseMode::Insensitive => {
                match deterministic {
                    Some(true) => {}
                    Some(false) => return Err(RenderError::CaseInsensitiveRefused(c.col.to_string())),
                    None => return Err(RenderError::CollationUnknown(c.col.to_string())),
                }
                Ok(format!("{col} {not}ILIKE {p} ESCAPE {e}"))
            }
        }
    }

    /// `json`, `xml`, `point` and the other geometric types, and arrays of them, have no
    /// equality a filter's literal reaches; `jsonb` and arrays of a type known to have one
    /// compare for equality only.
    fn compares(&self, m: &ColumnMeta, enum_known: bool) -> Result<Compares, NoEquality> {
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
            | ValueKind::Interval
            | ValueKind::Bytes
            | ValueKind::Bit => Ok(Compares::Order),
            // `jsonb` has equality; `json` has none.
            ValueKind::Json if ty == "jsonb" => Ok(Compares::Equality),
            ValueKind::Json => Err(NoEquality),
            // An array compares through its element type's equality: only one known to have it.
            ValueKind::Array(ArrayElement::Number | ArrayElement::Bool | ArrayElement::Text)
                if ty.strip_suffix("[]").is_some_and(|e| PG_ELEMENT_EQ.contains(&e)) =>
            {
                Ok(Compares::Equality)
            }
            ValueKind::Array(_) => Err(NoEquality),
            ValueKind::Other if enum_known || PG_ORDERED.contains(&ty.as_str()) => Ok(Compares::Order),
            ValueKind::Other => Err(NoEquality),
        }
    }

    /// The value only when its length is at most [`GROUP_PREVIEW`] bytes, for a type that may
    /// hold a long one; every function and operator named with `pg_catalog`, so that none of
    /// them depends on the session's search path.
    fn group_parts(&self, column: &ColumnMeta, quoted: &str) -> GroupParts {
        let c = quoted;
        let (preview, length) = match group_length(column, c) {
            None => (c.to_string(), "NULL".to_string()),
            Some(len) => (format!("CASE WHEN {len} OPERATOR(pg_catalog.<=) {GROUP_PREVIEW} THEN {c} END"), len),
        };
        GroupParts { preview, length, count: "pg_catalog.count(*)" }
    }

    /// Never: text is written as `'…'` or `E'…'`, read as written in any session.
    fn plain_text(&self, _: Option<&SessionValues>) -> bool {
        false
    }

    fn keywords(&self) -> &'static [&'static str] {
        PG_CONDITION_KEYWORDS
    }

    /// None: every column of a table is in a result of `SELECT *`.
    fn catalogue_meta(&self, _: &str, _: &str) -> Option<ColumnMeta> {
        None
    }

    /// Never: a `timestamp with time zone` is shown with its offset.
    fn catalogue_zoned(&self, _: &str) -> bool {
        false
    }

    /// By the name `format_type` gives; a domain, an array, dates and a type not named here are
    /// [`ColumnKind::Other`].
    fn catalogue_kind(&self, type_name: &str) -> ColumnKind {
        let base = base_type(type_name);
        if base.ends_with("[]") {
            return ColumnKind::Other;
        }
        match base.as_str() {
            "real" | "float4" => ColumnKind::Real,
            "smallint" | "integer" | "int" | "bigint" | "int2" | "int4" | "int8" | "oid" | "double precision"
            | "float8" | "numeric" | "decimal" => ColumnKind::Number,
            "text" | "character varying" | "varchar" | "character" | "char" | "bpchar" | "name" => ColumnKind::Text,
            "bytea" => ColumnKind::Bytes,
            _ => ColumnKind::Other,
        }
    }
}

/// The keywords a condition is written with on PostgreSQL, sorted ([`FilterSql::keywords`]).
const PG_CONDITION_KEYWORDS: &[&str] = &[
    "ALL",
    "AND",
    "ANY",
    "ARRAY",
    "AS",
    "BETWEEN",
    "CASE",
    "CAST",
    "COALESCE",
    "COLLATE",
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "DISTINCT",
    "ELSE",
    "END",
    "ESCAPE",
    "EXISTS",
    "EXTRACT",
    "FALSE",
    "FROM",
    "ILIKE",
    "IN",
    "INTERVAL",
    "IS",
    "LIKE",
    "NOT",
    "NULL",
    "NULLIF",
    "OR",
    "SIMILAR",
    "SOME",
    "SYMMETRIC",
    "THEN",
    "TO",
    "TRUE",
    "UNKNOWN",
    "WHEN",
];

/// PostgreSQL types of no kind of their own whose values compare as their text reads back
/// (equality and order).
const PG_ORDERED: &[&str] = &["uuid", "inet", "cidr", "macaddr", "macaddr8", "citext"];

/// Built-in PostgreSQL element types (as an array's type names them, `text` of `text[]`) known
/// to have the equality an array's `=` compares its elements with. Any other (`xml`, `json`,
/// `point` and the other geometric types have none) is not compared.
const PG_ELEMENT_EQ: &[&str] = &[
    "int2",
    "int4",
    "int8",
    "float4",
    "float8",
    "numeric",
    "oid",
    "money",
    "text",
    "varchar",
    "bpchar",
    "char",
    "name",
    "bool",
    "date",
    "time",
    "timetz",
    "timestamp",
    "timestamptz",
    "interval",
    "uuid",
    "bytea",
    "inet",
    "cidr",
    "macaddr",
    "macaddr8",
    "bit",
    "varbit",
];

/// The byte length of value `c` (column `column` quoted) of a group count when its type may hold
/// a long one; `None` for a type whose values are short (a number, a date, …). Text and bytes
/// are measured as they are; JSON, arrays and `numeric` (a domain over it too) by their text,
/// written by the type's own output function (no cast, which could be one of the user's).
///
/// The column's type is the one its metadata says, which may be older than the table: the
/// driver checks the statement's columns as the server prepares it, and refuses a value of a
/// type that is not short sent without its length.
fn group_length(column: &ColumnMeta, c: &str) -> Option<String> {
    let ty = column.type_name.to_ascii_lowercase();
    match column.kind {
        ValueKind::Text | ValueKind::Bytes | ValueKind::Bit => Some(format!("pg_catalog.octet_length({c})")),
        ValueKind::Other if ty == "citext" => Some(format!("pg_catalog.octet_length({c})")),
        ValueKind::Json if ty == "jsonb" => {
            Some(format!("pg_catalog.octet_length(pg_catalog.textin(pg_catalog.jsonb_out({c})))"))
        }
        ValueKind::Array(_) => Some(format!("pg_catalog.octet_length(pg_catalog.textin(pg_catalog.array_out({c})))")),
        // `numeric` and a domain over it (by its own name): its text has no bound of its own.
        // `money` is short.
        ValueKind::Decimal if ty != "money" => {
            Some(format!("pg_catalog.octet_length(pg_catalog.textin(pg_catalog.numeric_out({c})))"))
        }
        _ => None,
    }
}
