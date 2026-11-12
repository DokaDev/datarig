//! A table tab's query: the rows of its table, filtered and ordered on the server.
//!
//! The tab keeps three snapshots of its [`Query`] ([`TableQuery`]): the `draft` the user is
//! writing, the one `requested` by the run in flight (bound to that run's query id when it is
//! sent) and the one `shown`, of the rows the tab holds (taken when that run's first page
//! lands). Paging, counting, the estimate and export read `shown`, never the draft; an answer of
//! another run is not this one's (its query id differs), so a late answer of an earlier filter
//! never passes for the rows of the current one.
//!
//! Every run of a table tab goes through [`App::table_run`]: the filter's text is checked first
//! ([`query::check_filter`]: a refusal is said and nothing is sent), a call of a function whose
//! effects are not known is confirmed once per text and tab (refused on a read-only profile),
//! then the compiled statement takes the way of any run ([`App::run_in`]: pending edits, the
//! read-only check, the classifier's confirm). A page read again past a closed portal and a
//! count check the shown filter again the same way and never ask.
//!
//! On MySQL a plain read locks the rows it reads (InnoDB reads as `FOR SHARE`) when the
//! session's level is SERIALIZABLE and autocommit is off or a transaction is open (one the app
//! opens for the read with manual commit too): in that state, or with the level not known, a
//! run asks first ([`Locks`]); a yes holds for the tab while nothing that decides it changed
//! ([`LockState`]: not even to come back to what it was). A page read again and a count are
//! refused in that state unless the tab said yes to it.

use super::*;
use datarig_core::driver::structure::TableStructure;
use datarig_core::driver::{ColumnMeta, Isolation, TxRule, TxStep, ValueKind};
use datarig_core::query::{
    self, BarRefusal, ColumnFacts, ColumnKind, Filter, FilterVerdict, LexProblem, OrderError, OrderProblem, Query,
    ShapeProblem, TableName, Target,
};
use std::collections::HashSet;
use std::ops::Range;

/// Where a table tab's statement is opened, not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenIn {
    /// A new console (`space r v`).
    Console,
    /// The user's editor (`Ctrl+G`); what comes back opens in a new console.
    Editor,
    /// A new console, from the filter dialog's `Open in editor`: the dialog's draft, not the
    /// statement of the rows held.
    Dialog,
}

/// A part of the table tab's query the user writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Where,
    Order,
    Limit,
}

/// Why the app did not run what the user wrote: the part, where in its text (bytes) when
/// known, and what the status line said; `text`: what was refused (`at` is in it).
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    pub field: Field,
    pub at: Option<Range<usize>>,
    pub notice: Notice,
    pub text: String,
}

/// Why reads of a table tab take row locks in its session: MySQL with autocommit off, a
/// transaction open, or one the app opens for the read (`manual`: manual commit), at
/// SERIALIZABLE (`known`) or a level not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Locks {
    known: bool,
    autocommit_off: bool,
    manual: bool,
}

/// The session state a yes to reads that lock rows was given in: the session, its level, its
/// autocommit, and the revision of all that ([`TabSession::safety_rev`]), which a change of any
/// of them (or a transaction's start or end) moves on for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct LockState {
    generation: u64,
    revision: u64,
    isolation: Option<Isolation>,
    autocommit_off: bool,
}

/// A query's statement as sent: its text, where the filter's text is in it (bytes), the keys
/// it orders by (the tie-breaker keys appended included) and whether that order is total. What
/// the tab says of rows read with it comes from here, never from compiling the query again
/// (a structure read later changes the next statement, not this one).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Statement {
    pub query: Query,
    pub sql: String,
    pub filter_span: Option<Range<usize>>,
    pub order: Vec<query::OrderKey>,
    pub total: bool,
}

impl Statement {
    /// Query `q` as `c` compiled it.
    pub(super) fn of(q: Query, c: query::Compiled) -> Self {
        Statement { query: q, sql: c.sql, filter_span: c.filter_span, order: c.order, total: c.total }
    }
}

/// The run of the query a tab sent ended without its first page: the rows shown are those of
/// an earlier query. `at`: where in the filter's text the server said the error is (bytes),
/// when it pointed into it; `how`: why the run brought no rows; `message`: the server's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub at: Option<Range<usize>>,
    pub how: Unapplied,
    /// What the server said, when it refused the statement.
    pub message: Option<String>,
}

/// Why the run of a table tab's query brought no rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unapplied {
    /// The server refused its statement.
    Refused,
    /// The user cancelled it.
    Cancelled,
    /// It ended otherwise: its session went, or it answered without rows.
    Ended,
}

/// The metadata session a table tab's keys are read on, as it is now: the tab's profile and
/// database, the tab's binding, and the session itself (the profile's connection generation for
/// its own database, the aux session's id for another one; both change when it opens again).
/// What was read on another one, or on this one before it was lost and opened again, is not
/// this tab's any more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeysOn {
    pub profile: ProfileId,
    pub database: Option<String>,
    pub binding: u64,
    pub session: u64,
}

/// What the tab knows of its table's keys that make an order total, from the key-only catalog
/// read (`DbCommand::LoadTableKeys`), each state for the session it was asked on ([`KeysOn`]).
/// Only these facts make an order total: the explorer's structure does not say whether a key
/// is deferred, valid, or compares as an `ORDER BY` does.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum OrderKeysState {
    /// Not asked yet.
    #[default]
    Unknown,
    /// Request `request` is on its way.
    Asked { request: u64, on: KeysOn },
    /// Read.
    Known { on: KeysOn, keys: datarig_core::driver::keys::OrderKeys },
    /// The read failed (`reason`, as said): not known (an order is not taken for total), asked
    /// again with the next run.
    Failed { on: KeysOn, reason: String },
}

/// The most bytes a text field of a table's query holds (a paste is cut there, and said).
pub const FIELD_CAP: usize = 16 * 1024;

/// The query state of a table tab.
#[derive(Clone, Debug)]
pub struct TableQuery {
    /// What the user is writing: run by the next apply.
    pub draft: Query,
    /// The query of the run in flight, by its query id. `None` when the tab's last run was
    /// not one of its query (nothing is claimed about what it read).
    pub requested: Option<(u64, Query)>,
    /// The query of the rows the tab holds; `None` when it holds none, or rows of a statement
    /// that was not its query.
    pub shown: Option<Query>,
    /// The latest refusal of what the user wrote, until the next run is let through.
    pub refusal: Option<Refusal>,
    /// The query and its statement on their way to a session (past the checks, through the
    /// run's own asks and queues): bound to the run that sends exactly that statement.
    pending: Option<Statement>,
    /// The statement of the run in flight when it is the query's ([`TableQuery::requested`]),
    /// by its query id: where a server's error points is read in it.
    statement: Option<(u64, Statement)>,
    /// The run of the query that failed on the server, by its query id: what the dialog says.
    failed: Option<(u64, Failure)>,
    /// The run whose first page landed last, by its query id: it brought its query's rows.
    landed: Option<u64>,
    /// The statement the rows held were read with (`shown`'s, as it was sent).
    shown_run: Option<Statement>,
    /// The query waiting for the user's word on the functions it calls.
    asked: Option<Query>,
    /// Where the statement of the query waiting for that word is to be opened instead of run
    /// (`space r v`, `Ctrl+G`); `None`: it is to run.
    asked_open: Option<OpenIn>,
    /// The filter texts whose functions the user let run in this tab.
    confirmed: HashSet<String>,
    /// The query waiting for the user's word on reads that lock rows, and the state asked in.
    locks_asked: Option<(Query, LockState)>,
    /// The session state in which the user let reads that lock rows run in this tab.
    locks_confirmed: Option<LockState>,
    /// The query held until the tab's session, still opening, has said its state, and the
    /// session (its generation) it waits for: work in progress of the tab until it runs, is
    /// cancelled or that session is gone.
    session_wait: Option<(Query, u64)>,
    /// The query to run once the table's keys are read (its order is made total with them), and
    /// the tab binding it waits in: work in progress of the tab until it runs, is cancelled or
    /// its wait ends ([`App::table_keys_waiting`]).
    keys_wait: Option<(Query, u64)>,
    /// The query the tab held as applied before its last applied change (a run of another
    /// condition, order or limit sent): the filter dialog's undo goes back to it once.
    pub before_last: Option<Query>,
    /// The query put back in the tab as applied ([`TableQuery::restored`]) while no run of the
    /// tab's query has been sent: what was applied before the first one, though the draft that
    /// run sends is written over it before it is sent (and stays written over when the run is
    /// refused or declined).
    restored: Option<Query>,
    /// The table's keys for a total order, as far as read.
    pub order_keys: OrderKeysState,
    /// The query a sort action runs ([`App::table_sort`]): the draft once its statement is
    /// sent, so that a sort asked about and declined leaves the draft as it was.
    sorted: Option<Query>,
}

impl TableQuery {
    /// Every row of `table`, nothing run yet.
    pub fn new(table: TableName) -> Self {
        Self {
            draft: Query::table(table),
            requested: None,
            shown: None,
            refusal: None,
            pending: None,
            statement: None,
            failed: None,
            landed: None,
            shown_run: None,
            asked: None,
            asked_open: None,
            confirmed: HashSet::new(),
            locks_asked: None,
            locks_confirmed: None,
            session_wait: None,
            keys_wait: None,
            before_last: None,
            restored: None,
            order_keys: OrderKeysState::Unknown,
            sorted: None,
        }
    }

