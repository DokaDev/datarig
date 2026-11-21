//! PostgreSQL's row of the registry.

use super::{
    CountInTx, DdlLock, EngineId, EngineSpec, Fact, OpenVerdict, READ_ONLY_NOT_PROVEN, ReadOnlyProof, RiskRules,
    ServerFacts, SessionRules, Support, TextRules, Transport, TxAsk, TxNote, TxRefusal, TxRules, TxStatement,
    TxVerdict,
};
use crate::driver::{Hierarchy, SessionRole};
use crate::profile::dsn::Scheme;
use crate::sql::dialect::{Dialect, DollarQuotes, Language};
use crate::sql::risk::mysql::Judged;
use crate::sql::risk::{Class, Classifier, TxStep};
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
};

/// How PostgreSQL's text is read.
struct PgText;

impl TextRules for PgText {
    /// Always: `$$…$$` and `$tag$…$tag$` are string constants on every PostgreSQL.
    fn dollar_quotes(&self, _: &ServerFacts) -> DollarQuotes {
        DollarQuotes::Read
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
