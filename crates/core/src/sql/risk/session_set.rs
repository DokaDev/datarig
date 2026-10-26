//! Whether a statement may be one of the settings a query session starts with (a session
//! preset's, a profile's own `session_sql`): exactly one statement that sets settings of the
//! session and nothing else, read by the dialect's classifier ([`super::Classifier::session_set`]).
//! Allowlist: PostgreSQL's `SET [SESSION] name = value | TO value | TO DEFAULT`, `SET TIME ZONE`
//! and `SET SESSION CHARACTERISTICS AS TRANSACTION …` (from its parse tree); MySQL's `SET
//! [SESSION | LOCAL] name = value`, `SET @@[session.]name = value`, `SET @user = value` (each a
//! list of them) and `SET SESSION TRANSACTION …` ([`super::mysql::session_set`]). Anything else
//! is refused with why ([`SetProblem`]): what only one transaction keeps (`SET LOCAL`, a plain
//! `SET TRANSACTION`), what acts on the server (`SET GLOBAL`), resets (`RESET`, `DISCARD`), a
//! call (`SELECT set_config(…)`), and the settings the app keeps itself ([`Owner`]).
//!
//! PostgreSQL behind a pooler in transaction mode keeps nothing of a session: there the same
//! setting goes first in each transaction as `SET LOCAL` ([`SessionSet::per_transaction`]),
//! made from the statement's own text by adding `LOCAL` after its `SET` (or in place of its
//! `SESSION`) and checked by parsing it again: the same setting, the same value, now for the
//! transaction only. A statement that has no such form ([`NotLocal`]) is not sent there.

use super::{Danger, Risk, STACK, THREAD, classify, node, parse, text};
use crate::sql::lexer::{Tok, lex};
use serde_json::Value;

/// Why a statement cannot be a session's start-up setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetProblem {
    /// Not exactly one statement (none, or several).
    NotOne,
    /// The text cannot be read (the parser rejects it, or it is too long or too deep to check).
    Unread,
    /// Not a statement that sets the session's own settings (a query, `RESET`, `SET GLOBAL`, a
    /// plain `SET TRANSACTION`, a call of `set_config()`, …).
    NotSet,
    /// PostgreSQL's `SET LOCAL`: it would end with the transaction it runs in.
    Local,
    /// A setting the app keeps itself: `name` (lower case), and what sets it.
    AppOwned { name: String, owner: Owner },
    /// It may change data or run code the text does not show.
    Writes,
    /// It is dangerous in a way no start-up setting may be.
    Danger(Danger),
}

/// What keeps a setting the app does not let a start-up statement change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The environment's `lock_wait`.
    LockWait,
    /// The environment's `idle_disconnect`.
    IdleDisconnect,
    /// The environment's `tx_idle_rollback`.
    TxIdleRollback,
    /// The environment's `read_only`.
    ReadOnly,
    /// The environment's `commit`.
    Commit,
    /// The tab's schema (`:use`).
    Schema,
    /// The app itself (what it reads results in, how it tells sessions apart, paging).
    App,
}

/// Why a PostgreSQL start-up setting has no form for one transaction (so it is not sent behind a
/// pooler in transaction mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotLocal {
    /// Not a plain `SET name = value` (`SET SESSION CHARACTERISTICS …`).
    Form,
    /// It changes who the session is (`SET ROLE`, `SET SESSION AUTHORIZATION`): set per
    /// transaction, a statement that runs outside one (`VACUUM`, `CALL`) would run as someone
    /// else.
    Identity,
}

/// A statement that may be a session's start-up setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSet {
    /// What it may do, as the classifier reads it (a read-only environment refuses what
    /// [`Risk::read_only`] refuses).
    pub risk: Risk,
    /// PostgreSQL: the same setting for one transaction (`SET LOCAL …`), or why there is none.
    /// `None` for MySQL, whose sessions are never set per transaction.
    pub per_transaction: Option<Result<String, NotLocal>>,
}

/// PostgreSQL's settings the app keeps itself.
const OWNED: &[(&str, Owner)] = &[
    ("search_path", Owner::Schema),
    ("lock_timeout", Owner::LockWait),
    ("idle_session_timeout", Owner::IdleDisconnect),
    ("idle_in_transaction_session_timeout", Owner::TxIdleRollback),
    ("default_transaction_read_only", Owner::ReadOnly),
    ("transaction_read_only", Owner::ReadOnly),
    ("application_name", Owner::App),
    ("client_encoding", Owner::App),
    // How the server writes values as text: the app reads them as the defaults write them.
    ("datestyle", Owner::App),
    ("intervalstyle", Owner::App),
    ("bytea_output", Owner::App),
    ("extra_float_digits", Owner::App),
];

/// What the classifier's risk says against a start-up setting: a write or code, or a danger
/// other than changing a setting (which only a read-only environment refuses).
pub(crate) fn judge(risk: &Risk) -> Result<(), SetProblem> {
    if risk.writes || risk.runs_code || risk.unchecked_call {
        return Err(SetProblem::Writes);
    }
    match risk.danger {
        None | Some(Danger::Setting) => Ok(()),
        Some(Danger::Unparsed | Danger::TooComplex | Danger::Unrecognized) => Err(SetProblem::Unread),
        Some(d) => Err(SetProblem::Danger(d)),
    }
}