    /// Query `q` put back in a tab (the workspace restored, a closed tab reopened): applied,
    /// nothing run. The next run checks it as any other.
    pub fn restored(q: Query) -> Self {
        let query::Source::Table(table) = q.source.clone();
        Self { draft: q.clone(), restored: Some(q), ..Self::new(table) }
    }

    /// What the tab holds as applied: the query of the run sent last when it was its query,
    /// else the one of the rows it holds, else the draft (nothing run yet).
    pub fn applied(&self) -> &Query {
        self.requested.as_ref().map(|(_, q)| q).or(self.shown.as_ref()).unwrap_or(&self.draft)
    }

    /// Whether the rows held are those of the query applied: not after a run of the query that
    /// brought no rows (refused, cancelled, its session lost), whose query stays applied while
    /// the rows held are an earlier query's.
    pub fn shows_applied(&self) -> bool {
        self.requested.as_ref().is_none_or(|(_, q)| self.shown.as_ref() == Some(q))
    }

    /// Run `qid` sends `statements`: it is the pending query's run when it sends exactly its
    /// statement, else the run of something else. A query of another condition, order or limit
    /// than the one applied keeps that one as [`TableQuery::before_last`].
    pub(super) fn sent(&mut self, qid: u64, statements: &[String]) {
        // Before the first run of a tab put back, what it held as applied is what was put back,
        // not the draft written over it for this run.
        let before = match (&self.requested, &self.shown, &self.restored) {
            (None, None, Some(q)) => q.clone(),
            _ => self.applied().clone(),
        };
        self.statement = match self.pending.take() {
            Some(s) if statements.len() == 1 && statements[0] == s.sql => Some((qid, s)),
            _ => None,
        };
        self.requested = self.statement.as_ref().map(|(id, s)| (*id, s.query.clone()));
        if let Some((_, q)) = &self.requested {
            self.restored = None;
            if changed(&before, q) {
                self.before_last = Some(before);
            }
        }
        if let Some(q) = self.sorted.take().filter(|q| self.requested.as_ref().is_some_and(|(_, r)| r == q)) {
            self.draft = q;
        }
    }

    /// The first page of run `qid` landed: the rows are of its query, when it was one. Returns
    /// that query (`None`: the run was not the query's, or an earlier run's).
    pub(super) fn first_page(&mut self, qid: u64) -> Option<Query> {
        self.shown = self.requested.as_ref().filter(|(id, _)| *id == qid).map(|(_, q)| q.clone());
        self.shown_run = self.statement.as_ref().filter(|(id, _)| *id == qid).map(|(_, s)| s.clone());
        self.failed = None;
        self.landed = Some(qid);
        self.shown.clone()
    }

    /// The statement the rows held were read with, when they are of the tab's query.
    pub(super) fn shown_run(&self) -> Option<&Statement> {
        self.shown_run.as_ref().filter(|_| self.shown.is_some())
    }

    /// Run `qid` ended (`how`, when it brought no rows): when it was the query's and its first
    /// page did not land, the query is marked not applied (a server's refusal marked already
    /// stays as it is). The rows held stay those of `shown`.
    pub(super) fn run_over(&mut self, qid: u64, how: Unapplied) {
        let ours = self.requested.as_ref().is_some_and(|(id, _)| *id == qid);
        let marked = self.failed.as_ref().is_some_and(|(id, _)| *id == qid);
        if ours && !marked && self.landed != Some(qid) {
            self.failed = Some((qid, Failure { at: None, how, message: None }));
        }
    }

    /// The server refused the statement of run `qid` (`error`, in dialect `d`): when it is the
    /// query's, the query is marked failed, with where in its filter's text the error is when
    /// the server pointed there. PostgreSQL counts characters of the statement as sent; MySQL
    /// quotes the text from the error on (`near '…'`), which marks the filter only when it is
    /// found once in the statement and starts in the filter. A server's error of a statement
    /// with an order drops the keys read: another session may have changed them (a key column
    /// renamed), and the next run reads them again rather than append a column that may be
    /// gone (until then the order is not taken for total, never for one without a key).
    pub(super) fn run_failed(&mut self, qid: u64, error: &ServerError<'_>, d: Dialect) {
        let Some((_, s)) = self.statement.as_ref().filter(|(id, _)| *id == qid) else { return };
        let ordered = !s.order.is_empty();
        let at = s.filter_span.as_ref().and_then(|span| {
            let (start, end) = match (error, d) {
                (ServerError::Message(m), Dialect::Postgres) => {
                    let at = datarig_core::driver::ServerMessage::byte_offset(&s.sql, m.position?)?;
                    (at, None)
                }
                (ServerError::Message(m), Dialect::MySql(_)) => {
                    let near = mysql_near(&m.text)?;
                    let at = s.sql.find(near)?;
                    // Found again from the next character on (an overlapping match counts):
                    // unknown, not located.
                    let next = at + s.sql[at..].chars().next().map_or(1, char::len_utf8);
                    if s.sql[next..].contains(near) {
                        return None;
                    }
                    (at, Some(at + near.len()))
                }
                (ServerError::Other, _) => return None,
            };
            if !span.contains(&start) {
                return None;
            }
            let text = &s.sql[span.clone()];
            let from = start - span.start;
            let to = match end {
                Some(end) => end.min(span.end) - span.start,
                None => token_end(text, from, d),
            };
            Some(from..to)
        });
        let message = match error {
            ServerError::Message(m) => Some(m.text.clone()),
            ServerError::Other => None,
        };
        self.failed = Some((qid, Failure { at, how: Unapplied::Refused, message }));
        if ordered && matches!(error, ServerError::Message(_)) {
            self.order_keys = OrderKeysState::Unknown;
        }
    }

    /// The failure of the query the tab holds as applied, while it is the run sent last.
    pub fn failure(&self) -> Option<&Failure> {
        let (qid, f) = self.failed.as_ref()?;
        self.requested.as_ref().is_some_and(|(id, _)| id == qid).then_some(f)
    }

    /// The text of the shown filter, when it has one.
    pub fn shown_filter(&self) -> Option<&str> {
        filter_text(self.shown.as_ref()?)
    }
}

/// `b` reads another condition, order (the user's keys) or limit than `a`.
fn changed(a: &Query, b: &Query) -> bool {
    let user = |q: &Query| -> Vec<query::OrderKey> {
        q.order.iter().filter(|k| k.origin == query::KeyOrigin::User).cloned().collect()
    };
    filter_text(a) != filter_text(b) || user(a) != user(b) || a.limit != b.limit
}

/// The condition of the rows table tab `t` holds, when they are filtered (the tab's title and
/// the tab list say it; a query that did not bring its rows is not said there).
pub fn applied_filter(t: &Tab) -> Option<&str> {
    t.doc.table_query.as_ref()?.shown_filter()
}

/// The filter text of `q`, unless it has none or only blanks (which filter nothing).
pub fn filter_text(q: &Query) -> Option<&str> {
    q.filter.as_ref().map(|f| f.text.as_str()).filter(|t| !t.trim().is_empty())
}

/// `a` and `b` read the same rows in the same order with the same cap: what the app understood
/// of a filter ([`Filter::recognised`]) is left out, and a blank filter is none.
pub fn same_query(a: &Query, b: &Query) -> bool {
    a.source == b.source
        && filter_text(a) == filter_text(b)
        && a.order == b.order
        && a.limit == b.limit
        && a.offset == b.offset
}

/// `a` and `b` read the same rows, in any order ([`same_query`] but for the order).
pub fn same_rows(a: &Query, b: &Query) -> bool {
    same_query(&Query { order: Vec::new(), ..a.clone() }, &Query { order: Vec::new(), ..b.clone() })
}

/// The largest cap a table tab's rows take: what both servers read as a `LIMIT`
/// (PostgreSQL's is a signed 64-bit number).
pub const LIMIT_MAX: u64 = i64::MAX as u64;

/// Why the text of the `LIMIT` field is not a cap on the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitProblem {
    /// Not a whole number written in digits.
    NotANumber,
    /// Zero rows.
    Zero,
    /// Above [`LIMIT_MAX`].
    TooLarge,
}

/// The cap the `LIMIT` field's `text` sets: `None` for a blank text (no cap), else a whole
/// number of rows from 1 to [`LIMIT_MAX`], written in digits (blanks around it are left out).
/// A refusal says where in `text` (bytes) and why.
pub fn parse_limit(text: &str) -> Result<Option<u64>, (Range<usize>, LimitProblem)> {
    let digits = text.trim();
    if digits.is_empty() {
        return Ok(None);
    }
    let start = text.len() - text.trim_start().len();
    let at = start..start + digits.len();
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err((at, LimitProblem::NotANumber));
    }
    match digits.parse::<u64>() {
        Ok(0) => Err((at, LimitProblem::Zero)),
        Ok(n) if n <= LIMIT_MAX => Ok(Some(n)),
        _ => Err((at, LimitProblem::TooLarge)),
    }
}

/// Why the `LIMIT` field is refused, in words.
pub(super) fn limit_why(i18n: &I18n, p: LimitProblem) -> String {
    let max = datarig_i18n::fmt_count(LIMIT_MAX);
    match p {
        LimitProblem::NotANumber => i18n.label(Label::TableLimitWhyNotANumber).to_string(),
        LimitProblem::Zero => i18n.label(Label::TableLimitWhyZero).to_string(),
        LimitProblem::TooLarge => i18n.msg(&Msg::TableLimitWhyTooLarge { max }).to_string(),
    }
}

