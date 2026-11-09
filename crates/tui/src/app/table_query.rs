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

use super::*;
use datarig_core::driver::structure::TableStructure;
use datarig_core::driver::{ColumnMeta, ValueKind};
use datarig_core::query::{
    self, BarRefusal, ColumnFacts, ColumnKind, Filter, FilterVerdict, LexProblem, OrderError, OrderProblem, Query,
    ShapeProblem, TableName, Target,
};
use std::collections::HashSet;
use std::ops::Range;

/// A part of the table tab's query the user writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Where,
    Order,
}

/// Why the app did not run what the user wrote: the part, where in its text (bytes) when
/// known, and what the status line said.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    pub field: Field,
    pub at: Option<Range<usize>>,
    pub notice: Notice,
}

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
    pending: Option<(Query, String)>,
    /// The query waiting for the user's word on the functions it calls.
    asked: Option<Query>,
    /// The filter texts whose functions the user let run in this tab.
    confirmed: HashSet<String>,
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
            asked: None,
            confirmed: HashSet::new(),
        }
    }

    /// Run `qid` sends `statements`: it is the pending query's run when it sends exactly its
    /// statement, else the run of something else.
    pub(super) fn sent(&mut self, qid: u64, statements: &[String]) {
        self.requested = match self.pending.take() {
            Some((q, sql)) if statements.len() == 1 && statements[0] == sql => Some((qid, q)),
            _ => None,
        };
    }

    /// The first page of run `qid` landed: the rows are of its query, when it was one.
    pub(super) fn first_page(&mut self, qid: u64) {
        self.shown = self.requested.as_ref().filter(|(id, _)| *id == qid).map(|(_, q)| q.clone());
    }

    /// The text of the shown filter, when it has one.
    pub fn shown_filter(&self) -> Option<&str> {
        filter_text(self.shown.as_ref()?)
    }
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

/// The kind of a result column, as the query model writes literals for it.
fn column_kind(c: &ColumnMeta) -> ColumnKind {
    match c.kind {
        ValueKind::Integer | ValueKind::Float => ColumnKind::Number,
        ValueKind::Decimal if c.type_name != "money" => ColumnKind::Number,
        ValueKind::Text => ColumnKind::Text,
        ValueKind::Bytes => ColumnKind::Bytes,
        _ => ColumnKind::Other,
    }
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
    fn table_query_mut(&mut self, id: TabId) -> Option<&mut TableQuery> {
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

    /// What the app knows of table tab `id`'s table for writing its SQL: the columns of the
    /// rows it holds of its query (else the catalog's names, of unknown kind), with the
    /// structure when it was read (collations, the primary key and the unique keys whose
    /// columns are all `NOT NULL`). Until the structure is known no key is: no tie-breaker is
    /// appended and no order is taken for total.
    pub(crate) fn table_target(&self, id: TabId) -> Option<Target> {
        let t = self.tabs.get(id).filter(|t| t.is_table())?;
        let table = t.doc.table.as_ref()?;
        let d = self.tab_dialect(id);
        let held = t.doc.table_query.as_ref().is_some_and(|q| q.shown.is_some());
        let mut columns: Vec<ColumnFacts> = match &t.results {
            Results::Rows(rs) if held => {
                rs.columns.iter().map(|c| ColumnFacts::new(c.meta.name.clone(), column_kind(&c.meta))).collect()
            }
            _ => self
                .conns
                .catalog_in(t.profile, t.context.database.as_deref())
                .relations
                .iter()
                .find(|r| r.schema == table.schema && r.name == table.name)
                .map(|r| r.columns.iter().map(|c| ColumnFacts::new(c.name.clone(), ColumnKind::Other)).collect())
                .unwrap_or_default(),
        };
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
                    c.deterministic = coll.deterministic;
                }
            }
            // Rows of tables that inherit from it are read too: its keys do not hold across them.
            if !s.inherited {
                target.primary_key = s.primary_key.as_ref().map(|k| k.columns.clone()).unwrap_or_default();
                let not_null = |c: &String| s.columns.iter().any(|sc| &sc.name == c && sc.not_null);
                target.unique_keys = s
                    .unique_constraints
                    .iter()
                    .filter(|k| !k.columns.is_empty() && k.columns.iter().all(not_null))
                    .map(|k| k.columns.clone())
                    .collect();
            }
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
    /// said, kept for the line and nothing is sent); calls of functions whose effects are not
    /// known confirmed first (once per text in the tab); then its statement runs as any run
    /// does ([`App::run_in`]: pending edits asked about, read-only and classifier checks).
    pub(crate) fn table_run(&mut self, id: TabId, mut q: Query) {
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
                Err(r) => {
                    let why = refusal_why(&self.i18n, &r);
                    let notice = Notice::new(Msg::TableFilterRefused { why }, Level::Warning);
                    if let Some(tq) = self.table_query_mut(id) {
                        tq.refusal = Some(Refusal { field: Field::Where, at: refusal_at(&r), notice: notice.clone() });
                    }
                    return self.tab_status(id, notice);
                }
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
                    }
                    return self.ask_filter_calls(id, calls);
                }
                Ok(_) => {}
            }
        }
        let sql = query::compile(&q, &target).sql;
        if let Some(tq) = self.table_query_mut(id) {
            tq.refusal = None;
            tq.pending = Some((q, sql.clone()));
        }
        self.run_in(id, vec![sql]);
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
        let Some(q) = self.table_query_mut(id).and_then(|tq| tq.asked.take()) else { return };
        if let (Some(text), Some(tq)) = (filter_text(&q).map(str::to_string), self.table_query_mut(id)) {
            tq.confirmed.insert(text);
        }
        self.table_run(id, q);
    }

    /// Anything but `y` to the functions of table tab `id`'s filter: nothing runs.
    pub(super) fn filter_calls_declined(&mut self, id: TabId) {
        if self.table_query_mut(id).and_then(|tq| tq.asked.take()).is_some() {
            self.tab_status(id, Notice::new(Label::TableFilterCallsCancelled, Level::Warning));
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

    /// The statement a count of table tab `id`'s shown rows counts around: the shown query
    /// with its cap written when it has one, else the statement the rows came from.
    pub(super) fn table_count_source(&self, id: TabId) -> Option<String> {
        let q = self.table_query(id)?.shown.as_ref().filter(|q| q.limit.is_some())?;
        Some(query::compile_capped(q, &self.table_target(id)?).sql)
    }

    /// `:where <condition>`: the active table tab's filter (none when blank), then a run.
    pub(super) fn where_command(&mut self, text: &str) -> Result<(), Notice> {
        let id = self.table_command_tab("where")?;
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
                    tq.refusal = Some(Refusal { field: Field::Order, at: Some(e.range), notice: notice.clone() });
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