/// Whether PostgreSQL statement `sql` may be a session's start-up setting (see the module docs).
pub fn session_set_pg(sql: &str) -> Result<SessionSet, SetProblem> {
    let risk = classify(sql);
    if matches!(risk.danger, Some(Danger::Unparsed | Danger::TooComplex)) {
        return Err(SetProblem::Unread);
    }
    // The tree is read on a thread with the classifier's stack, as the classifier reads it.
    let shape = std::thread::scope(|scope| {
        let worker = std::thread::Builder::new().name(THREAD.to_string()).stack_size(STACK);
        worker.spawn_scoped(scope, || shape(sql)).ok().and_then(|h| h.join().ok())
    });
    let per_transaction = shape.unwrap_or(Err(SetProblem::Unread))?;
    judge(&risk)?;
    Ok(SessionSet { risk, per_transaction: Some(per_transaction) })
}

/// The shape of `sql`: one `VariableSetStmt` that sets a setting for the session, not one the
/// app keeps; its form for one transaction, or why it has none.
fn shape(sql: &str) -> Result<Result<String, NotLocal>, SetProblem> {
    use pg_query::protobuf::VariableSetKind as K;
    let statements = parse(sql).ok_or(SetProblem::Unread)?;
    let [stmt] = statements.as_slice() else { return Err(SetProblem::NotOne) };
    let Some(("VariableSetStmt", b)) = node(stmt) else { return Err(SetProblem::NotSet) };
    let kind = b["kind"].as_i64().unwrap_or(0);
    let name = text(&b["name"]).unwrap_or_default().to_ascii_lowercase();
    let value = [K::VarSetValue, K::VarSetDefault].iter().any(|k| *k as i64 == kind);
    // `SET SESSION CHARACTERISTICS AS TRANSACTION …`: the session's default for its transactions
    // (`SET TRANSACTION …` is the next transaction's only).
    let characteristics = kind == K::VarSetMulti as i64 && name == "session characteristics";
    if !value && !characteristics {
        return Err(SetProblem::NotSet);
    }
    if b["is_local"].as_bool().unwrap_or(false) {
        return Err(SetProblem::Local);
    }
    if characteristics {
        let read_only = b["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(node)
            .any(|(kind, o)| kind == "DefElem" && text(&o["defname"]) == Some("transaction_read_only"));
        if read_only {
            return Err(SetProblem::AppOwned { name: "transaction_read_only".into(), owner: Owner::ReadOnly });
        }
        return Ok(Err(NotLocal::Form));
    }
    if let Some((_, owner)) = OWNED.iter().find(|(n, _)| *n == name) {
        return Err(SetProblem::AppOwned { name, owner: *owner });
    }
    if matches!(name.as_str(), "role" | "session_authorization") {
        return Ok(Err(NotLocal::Identity));
    }
    Ok(set_local(sql, b))
}

/// `sql` (a `SET` of the session, read as `b`) for one transaction: its words without comments
/// or a closing `;` (it goes in a list of statements), `LOCAL` added after its `SET` or in place
/// of its `SESSION`; parsed again, it must set the same setting the same way.
fn set_local(sql: &str, b: &Value) -> Result<String, NotLocal> {
    let mut toks: Vec<_> = lex(sql).into_iter().filter(|t| !t.is_trivia()).collect();
    while toks.last().is_some_and(|t| t.kind == Tok::Semi) {
        toks.pop();
    }
    let is = |i: usize, w: &str| toks.get(i).is_some_and(|t| t.is_word() && t.text(sql).eq_ignore_ascii_case(w));
    if !is(0, "SET") || toks.len() < 2 || toks.iter().any(|t| t.kind == Tok::Semi) {
        return Err(NotLocal::Form);
    }
    // `SESSION` is the scope unless it starts a name (`session.x`).
    let scope = is(1, "SESSION") && toks.get(2).is_some_and(|t| t.kind != Tok::Dot);
    let rest = toks.iter().skip(if scope { 2 } else { 1 }).map(|t| t.text(sql));
    let local = std::iter::once("SET LOCAL").chain(rest).collect::<Vec<_>>().join(" ");
    let again = parse(&local).ok_or(NotLocal::Form)?;
    let [stmt] = again.as_slice() else { return Err(NotLocal::Form) };
    let Some(("VariableSetStmt", l)) = node(stmt) else { return Err(NotLocal::Form) };
    let same = l["is_local"].as_bool() == Some(true)
        && l["kind"] == b["kind"]
        && l["name"] == b["name"]
        && placeless(&l["args"]) == placeless(&b["args"]);
    if same { Ok(local) } else { Err(NotLocal::Form) }
}

/// A parse tree without its `location`s (where each part starts in the text).
fn placeless(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            Value::Object(o.iter().filter(|(k, _)| *k != "location").map(|(k, v)| (k.clone(), placeless(v))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(placeless).collect()),
        v => v.clone(),
    }
}

#[cfg(test)]
mod tests;