/// A page of `rows` read after the `held` rows a result has, the result capped at `cap` rows:
/// the rows past the cap are left out. `Some(more)` once the rows reach the cap (`more`: the
/// server had more, so what it holds open for the result is to be closed: no page past the
/// cap is fetched), `None` while they do not (or nothing caps them).
pub(crate) fn cap_page<T>(rows: &mut Vec<T>, held: usize, cap: Option<u64>, more: bool) -> Option<bool> {
    let left = usize::try_from(cap?).unwrap_or(usize::MAX).saturating_sub(held);
    if rows.len() < left {
        return None;
    }
    rows.truncate(left);
    Some(more)
}

/// The kind of a result column, as the query model writes literals for it.
pub(crate) fn column_kind(c: &ColumnMeta) -> ColumnKind {
    match c.kind {
        // PostgreSQL's `real` (`float4`), MySQL's `FLOAT`: single precision.
        ValueKind::Float if matches!(c.type_name.as_str(), "float4" | "float" | "float unsigned") => ColumnKind::Real,
        ValueKind::Integer | ValueKind::Float => ColumnKind::Number,
        ValueKind::Decimal if c.type_name != "money" => ColumnKind::Number,
        ValueKind::Text => ColumnKind::Text,
        ValueKind::Bytes => ColumnKind::Bytes,
        _ => ColumnKind::Other,
    }
}

/// A result column of dialect `d` that is a MySQL `TIMESTAMP` ([`ColumnFacts::zoned`]).
fn zoned(d: Dialect, c: &ColumnMeta) -> bool {
    matches!(d, Dialect::MySql(_)) && c.kind == ValueKind::TimestampTz
}

/// Why the bar refuses a filter, in words (the part in parentheses of the refusal).
pub(super) fn refusal_why(i18n: &I18n, r: &BarRefusal) -> String {
    let label = |l: Label| i18n.label(l).to_string();
    match r {
        BarRefusal::Empty => String::new(),
        BarRefusal::Lexical { problem, .. } => label(match problem {
            LexProblem::Semicolon => Label::TableFilterWhySemicolon,
            LexProblem::Comment => Label::TableFilterWhyComment,
            LexProblem::ExecutableComment => Label::TableFilterWhyExecutableComment,
            LexProblem::ClientCommand => Label::TableFilterWhyClientCommand,
            LexProblem::Unclosed => Label::TableFilterWhyUnclosed,
            LexProblem::ClosesOutside => Label::TableFilterWhyClosesOutside,
            LexProblem::Unterminated => Label::TableFilterWhyUnterminated,
            LexProblem::Ambiguous => Label::TableFilterWhyAmbiguous,
        }),
        BarRefusal::Shape { problem, .. } => label(match problem {
            ShapeProblem::NotOneSelect => Label::TableFilterWhyNotOneSelect,
            ShapeProblem::Source => Label::TableFilterWhySource,
            ShapeProblem::SetOperation => Label::TableFilterWhySetOperation,
            ShapeProblem::Clause => Label::TableFilterWhyClause,
            ShapeProblem::Locking => Label::TableFilterWhyLocking,
            ShapeProblem::Into => Label::TableFilterWhyInto,
            ShapeProblem::Assignment => Label::TableFilterWhyAssignment,
            ShapeProblem::Sequence => Label::TableFilterWhySequence,
        }),
        BarRefusal::NotRead { .. } => label(Label::TableFilterWhyNotRead),
        BarRefusal::RefusedCalls(names) => {
            i18n.msg(&Msg::TableFilterWhyRefusedCalls { names: names.join(", ") }).to_string()
        }
        BarRefusal::UncheckedCalls(names) => {
            i18n.msg(&Msg::TableFilterWhyUncheckedCalls { names: names.join(", ") }).to_string()
        }
        BarRefusal::ReadOnly(why) => {
            let what = label(super::safety::blocked_label(*why));
            i18n.msg(&Msg::TableFilterWhyReadOnly { what }).to_string()
        }
    }
}

/// What the server said of a failed statement, as the tab reads it.
pub(super) enum ServerError<'a> {
    /// Its own message, which may say where the error is.
    Message(&'a datarig_core::driver::ServerMessage),
    /// Anything else (a lost session, a lock wait): no place in the text.
    Other,
}

/// The text MySQL quotes from where it stopped reading (`… near 'x = = 1)' at line 1`), when
/// it quotes any.
pub(super) fn mysql_near(message: &str) -> Option<&str> {
    let from = message.find(" near '")? + " near '".len();
    let rest = &message[from..];
    let to = rest.rfind("' at line ")?;
    Some(&rest[..to]).filter(|s| !s.is_empty())
}

/// Where the token that starts at byte `from` of `text` ends (one character for a blank or the
/// end of the text): what a server's position marks.
fn token_end(text: &str, from: usize, d: Dialect) -> usize {
    let next = || text[from..].chars().next().map_or(from, |c| from + c.len_utf8());
    match datarig_core::sql::lexer::lex_in(text, d).into_iter().find(|t| t.start <= from && from < t.end) {
        Some(t) if t.kind != datarig_core::sql::lexer::Tok::Whitespace => t.end,
        _ => next(),
    }
}

/// Where in the filter's text a refusal is (bytes), when known.
fn refusal_at(r: &BarRefusal) -> Option<Range<usize>> {
    match r {
        BarRefusal::Lexical { at, .. } => Some(at.clone()),
        BarRefusal::Shape { at, .. } => at.clone(),
        _ => None,
    }
}

/// Why an `ORDER BY` text is refused, in words.
pub(super) fn order_why(i18n: &I18n, e: &OrderError) -> String {
    let label = |l: Label| i18n.label(l).to_string();
    match &e.kind {
        OrderProblem::MissingColumn => label(Label::TableOrderWhyMissingColumn),
        OrderProblem::NotAColumn => label(Label::TableOrderWhyNotAColumn),
        OrderProblem::UnknownColumn(name) => {
            i18n.msg(&Msg::TableOrderWhyUnknownColumn { name: name.clone() }).to_string()
        }
        OrderProblem::AmbiguousColumn(name) => {
            i18n.msg(&Msg::TableOrderWhyAmbiguousColumn { name: name.clone() }).to_string()
        }
        OrderProblem::Duplicate(name) => i18n.msg(&Msg::TableOrderWhyDuplicate { name: name.clone() }).to_string(),
        OrderProblem::Unterminated => label(Label::TableOrderWhyUnterminated),
        OrderProblem::NullsUnsupported => label(Label::TableOrderWhyNullsUnsupported),
        OrderProblem::NullsIncomplete => label(Label::TableOrderWhyNullsIncomplete),
        OrderProblem::Unexpected => label(Label::TableOrderWhyUnexpected),
    }
}

impl App {
    /// The table of table tab `id` as the query model names it (in the tab's database).
    pub(crate) fn table_name(&self, id: TabId) -> Option<TableName> {
        let t = self.tabs.get(id).filter(|t| t.is_table())?;
        let table = t.doc.table.as_ref()?;
        Some(TableName { database: t.context.database.clone(), schema: table.schema.clone(), name: table.name.clone() })
    }

    /// The query state of table tab `id`.
    pub(crate) fn table_query(&self, id: TabId) -> Option<&TableQuery> {
        self.tabs.get(id)?.doc.table_query.as_deref()
    }

    /// The query state of table tab `id`, made when the tab has none yet (every row of its
    /// table). Its table and database are the tab's own, whatever the state held.
    pub(super) fn table_query_mut(&mut self, id: TabId) -> Option<&mut TableQuery> {
        let name = self.table_name(id)?;
        let t = self.tabs.get_mut(id)?;
        let tq = t.doc.table_query.get_or_insert_with(|| Box::new(TableQuery::new(name.clone())));
        tq.draft.source = query::Source::Table(name);
        Some(tq)
    }

    /// The structure of table tab `t`'s table, when the explorer of its profile and database
    /// has read it.
    fn table_structure<'a>(&'a self, t: &Tab) -> Option<&'a TableStructure> {
        let table = t.doc.table.as_ref()?;
        let id = t.profile?;
        let tree = match t.context.database.as_deref() {
            None => &self.conns.get(id)?.tree,
            Some(db) => &self.conns.aux(id, db)?.tree,
        };
        tree.structures.get(&(table.schema.clone(), table.name.clone()))?.loaded()
    }

    /// The structure of table tab `id`'s table, when the explorer has read it.
    pub(super) fn table_structure_of(&self, id: TabId) -> Option<&TableStructure> {
        self.table_structure(self.tabs.get(id).filter(|t| t.is_table())?)
    }

    /// What the app knows of table tab `id`'s table for writing its SQL: the columns of the
    /// rows it holds of its query (else those of the key-only read, of their catalogue type's
    /// kind; else the catalog's names, of unknown kind), with the
    /// explorer's structure when it was read (collations), and the keys of the key-only read on
    /// the tab's metadata session as it is now ([`OrderKeysState`]), with whether each column's
    /// collation is deterministic as that read says (when the structure does not). Until they
    /// are known no key is: no tie-breaker is appended and no order is taken for total.
    pub(crate) fn table_target(&self, id: TabId) -> Option<Target> {
        let t = self.tabs.get(id).filter(|t| t.is_table())?;
        let table = t.doc.table.as_ref()?;
        let d = self.tab_dialect(id);
        let held = t.doc.table_query.as_ref().is_some_and(|q| q.shown.is_some());
        // The keys that make an order total: only the key-only read's, for the session as it
        // is now (rows of tables that inherit from it are read too: its keys do not hold).
        let on = self.keys_on(t);
        let read = t.doc.table_query.as_ref().and_then(|q| match &q.order_keys {
            OrderKeysState::Known { on: o, keys } if Some(o) == on.as_ref() => Some(keys),
            _ => None,
        });
        // No rows held: the columns of that read, of the kind their catalogue type says (as the
        // filter dialog takes them), so that a value is written for its column as it would be
        // with rows held; else the explorer's names, of unknown kind.
        let catalogue = read.and_then(|k| k.columns.as_ref()).filter(|c| !c.is_empty());
        let mut columns: Vec<ColumnFacts> = match (&t.results, catalogue) {
            (Results::Rows(rs), _) if held => rs
                .columns
                .iter()
                .map(|c| ColumnFacts {
                    zoned: zoned(d, &c.meta),
                    ..ColumnFacts::new(c.meta.name.clone(), column_kind(&c.meta))
                })
                .collect(),
            (_, Some(cat)) => cat
                .iter()
                .map(|c| ColumnFacts {
                    zoned: super::filter_dialog::draft::zoned_type(d, &c.type_name),
                    ..ColumnFacts::new(c.name.clone(), super::filter_dialog::draft::kind_of_type(d, &c.type_name))
                })
                .collect(),
            _ => self
                .conns
                .catalog_in(t.profile, t.context.database.as_deref())
                .relations
                .iter()
                .find(|r| r.schema == table.schema && r.name == table.name)
                .map(|r| r.columns.iter().map(|c| ColumnFacts::new(c.name.clone(), ColumnKind::Other)).collect())
                .unwrap_or_default(),
        };
        // A column `SELECT *` leaves out (MySQL INVISIBLE) is no column of the rows held, but one
        // of the table: of the kind its catalogue type says.
        if held && let Some(cat) = read.and_then(|k| k.columns.as_ref()) {
            for c in cat.iter().filter(|c| c.invisible) {
                if columns.iter().any(|f| f.name == c.name) {
                    continue;
                }
                let kind = super::filter_dialog::draft::catalogue_meta(d, &c.name, &c.type_name)
                    .map_or(ColumnKind::Other, |m| column_kind(&m));
                // A TIMESTAMP is written from its instant, as one of the rows is.
                let zoned = super::filter_dialog::draft::zoned_type(d, &c.type_name);
                columns.push(ColumnFacts { zoned, ..ColumnFacts::new(c.name.clone(), kind) });
            }
        }
        let s = self.table_structure(t);
        if columns.is_empty()
            && let Some(s) = s
        {
            columns = s.columns.iter().map(|c| ColumnFacts::new(c.name.clone(), ColumnKind::Other)).collect();
        }
        let mut target = Target::new(d, columns);
        if let Some(s) = s {
            for c in &mut target.columns {
                if let Some(coll) = s.columns.iter().find(|sc| sc.name == c.name).and_then(|sc| sc.collation.as_ref()) {
                    c.collation = Some(coll.name.clone());
                    c.deterministic = Some(coll.deterministic);
                }
            }
        }
        if let Some(k) = read {
            for c in target.columns.iter_mut().filter(|c| c.deterministic.is_none()) {
                c.deterministic = k.deterministic.get(&c.name).copied();
            }
            target.like_nondeterministic = k.like_nondeterministic;
        }
        if let Some(k) = read.filter(|k| !k.inherited) {
            target.primary_key = k.primary_key.clone();
            target.unique_keys = k.unique_not_null.clone();
        }
        target.utf8mb4 = matches!(d, Dialect::MySql(_))
            && t.exec.values.as_ref().is_some_and(|v| v.charset.eq_ignore_ascii_case("utf8mb4"));
        Some(target)
    }

    /// Run table tab `id`'s draft (open, the run key, a reload).
    pub(crate) fn table_rerun(&mut self, id: TabId) {
        let Some(q) = self.table_query_mut(id).map(|tq| tq.draft.clone()) else { return };
        self.table_run(id, q);
    }

    /// Run `q` in table tab `id`: refused while the tab runs; its filter checked (a refusal is
    /// said, kept as the refusal and nothing is sent); calls of functions whose effects are not
    /// known confirmed first (once per text in the tab); then its statement runs as any run
    /// does ([`App::run_in`]: pending edits asked about, read-only and classifier checks).
    pub(crate) fn table_run(&mut self, id: TabId, q: Query) {
        self.table_run_with(id, q, true);
    }

    /// [`App::table_run`]; `keys_first`: the tab's first query with an order runs once the
    /// table's keys are read on the tab's metadata session as it is now (asked for, the
    /// profile connected first); else it runs with what is known.
    fn table_run_with(&mut self, id: TabId, mut q: Query, keys_first: bool) {
        if self.tab_busy(id) {
            return self.flash_busy();
        }
        let (Some(name), Some(target)) = (self.table_name(id), self.table_target(id)) else { return };
        q.source = query::Source::Table(name.clone());
        if q.filter.as_ref().is_some_and(|f| f.text.trim().is_empty()) {
            q.filter = None;
        }
        if let Some(text) = filter_text(&q).map(str::to_string) {
            let read_only = self.tabs.get(id).and_then(|t| t.profile).is_some_and(|p| self.read_only(p));
            let verdict = query::check_filter(&text, &name, &target, &self.classifier(id), read_only);
            match verdict {
                Err(r) => return self.filter_refused(id, &r, text),
                Ok(FilterVerdict::NeedsConfirm(asks))
                    if !self.table_query(id).is_some_and(|tq| tq.confirmed.contains(&text)) =>
                {
                    let calls: Vec<String> = asks
                        .iter()
                        .flat_map(|a| match a {
                            query::Confirm::Calls(c) => c.clone(),
                        })
                        .collect();
                    if let Some(tq) = self.table_query_mut(id) {
                        tq.asked = Some(q);
                        tq.asked_open = None;
                    }
                    return self.ask_filter_calls(id, calls);
                }
                Ok(_) => {}
            }
        }
        // A query put back in the tab (the workspace restored, a closed tab reopened) runs
        // before the tab has read anything: its order waits for the keys a tab reads when it
        // opens, so it is made total as one typed after them.
        let first = self.table_query(id).is_some_and(|tq| tq.landed.is_none());
        if keys_first && first && !q.order.is_empty() {
            // Waiting before the keys are asked for: a connect that fails at once ends the wait
            // (and says so, [`App::table_keys_dropped`]).
            let binding = self.tabs.get(id).map_or(0, |t| t.binding);
            if let Some(tq) = self.table_query_mut(id) {
                tq.keys_wait = Some((q.clone(), binding));
            }
            if self.keys_to_read(id) {
                if self.table_keys_waiting(id) {
                    self.tab_status(id, Notice::new(Label::TableOrderKeysFirst, Level::Info));
                }
                return;
            }
            if let Some(tq) = self.table_query_mut(id) {
                tq.keys_wait = None;
            }
        }
        let compiled = query::compile(&q, &target);
        let sql = compiled.sql.clone();
        // Asked before the run's own asks on the session the tab has; a run that opens one is
        // asked about on it, right before it is sent ([`App::table_locks_hold`]).
        let session = self.tabs.get(id).is_some_and(|t| t.exec.session.is_some());
        if session
            && let Some((locks, state)) =
                self.table_locks(id, Some(&sql)).filter(|(_, st)| !self.locks_confirmed(id, *st))
        {
            if let Some(tq) = self.table_query_mut(id) {
                tq.locks_asked = Some((q, state));
            }
            return self.ask_table_locks(id, locks);
        }
        if let Some(tq) = self.table_query_mut(id) {
            tq.refusal = None;
            tq.pending = Some(Statement::of(q, compiled));
        }
        self.run_in(id, vec![sql]);
        // The keys that make an order total: asked for with the first run (the tab opening),
        // so that an order applied later is known total.
        self.order_keys_ask(id);
    }

    /// The filter `text` of table tab `id` is refused (`r`): said, and kept as the refusal.
    fn filter_refused(&mut self, id: TabId, r: &BarRefusal, text: String) {
        let why = refusal_why(&self.i18n, r);
        let notice = Notice::new(Msg::TableFilterRefused { why }, Level::Warning);
        if let Some(tq) = self.table_query_mut(id) {
            let at = refusal_at(r);
            tq.refusal = Some(Refusal { field: Field::Where, at, notice: notice.clone(), text });
        }
        self.tab_status(id, notice);
    }

    /// The statement opened for table tab `id` (in a console, or in the user's editor), checked
    /// as a run would be: the one the rows held were read with (their filter checked again),
    /// else the draft's (checked as Enter checks it, its unchecked functions asked about first,
    /// `y` opening it `to`). `None` when refused (said, as Enter says it) or asked about.
    pub(super) fn table_sql_to_open(&mut self, id: TabId, to: OpenIn) -> Option<String> {
        let tq = self.table_query(id)?;
        if let Some(s) = tq.shown_run() {
            // With its cap written, so that running it reads the rows the tab shows.
            let sql = self.table_capped_sql(id).unwrap_or_else(|| s.sql.clone());
            if let Err(why) = self.table_recheck(id) {
                self.tab_status(id, Notice::new(Msg::TableFilterRefused { why }, Level::Warning));
                return None;
            }
            return Some(sql);
        }
        let q = tq.draft.clone();
        self.table_query_sql_to_open(id, q, to)
    }

    /// The statement of query `q` of table tab `id`, opened `to` and not run: its filter
    /// checked as Enter checks it, its unchecked functions asked about first (`y` opening it).
    /// `None` when refused (said, as Enter says it) or asked about.
    pub(super) fn table_query_sql_to_open(&mut self, id: TabId, q: Query, to: OpenIn) -> Option<String> {
        let (name, target) = (self.table_name(id)?, self.table_target(id)?);
        if let Some(text) = filter_text(&q).map(str::to_string) {
            let read_only = self.tabs.get(id).and_then(|t| t.profile).is_some_and(|p| self.read_only(p));
            match query::check_filter(&text, &name, &target, &self.classifier(id), read_only) {
                Err(r) => {
                    self.filter_refused(id, &r, text);
                    return None;
                }
                // Its functions asked about first, as a run asks: `y` opens it.
                Ok(FilterVerdict::NeedsConfirm(asks))
                    if !self.table_query(id).is_some_and(|tq| tq.confirmed.contains(&text)) =>
                {
                    let calls = asks
                        .iter()
                        .flat_map(|a| match a {
                            query::Confirm::Calls(c) => c.clone(),
                        })
                        .collect();
                    if let Some(tq) = self.table_query_mut(id) {
                        tq.asked = Some(q);
                        tq.asked_open = Some(to);
                    }
                    self.ask_filter_calls(id, calls);
                    return None;
                }
                Ok(_) => {}
            }
        }
        Some(query::compile_capped(&q, &target).sql)
    }

    /// `space r v`: table tab `id`'s statement in a new console, checked first
    /// ([`App::table_sql_to_open`]); not run.
    pub(super) fn table_open_sql(&mut self, id: TabId) {
        if self.table_query_mut(id).is_none() {
            return;
        }
        let Some(sql) = self.table_sql_to_open(id, OpenIn::Console) else { return };
        let Some(t) = self.tabs.get(id) else { return };
        let (profile, context) = (t.profile, t.context.clone());
        self.console_with(&sql, profile, context);
    }

    /// The filter of the table query whose statement run `statements` sends, checked again
    /// with the policy as it is now (a run that waited for its connection while the
    /// environment turned read-only): `false` when refused (said, kept as the refusal, and the
    /// statement is not bound to the run any more).
    pub(super) fn table_pending_holds(&mut self, id: TabId, statements: &[String]) -> bool {
        let Some(tq) = self.table_query(id) else { return true };
        let Some(p) = tq.pending.as_ref().filter(|p| statements.len() == 1 && statements[0] == p.sql) else {
            return true;
        };
        let Some(text) = filter_text(&p.query).map(str::to_string) else { return true };
        let (Some(name), Some(target)) = (self.table_name(id), self.table_target(id)) else { return true };
        let read_only = self.tabs.get(id).and_then(|t| t.profile).is_some_and(|pid| self.read_only(pid));
        let refused = match query::check_filter(&text, &name, &target, &self.classifier(id), read_only) {
            Err(r) => Some(Err(r)),
            Ok(FilterVerdict::NeedsConfirm(_)) if !tq.confirmed.contains(&text) => Some(Ok(())),
            Ok(_) => None,
        };
        match refused {
            None => true,
            Some(r) => {
                if let Some(tq) = self.table_query_mut(id) {
                    tq.pending = None;
                }
                match r {
                    Err(r) => self.filter_refused(id, &r, text),
                    Ok(()) => {
                        let why = self.i18n.label(Label::TableFilterWhyUnconfirmed).to_string();
                        self.tab_status(id, Notice::new(Msg::TableFilterRefused { why }, Level::Warning));
                    }
                }
                false
            }
        }
    }

    /// The metadata session table tab `t`'s keys are read on, as it is now ([`KeysOn`]):
    /// `None` while there is none open.
    pub(super) fn keys_on(&self, t: &Tab) -> Option<KeysOn> {
        let profile = t.profile?;
        let database = t.context.database.clone();
        let session = match database.as_deref() {
            None => self.conns.get(profile).filter(|c| c.meta.is_some()).map(|c| c.generation)?,
            Some(db) => self.conns.aux(profile, db).filter(|a| a.session.is_some()).map(|a| a.id)?,
        };
        Some(KeysOn { profile, database, binding: t.binding, session })
    }

    /// Whether table tab `id`'s keys are to be read before its order runs: not read on its
    /// metadata session as it is now (nor failed there), and asked for now, or once its
    /// profile is connected (connecting it first). `false` when they cannot be asked for.
    fn keys_to_read(&mut self, id: TabId) -> bool {
        let Some(t) = self.tabs.get(id) else { return false };
        let Some(pid) = t.profile else { return false };
        if !self.structure_on(pid) {
            return false;
        }
        let on = self.keys_on(t);
        let state = t.doc.table_query.as_ref().map(|q| &q.order_keys);
        if matches!(state, Some(OrderKeysState::Known { on: o, .. } | OrderKeysState::Failed { on: o, .. }) if Some(o) == on.as_ref())
        {
            return false;
        }
        if !self.conns.get(pid).is_some_and(|c| c.connected) {
            // Read once the profile is connected ([`App::table_keys_connected`]).
            if self.conns.get(pid).is_none_or(|c| c.connecting.is_none()) {
                self.connect(pid);
            }
            return true;
        }
        self.order_keys_ask(id);
        let t = self.tabs.get(id);
        matches!(t.and_then(|t| t.doc.table_query.as_ref()).map(|q| &q.order_keys), Some(OrderKeysState::Asked { .. }))
    }

    /// Profile `pid` connected: its table tabs that wait for their keys ask for them now (or
    /// run without them when they cannot be asked for).
    pub(super) fn table_keys_connected(&mut self, pid: ProfileId) {
        let waiting: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.profile == Some(pid) && t.doc.table_query.as_ref().is_some_and(|q| q.keys_wait.is_some()))
            .map(|t| t.id)
            .collect();
        for id in waiting {
            if self.keys_to_read(id) {
                continue;
            }
            if let Some(q) = self.table_keys_take(id) {
                self.table_run_with(id, q, false);
            }
        }
    }

    /// The query table tab `id` waits to run with its keys, taken (the wait ends): `None` when
    /// none waits, or the one waiting was for another binding of the tab (dropped).
    fn table_keys_take(&mut self, id: TabId) -> Option<Query> {
        let binding = self.tabs.get(id)?.binding;
        let (q, b) = self.table_query_mut(id)?.keys_wait.take()?;
        (b == binding).then_some(q)
    }

    /// Whether table tab `id` has a query waiting for its table's keys (work in progress of
    /// the tab, as a statement waiting for its connection is).
    pub(in crate::app) fn table_keys_waiting(&self, id: TabId) -> bool {
        self.table_query(id).is_some_and(|tq| tq.keys_wait.is_some())
    }

    /// The query table tab `id` waits to run with its keys is cancelled (it never runs, and a
    /// late answer finds nothing to run); whether one waited.
    pub(in crate::app) fn table_keys_cancel(&mut self, id: TabId) -> bool {
        self.table_query_mut(id).is_some_and(|tq| tq.keys_wait.take().is_some())
    }

    /// Table tab `id` holds a query for its session to say its state, and that session is still
    /// opening.
    pub(in crate::app) fn table_session_waiting(&self, id: TabId) -> bool {
        self.tabs.get(id).is_some_and(|t| {
            t.exec.state == SessionState::Opening
                && t.doc
                    .table_query
                    .as_ref()
                    .and_then(|tq| tq.session_wait.as_ref())
                    .is_some_and(|(_, generation)| *generation == t.exec.generation)
        })
    }

    /// The query table tab `id` holds for its session is cancelled (it never runs); whether one
    /// waited.
    pub(in crate::app) fn table_session_cancel(&mut self, id: TabId) -> bool {
        let waiting = self.table_session_waiting(id);
        if let Some(tq) = self.table_query_mut(id) {
            tq.session_wait = None;
        }
        waiting
    }

    /// Query `q` of table tab `id`, applied and not sent yet, is cancelled wherever it waits
    /// (for its profile to connect, its session to say its state, its table's keys, or the
    /// user's word on its functions or on reads that lock rows): it never runs, and a late
    /// answer finds nothing to run. A wait of the tab for another query is left alone. Whether
    /// one waited.
    pub(in crate::app) fn table_cancel_waiting(&mut self, id: TabId, q: &Query) -> bool {
        let Some(tq) = self.table_query_mut(id) else { return false };
        let mut cancelled = false;
        if tq.keys_wait.as_ref().is_some_and(|(w, _)| same_query(w, q)) {
            tq.keys_wait = None;
            cancelled = true;
        }
        if tq.session_wait.as_ref().is_some_and(|(w, _)| same_query(w, q)) {
            tq.session_wait = None;
            cancelled = true;
        }
        if tq.locks_asked.as_ref().is_some_and(|(w, _)| same_query(w, q)) {
            tq.locks_asked = None;
            cancelled = true;
        }
        if tq.asked.as_ref().is_some_and(|w| same_query(w, q)) {
            tq.asked = None;
            tq.asked_open = None;
            cancelled = true;
        }
        // Its statement, waiting for the profile to connect.
        let sql = tq.pending.as_ref().filter(|p| same_query(&p.query, q)).map(|p| p.sql.clone());
        let queued = sql.is_some_and(|sql| {
            let one = std::slice::from_ref(&sql);
            self.conns.iter().any(|(_, c)| c.pending.iter().any(|w| w.tab == id && w.statements == one))
        });
        if queued {
            self.take_queued(id);
            self.unstage_run(id);
            if let Some(tq) = self.table_query_mut(id) {
                tq.pending = None;
            }
            cancelled = true;
        }
        cancelled
    }

    /// Table tab `id`'s session has said its state: the query held for that session runs, checked
    /// from the start against it.
    pub(in crate::app) fn table_session_ready(&mut self, id: TabId) {
        let Some(generation) = self.tabs.get(id).map(|t| t.exec.generation) else { return };
        let Some(tq) = self.table_query_mut(id) else { return };
        if let Some((q, waited)) = tq.session_wait.take()
            && waited == generation
        {
            self.table_run(id, q);
        }
    }

    /// Table tab `id`'s session did not open: the query held for it never runs, and the tab says
    /// so (while its profile connects, only the profile's own failure is said). A session left
    /// opening by that, with nothing running on it, is closed, so that the next run opens another
    /// instead of waiting on it (also when the query held for it was cancelled before).
    pub(in crate::app) fn table_session_failed(&mut self, id: TabId) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        let Some(tq) = t.doc.table_query.as_mut() else { return };
        let waited = tq.session_wait.take().is_some();
        if t.exec.state == SessionState::Opening && t.exec.running.is_none() {
            if let Some(s) = t.exec.session.take() {
                s.close();
            }
            t.exec.state = SessionState::Idle;
        }
        if !waited {
            return;
        }
        let name = t.profile.and_then(|p| self.profile(p)).map(|p| p.name.clone()).unwrap_or_default();
        self.tab_status(id, Notice::new(Msg::QueryQueuedFailed { name }, Level::Warning));
    }

    /// Profile `pid`'s attempt ended without a connection: the queries of its table tabs (in
    /// any database) that waited for their keys are dropped, each tab says so (`name`: the
    /// profile's).
    pub(in crate::app) fn table_keys_dropped(&mut self, pid: ProfileId, name: &str) {
        let waiting: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.profile == Some(pid))
            .map(|t| t.id)
            .filter(|id| self.table_keys_waiting(*id))
            .collect();
        for id in waiting {
            self.table_keys_cancel(id);
            self.tab_status(id, Notice::new(Msg::QueryQueuedFailed { name: name.to_string() }, Level::Warning));
        }
    }

    /// The waits of profile `pid`'s table tabs for their keys, taken (to wait again for the
    /// profile's next attempt: [`App::table_keys_wait_again`]).
    pub(in crate::app) fn table_keys_waits(&mut self, pid: ProfileId) -> Vec<(TabId, Query, u64)> {
        let tabs: Vec<TabId> = self.tabs.iter().filter(|t| t.profile == Some(pid)).map(|t| t.id).collect();
        tabs.into_iter()
            .filter_map(|id| {
                let (q, b) = self.tabs.get_mut(id)?.doc.table_query.as_mut()?.keys_wait.take()?;
                Some((id, q, b))
            })
            .collect()
    }

    /// The waits taken by [`App::table_keys_waits`] wait again, each in its tab while the tab
    /// is still bound as it was.
    pub(in crate::app) fn table_keys_wait_again(&mut self, waits: Vec<(TabId, Query, u64)>) {
        for (id, q, b) in waits {
            if self.tabs.get(id).is_some_and(|t| t.binding == b)
                && let Some(tq) = self.tabs.get_mut(id).and_then(|t| t.doc.table_query.as_mut())
            {
                tq.keys_wait = Some((q, b));
            }
        }
    }

    /// Profile `pid`'s metadata session (in `database`, `None`: its own) is gone or did not
    /// open: its table tabs that wait for their keys stop waiting; with `run`, their query
    /// runs with what is known (else it is left as the draft, the failure said by the profile).
    pub(super) fn table_keys_gone(&mut self, pid: ProfileId, database: Option<&str>, run: bool) {
        let waiting: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.profile == Some(pid) && t.context.database.as_deref() == database)
            .filter(|t| t.doc.table_query.as_ref().is_some_and(|q| q.keys_wait.is_some()))
            .map(|t| t.id)
            .collect();
        for id in waiting {
            let Some(q) = self.table_keys_take(id) else { continue };
            if run {
                self.table_run_with(id, q, false);
            }
        }
    }

    /// Ask for the keys of table tab `id`'s table that make an order total, with the key-only
    /// catalog read (never the full structure, whose functions open the table): unless they
    /// are read or being read on the session as it is now. Nothing is asked without a metadata
    /// session (the next run asks again).
    fn order_keys_ask(&mut self, id: TabId) {
        let Some(t) = self.tabs.get(id) else { return };
        let (Some(pid), Some(table)) = (t.profile, t.doc.table.clone()) else { return };
        if !self.structure_on(pid) {
            return;
        }
        // Another database's metadata session opened (again) first, as the DDL's is.
        if let Some(db) = t.context.database.clone() {
            self.ensure_aux(pid, &db);
        }
        let Some(t) = self.tabs.get(id) else { return };
        let Some(on) = self.keys_on(t) else { return };
        let current = match t.doc.table_query.as_ref().map(|q| &q.order_keys) {
            Some(OrderKeysState::Known { on: o, .. } | OrderKeysState::Asked { on: o, .. }) => *o == on,
            _ => false,
        };
        if current {
            return;
        }
        let session = match on.database.as_deref() {
            None => self.conns.get(pid).and_then(|c| c.meta.as_ref()),
            Some(db) => self.conns.aux(pid, db).and_then(|a| a.session.as_ref()),
        };
        let Some(session) = session else { return };
        self.order_keys_seq += 1;
        let request = self.order_keys_seq;
        session.send(DbCommand::LoadTableKeys { id: request, schema: table.schema, table: table.name });
        if on.database.is_none() {
            self.touch_profile(pid);
        }
        if let Some(tq) = self.table_query_mut(id) {
            tq.order_keys = OrderKeysState::Asked { request, on };
        }
    }

    /// The keys read for the table tabs of profile `profile` may be out of date: its schema may
    /// have changed (DDL ran in the app, the explorer read the schema again). `database`: only
    /// the tabs on that metadata session (`Some(None)`: the profile's own), else every tab of the
    /// profile (DDL may name a table of any database). Their keys are not known until read
    /// again: the next run asks for them (until then no order is taken for total), and an
    /// answer on its way is dropped.
    pub(super) fn forget_order_keys(&mut self, profile: ProfileId, database: Option<Option<&str>>) {
        // What earlier loads showed of the tables' values goes with them.
        self.value_memory.forget(profile, database);
        for t in self.tabs.iter_mut().filter(|t| t.profile == Some(profile)) {
            if database.is_some_and(|db| t.context.database.as_deref() != db) {
                continue;
            }
            if let Some(tq) = t.doc.table_query.as_mut() {
                tq.order_keys = OrderKeysState::Unknown;
            }
        }
        // A query waiting for them waits for keys asked for again.
        let waiting: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.profile == Some(profile) && t.doc.table_query.as_ref().is_some_and(|q| q.keys_wait.is_some()))
            .map(|t| t.id)
            .collect();
        for id in waiting {
            self.order_keys_ask(id);
        }
    }

    /// The answer to keys request `request`, from profile `profile`'s metadata session in
    /// `database` (`None`: its own): kept by the table tab that asked, when the session it
    /// asked on is still the one now and the tab has the same binding; else dropped (a late
    /// answer of an earlier one). A failure leaves the keys not known, never taken for none.
    pub(super) fn order_keys_answered(
        &mut self,
        profile: ProfileId,
        database: Option<&str>,
        request: u64,
        result: Result<datarig_core::driver::keys::OrderKeys, datarig_core::driver::DbError>,
    ) {
        let tab = self.tabs.iter().find(|t| {
            t.doc.table_query.as_ref().is_some_and(|q| {
                matches!(&q.order_keys, OrderKeysState::Asked { request: r, on }
                    if *r == request && on.profile == profile && on.database.as_deref() == database
                        && self.keys_on(t).as_ref() == Some(on))
            })
        });
        let Some(id) = tab.map(|t| t.id) else { return };
        let result = result.map_err(|e| self.db_error_text(&e));
        if let Some(tq) = self.tabs.get_mut(id).and_then(|t| t.doc.table_query.as_mut()) {
            let OrderKeysState::Asked { on, .. } = std::mem::take(&mut tq.order_keys) else { return };
            tq.order_keys = match result {
                Ok(keys) => OrderKeysState::Known { on, keys },
                Err(reason) => OrderKeysState::Failed { on, reason },
            };
        }
        self.filter_dialog_columns_came(id);
        // The query that waited for them runs now (one cancelled meanwhile does not).
        if let Some(q) = self.table_keys_take(id) {
            self.table_run_with(id, q, false);
        }
    }

    /// Ask whether table tab `id`'s filter may call `calls` (the query waits in its state).
    fn ask_filter_calls(&mut self, id: TabId, calls: Vec<String>) {
        self.tab_mut().popup = None;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.overlays.push(Overlay::Confirm(super::overlay::Confirm {
            title: Label::TableFilterCallsTitle,
            text: Msg::TableFilterCallsText { calls: calls.join(", ") },
            details: Vec::new(),
            keys: Label::TableFilterCallsKeys,
            action: ConfirmAction::FilterCalls(id),
            folder: None,
            path: None,
            tx: None,
            buttons: Default::default(),
        }));
    }

    /// `y` to the functions of table tab `id`'s filter: its text is let through in the tab,
    /// and the query runs.
    pub(super) fn filter_calls_confirmed(&mut self, id: TabId) {
        let Some(tq) = self.table_query_mut(id) else { return };
        let Some(q) = tq.asked.take() else { return };
        let open = std::mem::take(&mut tq.asked_open);
        if let (Some(text), Some(tq)) = (filter_text(&q).map(str::to_string), self.table_query_mut(id)) {
            tq.confirmed.insert(text);
        }
        match open {
            Some(OpenIn::Console) => self.table_open_sql(id),
            // Asked on the tab's own `Ctrl+G`: the editor only for the tab still active.
            Some(OpenIn::Editor) if self.tab().id == id => self.request_external_edit(),
            Some(OpenIn::Editor) => {}
            Some(OpenIn::Dialog) => {
                if let Some(sql) = self.table_query_sql_to_open(id, q, OpenIn::Dialog) {
                    self.filter_dialog_opened(id, &sql);
                }
            }
            None => self.table_run(id, q),
        }
    }

    /// Anything but `y` to the functions of table tab `id`'s filter: nothing runs (or opens).
    pub(super) fn filter_calls_declined(&mut self, id: TabId) {
        let Some(tq) = self.table_query_mut(id) else { return };
        tq.asked_open = None;
        if tq.asked.take().is_some() {
            self.tab_status(id, Notice::new(Label::TableFilterCallsCancelled, Level::Warning));
        }
    }

    /// Whether reads of table tab `id` lock rows in its session as it is now, as far as known
    /// (see [`Locks`]), and that state; `None` when they do not: not MySQL, autocommit on with
    /// no transaction open (nor one the run of `sql` opens for manual commit), or a level below
    /// SERIALIZABLE.
    pub(super) fn table_locks(&self, id: TabId, sql: Option<&str>) -> Option<(Locks, LockState)> {
        let state = self.lock_state(id)?;
        let e = &self.tabs.get(id)?.exec;
        let open = e.tx_open || e.in_block;
        // The run opens a transaction for the read itself (manual commit): it reads in one.
        let manual = !open
            && sql.is_some_and(|sql| {
                matches!(self.tx_rule(id, &[sql.to_string()], false),
                    TxRule::Manual { steps, .. } if steps.first() == Some(&TxStep::Open))
            });
        if !(e.autocommit_off || open || manual) {
            return None;
        }
        if e.isolation.is_some_and(|i| i != Isolation::Serializable) {
            return None;
        }
        Some((Locks { known: e.isolation.is_some(), autocommit_off: e.autocommit_off, manual }, state))
    }

    /// Table tab `id`'s read (`statements`, its query's pending statement) right before it is
    /// sent, on the session it goes to as it is now: when it locks rows there and the tab did
    /// not say yes to that in this state, nothing is sent, and the query waits for the user's
    /// word ([`App::table_locks_confirmed`] runs it again from the start). Whether it waits.
    pub(in crate::app) fn table_locks_hold(&mut self, id: TabId, statements: &[String]) -> bool {
        let Some(tq) = self.table_query(id) else { return false };
        let Some(p) = tq.pending.as_ref().filter(|p| statements.len() == 1 && statements[0] == p.sql) else {
            return false;
        };
        let q = p.query.clone();
        // A session still opening has not said its state: statements of its start-up may turn
        // autocommit off, and all that is held for it now is how sessions start. The read
        // waits, and runs from the start once the session is connected.
        if let Some(state) = self.lock_state(id)
            && self.tabs.get(id).is_some_and(|t| t.exec.state == SessionState::Opening)
        {
            if let Some(tq) = self.table_query_mut(id) {
                tq.pending = None;
                tq.session_wait = Some((q, state.generation));
            }
            return true;
        }
        let Some((locks, state)) =
            self.table_locks(id, Some(&statements[0])).filter(|(_, st)| !self.locks_confirmed(id, *st))
        else {
            return false;
        };
        if let Some(tq) = self.table_query_mut(id) {
            tq.pending = None;
            tq.locks_asked = Some((q, state));
        }
        self.ask_table_locks(id, locks);
        true
    }

    /// The state of table tab `id`'s session that decides whether its reads lock rows
    /// ([`LockState`]); `None` when they never do (not MySQL).
    pub(in crate::app) fn lock_state(&self, id: TabId) -> Option<LockState> {
        let t = self.tabs.get(id)?;
        if !matches!(t.exec.prepared.language().dialect(), Dialect::MySql(_)) {
            return None;
        }
        let e = &t.exec;
        Some(LockState {
            generation: e.generation,
            revision: e.safety_rev,
            isolation: e.isolation,
            autocommit_off: e.autocommit_off,
        })
    }

    /// The yes of table tab `id` to reads that lock rows, when it covers the run in flight: the
    /// tab's query run, sent in the state the yes was given in. A transaction that run opens
    /// (autocommit off, manual commit) is the one the yes was for ([`App::table_safety_changed`]).
    pub(in crate::app) fn table_locks_covering(&self, id: TabId) -> Option<LockState> {
        let t = self.tabs.get(id)?;
        let tq = t.doc.table_query.as_ref()?;
        // Only the run itself: never a count, a page, an export or an apply of the same query.
        let running = t.exec.running.as_ref().filter(|r| !(r.fetch || r.count || r.export || r.apply))?.id;
        let state = self.lock_state(id)?;
        (tq.requested.as_ref().is_some_and(|(qid, _)| *qid == running) && tq.locks_confirmed == Some(state))
            .then_some(state)
    }

    /// What decides whether table tab `id`'s reads lock rows may have changed (its session's
    /// [`TabSession::safety_rev`] moves on): a yes given before holds no more, except for a
    /// transaction the run it covered (`covering`, [`App::table_locks_covering`]) opened or
    /// ended itself, with the session, its level and its autocommit as they were.
    pub(in crate::app) fn table_safety_changed(&mut self, id: TabId, covering: Option<LockState>) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        t.exec.safety_rev += 1;
        let Some(now) = self.lock_state(id) else { return };
        let same = covering.is_some_and(|c| {
            (c.generation, c.isolation, c.autocommit_off) == (now.generation, now.isolation, now.autocommit_off)
        });
        if same && let Some(tq) = self.table_query_mut(id) {
            tq.locks_confirmed = Some(now);
        }
    }

    /// The user let reads that lock rows run in table tab `id` in session state `state`.
    fn locks_confirmed(&self, id: TabId, state: LockState) -> bool {
        self.table_query(id).is_some_and(|tq| tq.locks_confirmed == Some(state))
    }

    /// Ask whether table tab `id`'s read may lock rows (`locks`; the query waits in its state).
    fn ask_table_locks(&mut self, id: TabId, locks: Locks) {
        self.tab_mut().popup = None;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        let state = self
            .i18n
            .label(match locks {
                Locks { autocommit_off: true, .. } => Label::TableLocksStateAutocommitOff,
                Locks { manual: true, .. } => Label::TableLocksStateManual,
                _ => Label::TableLocksStateOpenTx,
            })
            .to_string();
        let (title, text) = match locks.known {
            true => (Label::TableLocksTitle, Msg::TableLocksText { state }),
            false => (Label::TableLocksTitleUnknown, Msg::TableLocksTextUnknown { state }),
        };
        self.overlays.push(Overlay::Confirm(super::overlay::Confirm {
            title,
            text,
            details: Vec::new(),
            keys: Label::TableFilterCallsKeys,
            action: ConfirmAction::TableReadLocks(id),
            folder: None,
            path: None,
            tx: None,
            buttons: Default::default(),
        }));
    }

    /// `y` to reads that lock rows in table tab `id`: let through in the tab for the session
    /// state asked in, and the query runs (asked again if that state changed meanwhile).
    pub(super) fn table_locks_confirmed(&mut self, id: TabId) {
        let Some(tq) = self.table_query_mut(id) else { return };
        let Some((q, state)) = tq.locks_asked.take() else { return };
        tq.locks_confirmed = Some(state);
        self.table_run(id, q);
    }

    /// Anything but `y` to reads that lock rows in table tab `id`: nothing runs.
    pub(super) fn table_locks_declined(&mut self, id: TabId) {
        let Some(tq) = self.table_query_mut(id) else { return };
        if tq.locks_asked.take().is_some() {
            self.tab_status(id, Notice::new(Label::TableLocksCancelled, Level::Warning));
        }
    }

    /// Table tab `id`'s statement may run again (a page past a closed portal, a count) as far
    /// as row locks go: `Err` with the reason in words when reads in its session lock rows and
    /// the tab did not say yes to that in this state (never asked here). Neither runs in a
    /// transaction of the app's own for manual commit.
    pub(super) fn table_locks_recheck(&self, id: TabId) -> Result<(), String> {
        if self.table_query(id).is_none() {
            return Ok(());
        }
        match self.table_locks(id, None) {
            Some((_, state)) if !self.locks_confirmed(id, state) => {
                Err(self.i18n.label(Label::TableLocksWhyUnconfirmed).to_string())
            }
            _ => Ok(()),
        }
    }

    /// The shown filter of table tab `id` checked again before a statement built from it runs
    /// again (a page past a closed portal, a count): `Err` with the reason in words when it is
    /// refused now, or calls functions the user did not let run in the tab (never asked here).
    pub(super) fn table_recheck(&self, id: TabId) -> Result<(), String> {
        let Some(text) = self.table_query(id).and_then(|tq| tq.shown_filter()) else { return Ok(()) };
        let (Some(name), Some(target)) = (self.table_name(id), self.table_target(id)) else { return Ok(()) };
        let read_only = self.tabs.get(id).and_then(|t| t.profile).is_some_and(|p| self.read_only(p));
        match query::check_filter(text, &name, &target, &self.classifier(id), read_only) {
            Err(r) => Err(refusal_why(&self.i18n, &r)),
            Ok(FilterVerdict::NeedsConfirm(_))
                if !self.table_query(id).is_some_and(|tq| tq.confirmed.contains(text)) =>
            {
                Err(self.i18n.label(Label::TableFilterWhyUnconfirmed).to_string())
            }
            Ok(_) => Ok(()),
        }
    }

    /// The statement that reads table tab `id`'s shown rows whole when they have a cap (a
    /// count counts around it, an export of every row reads it): the statement the rows were
    /// read with, as it was sent (its order too, whatever keys were read since), with only the
    /// cap written after it as a `LIMIT`. `None` without a cap: the statement the rows came
    /// from reads them.
    pub(super) fn table_capped_sql(&self, id: TabId) -> Option<String> {
        let tq = self.table_query(id)?;
        let q = tq.shown.as_ref().filter(|q| q.limit.is_some())?;
        let d = self.tabs.get(id)?.exec.prepared.language().dialect();
        Some(query::capped(&tq.shown_run()?.sql, q, d))
    }

    /// `:where <condition>`: the active table tab's filter (none when blank), then a run.
    pub(super) fn where_command(&mut self, text: &str) -> Result<(), Notice> {
        let id = self.table_command_tab("where")?;
        // Refused before the draft changes: a busy tab keeps what it would run next.
        if self.tab_busy(id) {
            self.flash_busy();
            return Ok(());
        }
        let Some(target) = self.table_target(id) else { return Ok(()) };
        let filter = (!text.trim().is_empty())
            .then(|| Filter { text: text.to_string(), recognised: query::recognise(text, &target) });
        let Some(tq) = self.table_query_mut(id) else { return Ok(()) };
        tq.draft.filter = filter;
        let q = tq.draft.clone();
        self.table_run(id, q);
        Ok(())
    }

    /// `:orderby <keys>`: the active table tab's order (none when blank), then a run. A text
    /// that is not a list of its columns is refused and said; nothing runs.
    pub(super) fn orderby_command(&mut self, text: &str) -> Result<(), Notice> {
        let id = self.table_command_tab("orderby")?;
        // Refused before the draft changes: a busy tab keeps what it would run next.
        if self.tab_busy(id) {
            self.flash_busy();
            return Ok(());
        }
        let Some(target) = self.table_target(id) else { return Ok(()) };
        if !text.trim().is_empty() && target.columns.is_empty() {
            // Unknown, not absent: no column is said not to exist.
            self.tab_status(id, Notice::new(Label::TableOrderColumnsUnknown, Level::Warning));
            return Ok(());
        }
        match query::parse_order(text, &target) {
            Err(e) => {
                let why = order_why(&self.i18n, &e);
                let notice = Notice::new(Msg::TableOrderRefused { why }, Level::Warning);
                if let Some(tq) = self.table_query_mut(id) {
                    let (at, text) = (Some(e.range), text.to_string());
                    tq.refusal = Some(Refusal { field: Field::Order, at, notice: notice.clone(), text });
                }
                self.tab_status(id, notice);
            }
            Ok(keys) => {
                let Some(tq) = self.table_query_mut(id) else { return Ok(()) };
                tq.draft.order = keys;
                let q = tq.draft.clone();
                self.table_run(id, q);
            }
        }
        Ok(())
    }

    /// `:filter clear`: the active table tab's every row, unordered, then a run.
    pub(super) fn filter_command(&mut self) -> Result<(), Notice> {
        let id = self.table_command_tab("filter")?;
        // Refused before the draft changes: a busy tab keeps what it would run next.
        if self.tab_busy(id) {
            self.flash_busy();
            return Ok(());
        }
        let Some(name) = self.table_name(id) else { return Ok(()) };
        let Some(tq) = self.table_query_mut(id) else { return Ok(()) };
        tq.draft = Query::table(name);
        let q = tq.draft.clone();
        self.table_run(id, q);
        Ok(())
    }

    /// The active tab, when it is a table tab: else the command `name` is not available.
    pub(super) fn table_command_tab(&self, name: &str) -> Result<TabId, Notice> {
        let t = self.tab();
        match t.is_table() && t.doc.table.is_some() {
            true => Ok(t.id),
            false => Err(Notice::new(Msg::CommandsErrorUnavailable { name: name.to_string() }, Level::Error)),
        }
    }
}

#[cfg(test)]
mod tests;

impl App {
    /// A sort action on column `col` of table tab `id`'s grid (`s`, `S`, a click in the
    /// column's sort zone): the order of the rows shown ([`TableQuery::shown`], else what is
    /// applied when the tab shows none) changed by `step` ([`query::sort_column`]), with their
    /// condition and limit, runs as any run of the tab ([`App::table_run`]: checks, asks,
    /// pending edits). Refused while the query applied is not the one of the rows shown (its
    /// run was refused or cancelled: the earlier rows are still shown). The draft takes the new
    /// order only once its run is sent: a run asked about and declined, or refused, leaves them as they
    /// were.
    pub(super) fn table_sort(&mut self, id: TabId, col: usize, step: query::SortStep) {
        let name = match self.tabs.get(id).map(|t| &t.results) {
            Some(Results::Rows(rs)) => rs.columns.get(col).map(|c| c.meta.name.clone()),
            _ => None,
        };
        let Some(name) = name else {
            return self.flash(Notice::new(Label::TableSortNoColumns, Level::Info));
        };
        if self.tab_busy(id) {
            return self.flash_busy();
        }
        // A query applied whose rows are not the ones shown stays the user's: the sort would run
        // from rows of another query.
        if !self.rows_shown_applied(id) {
            return self.flash(Notice::new(Label::TableSortNotShown, Level::Warning));
        }
        let d = self.tab_dialect(id);
        let Some(tq) = self.table_query(id) else { return };
        let base = tq.shown.clone().unwrap_or_else(|| tq.applied().clone());
        let q = Query { order: query::sort_column(&base.order, &name, step), ..base };
        // The order is shown and edited on one line: a name with a line break would not read
        // back as the same name there.
        let user: Vec<_> = q.order.iter().filter(|k| k.origin == query::KeyOrigin::User).cloned().collect();
        if !crate::widgets::text_input::TextInput::holds(&query::order_text(&user, d)) {
            let held = |k: &&query::OrderKey| crate::widgets::text_input::TextInput::holds(&k.col);
            let column = user.iter().find(|k| !held(k)).map_or(name, |k| k.col.clone());
            let column = crate::text::escape_controls(&column);
            return self.flash(Notice::new(Msg::TableSortName { column }, Level::Warning));
        }
        let Some(tq) = self.table_query_mut(id) else { return };
        tq.sorted = Some(q.clone());
        self.table_run(id, q);
    }

    /// Whether a grid action on table tab `id` (a sort, a filter by value), which runs from the
    /// rows shown, runs from the query applied: not while that query's run was refused,
    /// cancelled, or ended without rows (the rows shown are an earlier query's).
    pub(super) fn rows_shown_applied(&self, id: TabId) -> bool {
        self.table_query(id).is_none_or(TableQuery::shows_applied)
    }

    /// What the headers of table tab `id`'s grid say of the rows shown, by result column: the
    /// user's sort keys (the keys appended to make the order total are not marked: the dialog
    /// shows them), and the columns the condition compares as far as the app read it (a
    /// condition it did not read marks none: no mark is no claim). Empty when the tab shows no
    /// rows of its query.
    pub fn table_head_marks(&self, id: TabId) -> Vec<crate::widgets::grid::HeadMarks> {
        let Some(t) = self.tabs.get(id).filter(|t| t.is_table()) else { return Vec::new() };
        let (Results::Rows(rs), Some(shown)) = (&t.results, t.doc.table_query.as_ref().and_then(|q| q.shown.as_ref()))
        else {
            return Vec::new();
        };
        let keys: Vec<&query::OrderKey> = shown.order.iter().filter(|k| k.origin == query::KeyOrigin::User).collect();
        let compared: Vec<&str> = match &shown.filter {
            Some(f) if !f.text.trim().is_empty() => f.recognised.as_ref().map(|c| c.columns()).unwrap_or_default(),
            _ => Vec::new(),
        };
        rs.columns
            .iter()
            .map(|c| {
                let name = c.meta.name.as_str();
                let sort = keys.iter().position(|k| k.col == name).map(|i| (keys[i].dir == query::Dir::Asc, i + 1));
                crate::widgets::grid::HeadMarks { sort, filtered: compared.contains(&name) }
            })
            .collect()
    }
}
