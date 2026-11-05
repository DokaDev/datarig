//! A result's pending changes as they are shown: how many of each kind, every reason they
//! cannot be applied yet (each with the row and the column it is about), one line per changed
//! cell, and the statements that apply them ([`ChangesModel`]).
//!
//! The model is made from the changes and what was counted for them ([`Checks`], kept with the
//! result), and kept with the result while neither moves: it is not made again for every frame.
//! What else it reads is read as it is when it is made, also while the preview is open: the
//! server's time zone and the session's values the rows were read in (which say whether a value
//! as read is shown whole).

use super::preview::{PreviewLine, VALUE_W, literals};
use super::*;
use datarig_core::driver::ReadIn;
use datarig_core::edit::{Choice, Facts, Origin, Plan, PlanError, RowEdit};
use datarig_core::i18n::{Lang, Localized};
use std::cell::{Cell as Count, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What was counted for a result's changes, and the user's choices of identical rows. Changed
/// only by its methods, each of which moves its version.
#[derive(Debug, Default)]
pub struct Checks {
    asked: Vec<(u64, usize)>,
    matches: BTreeMap<usize, u64>,
    choices: BTreeMap<usize, Choice>,
    failed: Option<String>,
    for_revision: u64,
    /// Moves on every change of these.
    version: u64,
}

impl Checks {
    /// Counts asked and not answered yet: their id and the row each counts.
    pub fn asked(&self) -> &[(u64, usize)] {
        &self.asked
    }

    /// The rows of the table each changed row matched.
    pub fn matches(&self) -> &BTreeMap<usize, u64> {
        &self.matches
    }

    /// The user's choices of rows that match several identical rows.
    pub fn choices(&self) -> &BTreeMap<usize, Choice> {
        &self.choices
    }

    /// Why the rows could not be counted.
    pub fn failed(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// The revision of the changes they are for ([`EditBatch::revision`]).
    pub fn for_revision(&self) -> u64 {
        self.for_revision
    }

    /// Asked again for the changes at `revision`: counts `asked`, nothing answered or chosen.
    pub(super) fn restart(&mut self, revision: u64, asked: Vec<(u64, usize)>) {
        *self = Checks { asked, for_revision: revision, version: self.version + 1, ..Checks::default() };
    }

    /// The answer to count `qid`: the rows it matched, or why it failed (the other counts are
    /// then not waited for).
    pub(super) fn answered(&mut self, qid: u64, result: Result<u64, Option<String>>) {
        let Some(k) = self.asked.iter().position(|(id, _)| *id == qid) else { return };
        let (_, row) = self.asked.remove(k);
        match result {
            Ok(n) => {
                self.matches.insert(row, n);
            }
            Err(text) => {
                self.failed = text;
                self.asked.clear();
            }
        }
        self.version += 1;
    }

    /// The user's choice for row `row`, which matches several identical rows.
    pub(super) fn choose(&mut self, row: usize, choice: Choice) {
        self.choices.insert(row, choice);
        self.version += 1;
    }
}

/// Where the plan of a result's changes stands.
#[derive(Debug)]
pub(super) enum Planned {
    Counting,
    Failed(String),
    /// New rows' cells left to a default their column lacks (`PlanError::NoDefault`,
    /// `PlanError::NotShown`).
    NoDefault(Vec<PlanError>),
    Refused(PlanError),
    Ready(Plan),
    /// Applied: the statements sent.
    Sent(Vec<edit::Step>),
}

/// A reason the changes cannot be applied yet, with the row (and the column) it is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// In words, the row named as the list names it (`new 1 · email is NOT NULL …`).
    pub text: String,
    /// In words without the row's name, as it is listed under its row.
    pub bare: String,
    pub at: Option<(RowAt, Option<usize>)>,
    /// A row that matches several identical rows: the Apply asks which.
    pub choice: bool,
}

/// What a line of the changes does to its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Changed,
    Inserted,
    Deleted,
}

/// How a line names its row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pointer {
    /// By its key's values as read (`id=16`, `a=1,b=2`).
    Key(String),
    /// A row found by its values: its row in the grid, counted from 1.
    Row(usize),
    /// A new row: which one, counted from 1 in the order the grid shows them.
    New(usize),
}

/// A line of the Changes tab's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListEntry {
    /// Problem `k` of the model.
    Problem(usize),
    /// Line `k` of the model's changed cells.
    Line(usize),
    /// The name of a row with problems and no changed cell listed.
    Row(RowAt, Pointer),
}

/// One changed cell (a whole row for a delete and a new row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListLine {
    pub mark: Mark,
    pub at: RowAt,
    /// The cell's column (`None`: the whole row).
    pub col: Option<usize>,
    /// The row's name, on its first line only.
    pub row: Option<Pointer>,
    /// The column's name (empty for the whole row).
    pub column: String,
    /// The value as read, as a literal.
    pub old: Option<String>,
    /// The value it is to be, as a literal; a new row's cells as `column=value · …`.
    pub new: Option<String>,
}

/// A result's pending changes as they are shown.
#[derive(Debug)]
pub struct ChangesModel {
    pub title: Localized,
    /// How many of each kind (`2 updates · 1 insert`).
    pub summary: String,
    /// Every reason they cannot be applied yet, the count's failure first.
    pub problems: Vec<Problem>,
    /// The changed cells: deletes, updates and inserts, as the plan orders them.
    pub list: Vec<ListLine>,
    /// The lines of the list, in order ([`entries`]).
    pub entries: Vec<ListEntry>,
    /// The statements as the preview shows them, or why there are none yet.
    pub sql: Vec<PreviewLine>,
    /// The statements' dialect (their colors).
    pub dialect: Dialect,
    pub(super) planned: Planned,
    /// Why the changes cannot be applied now, in words (`None`: they can).
    pub(super) why_not: Option<String>,
}

/// What a model was made for: the result, the revision of its changes, of what was counted, and
/// what its words and the values shown whole depend on.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelKey {
    result: u64,
    revision: u64,
    checks: u64,
    format: EditFormat,
    lang: Lang,
    /// The rows the grid shows before the new rows.
    before_new: usize,
    zone: Option<String>,
    /// Made from what an Apply changed (no change pending).
    applied: bool,
    /// The tab's dialect and the session's values the rows were read in: whether a value as read
    /// is shown whole.
    dialect: Dialect,
    read_in: ReadIn,
    /// The NULL token a paste reads (an empty pasted value's problem names it).
    null: datarig_core::config::PasteNull,
}

/// A result's changes model as last made.
#[derive(Debug, Default)]
pub struct ModelCache {
    made: RefCell<Option<(ModelKey, EditTarget, Arc<ChangesModel>)>>,
    /// How many times a model was made: read only by tests, which see from it whether the
    /// model was kept.
    builds: Count<u64>,
}

impl ModelCache {
    /// How many times a model was made (for tests: whether the model was kept).
    pub fn builds(&self) -> u64 {
        self.builds.get()
    }
}

/// The cells (row, column) of `rs`'s changed rows whose value as read is not shown whole.
fn not_whole(d: Dialect, rs: &ResultSet, zone: Option<&str>) -> BTreeSet<(usize, usize)> {
    let mut out = BTreeSet::new();
    for (row, e) in rs.edit.batch.rows() {
        for (c, v) in e.orig.iter().enumerate() {
            if let Some(v) = v
                && c < rs.columns.len()
                && !shown_whole(d, rs, c, v, zone)
            {
                out.insert((row, c));
            }
        }
    }
    out
}

/// `v` as a line of the changes says it: a text quoted and cut as the preview's values are.
fn literal(v: &Value) -> String {
    match v {
        Value::Text(t) => literals::quoted_with(t, VALUE_W, crate::text::escape_controls),
        Value::Null => "NULL".into(),
        Value::Default => "DEFAULT".into(),
    }
}

/// The longest a key pointer is, in columns.
const POINTER_W: usize = 24;

/// Column `c` of `names` as the list names it: its control characters spelled out.
fn column_name(names: &[String], c: usize) -> String {
    crate::text::escape_controls(names.get(c).map_or("", String::as_str))
}

/// How the list names read row `row` of `target` (`orig`: its values as read; `names`: the
/// result's column names): by its key's values, or its row in the grid.
fn read_pointer(target: &EditTarget, names: &[String], row: usize, orig: &[Cell]) -> Pointer {
    match &target.identity {
        Identity::Key { columns, .. } => {
            let parts: Vec<String> = columns
                .iter()
                .map(|&c| {
                    let v = orig.get(c).cloned().flatten();
                    let v = v.as_deref().map_or("NULL".into(), crate::text::escape_controls);
                    format!("{}={v}", column_name(names, c))
                })
                .collect();
            Pointer::Key(crate::text::clip(&parts.join(","), POINTER_W))
        }
        Identity::Values { .. } => Pointer::Row(row + 1),
    }
}

/// How the list names row `at` of `batch`'s changes to `target` (`names`: the result's column
/// names); a row read with no change is named by its row in the grid.
pub(super) fn row_pointer(target: &EditTarget, batch: &EditBatch, names: &[String], at: RowAt) -> Pointer {
    match at {
        RowAt::Read(r) => batch.row(r).map_or(Pointer::Row(r + 1), |e| read_pointer(target, names, r, &e.orig)),
        RowAt::New(id) => Pointer::New(batch.drafts().iter().position(|d| d.id == id).map_or(0, |i| i + 1)),
    }
}

/// Pointer `p` in words (`id=5`, `row 31`, `new 1`).
pub fn pointer_text(i18n: &datarig_core::i18n::I18n, p: &Pointer) -> String {
    match p {
        Pointer::Key(k) => k.clone(),
        Pointer::Row(n) => i18n.msg(&Msg::ChangesRow { n: n.to_string() }).to_string(),
        Pointer::New(n) => i18n.msg(&Msg::ChangesNew { n: n.to_string() }).to_string(),
    }
}

/// The lines of the list of `list` and `problems`: the problems about no row first, then each
/// row's lines with its problems under them; a row with problems and no line of its own is
/// named (`pointer`) on a line before them.
pub(super) fn entries(problems: &[Problem], list: &[ListLine], pointer: impl Fn(RowAt) -> Pointer) -> Vec<ListEntry> {
    let of = |at: RowAt| {
        (problems.iter().enumerate())
            .filter(move |(_, p)| p.at.map(|a| a.0) == Some(at))
            .map(|(k, _)| ListEntry::Problem(k))
    };
    let mut out: Vec<ListEntry> =
        problems.iter().enumerate().filter(|(_, p)| p.at.is_none()).map(|(k, _)| ListEntry::Problem(k)).collect();
    let mut listed = BTreeSet::new();
    let mut i = 0;
    while i < list.len() {
        let at = list[i].at;
        let end = i + list[i..].iter().take_while(|l| l.at == at).count();
        out.extend((i..end).map(ListEntry::Line));
        out.extend(of(at));
        listed.insert(at);
        i = end;
    }
    for p in problems {
        if let Some((at, _)) = p.at
            && listed.insert(at)
        {
            out.push(ListEntry::Row(at, pointer(at)));
            out.extend(of(at));
        }
    }
    out
}

/// The lines of `batch`'s changes to the rows of `target` (`names`: the result's column names):
/// deletes, then updates, then new rows; one line per changed cell, the row named on its first.
/// A new row is one line: the values set, then how many columns are left to their default.
pub(super) fn list_lines(target: &EditTarget, batch: &EditBatch, names: &[String]) -> Vec<ListLine> {
    let name = |c: usize| column_name(names, c);
    let pointer = |row: usize, e: &RowEdit| read_pointer(target, names, row, &e.orig);
    let mut out = Vec::new();
    for (row, e) in batch.rows().filter(|(_, e)| e.delete) {
        let (at, row) = (RowAt::Read(row), Some(pointer(row, e)));
        out.push(ListLine { mark: Mark::Deleted, at, col: None, row, column: String::new(), old: None, new: None });
    }
    for (row, e) in batch.rows().filter(|(_, e)| !e.delete) {
        for (k, (&c, v)) in e.set.iter().enumerate() {
            let old = Some(e.orig.get(c).cloned().flatten().map_or(Value::Null, Value::Text));
            out.push(ListLine {
                mark: Mark::Changed,
                at: RowAt::Read(row),
                col: Some(c),
                row: (k == 0).then(|| pointer(row, e)),
                column: name(c),
                old: old.as_ref().map(literal),
                new: Some(literal(v)),
            });
        }
    }
    for (i, d) in batch.drafts().iter().enumerate() {
        let mut cells: Vec<String> = (d.cells.iter().enumerate())
            .filter(|(_, v)| **v != Value::Default)
            .map(|(c, v)| format!("{}={}", name(c), literal(v)))
            .collect();
        // A computed column takes no value: it is not left to a default.
        let left = (d.cells.iter().enumerate())
            .filter(|(c, v)| **v == Value::Default && target.columns.get(*c).is_some_and(Option::is_some))
            .count();
        if left > 0 {
            cells.push(format!("DEFAULT \u{D7}{left}"));
        }
        out.push(ListLine {
            mark: Mark::Inserted,
            at: RowAt::New(d.id),
            col: None,
            row: Some(Pointer::New(i + 1)),
            column: String::new(),
            old: None,
            new: Some(cells.join(" \u{B7} ")),
        });
    }
    out
}

impl App {
    /// The model of the active tab's shown result's changes, in the format its server takes.
    pub fn shown_changes(&self) -> Option<Arc<ChangesModel>> {
        if self.tabs.is_empty() {
            return None;
        }
        let t = self.tab();
        let rs = rows_of(t)?;
        self.changes_model(t.id, rs.id, self.tab_edit_format(t.id)?)
    }

    /// The model of tab `tab`'s shown result's changes, while it is result `result` and its target
    /// is resolved, in format `format`; made again only when the changes, what was counted for
    /// them, the target, the language, the rows shown before the new rows, the server's time
    /// zone or the session's values the rows were read in moved.
    pub(crate) fn changes_model(&self, tab: TabId, result: u64, format: EditFormat) -> Option<Arc<ChangesModel>> {
        let t = self.tabs.get(tab)?;
        let rs = rows_of(t).filter(|rs| rs.id == result)?;
        let Target::Ready(target) = &rs.edit.target else { return None };
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        let key = ModelKey {
            result,
            revision: rs.edit.batch.revision(),
            checks: rs.edit.checks.version,
            format,
            lang: self.i18n.lang,
            before_new: rs.rows.len() + rs.edit.applied_new(),
            zone: zone.map(str::to_string),
            applied: rs.edit.batch.is_empty() && rs.edit.applied.is_some(),
            dialect: self.tab_dialect(t.id),
            read_in: rs.read_in.clone(),
            null: self.prefs.paste_null,
        };
        let cache = &rs.edit.changes;
        if let Some((k, x, m)) = &*cache.made.borrow()
            && *k == key
            && *x == **target
        {
            return Some(m.clone());
        }
        let m = Arc::new(self.make_changes(t, rs, target, format, zone));
        cache.builds.set(cache.builds.get() + 1);
        *cache.made.borrow_mut() = Some((key, (**target).clone(), m.clone()));
        Some(m)
    }

    /// The cells (row, column) of `rs`'s changed rows (shown in tab `t`) whose value as read is
    /// not shown whole.
    pub(super) fn cells_not_whole(&self, t: &Tab, rs: &ResultSet) -> BTreeSet<(usize, usize)> {
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        not_whole(self.tab_dialect(t.id), rs, zone)
    }

    fn make_changes(
        &self,
        t: &Tab,
        rs: &ResultSet,
        target: &EditTarget,
        format: EditFormat,
        zone: Option<&str>,
    ) -> ChangesModel {
        let i18n = &self.i18n;
        // Nothing pending after an Apply: what it changed, and the statements it sent.
        let applied = rs.edit.applied.as_deref().filter(|_| rs.edit.batch.is_empty());
        let batch = applied.map_or(&rs.edit.batch, |a| &a.batch);
        let table = format!("{}.{}", target.table.schema, target.table.name);
        let title = i18n.msg(&Msg::EditPreviewTitle { table });
        let c = batch.counts();
        let summary = [
            (c.updates > 0).then_some(Msg::EditPreviewUpdates { count: c.updates as u64 }),
            (c.inserts > 0).then_some(Msg::EditPreviewInserts { count: c.inserts as u64 }),
            (c.deletes > 0).then_some(Msg::EditPreviewDeletes { count: c.deletes as u64 }),
        ]
        .into_iter()
        .flatten()
        .map(|m| i18n.msg(&m).to_string())
        .collect::<Vec<_>>()
        .join(" · ");
        let f = edit::format(format);
        let dialect = f.language().dialect();
        let names: Vec<String> = rs.columns.iter().map(|c| c.meta.name.clone()).collect();
        if let Some(a) = applied {
            let list = list_lines(target, batch, &names);
            let entries = entries(&[], &list, |at| row_pointer(target, batch, &names, at));
            let sql = self.step_lines(&a.sent, dialect);
            let planned = Planned::Sent(a.sent.clone());
            let why_not = Some(i18n.label(Label::EditWhyApplied).to_string());
            let problems = Vec::new();
            return ChangesModel { title, summary, problems, list, entries, sql, dialect, planned, why_not };
        }
        let not_whole = not_whole(self.tab_dialect(t.id), rs, zone);
        let whole = |r: usize, c: usize| !not_whole.contains(&(r, c));
        // What was counted for other changes is not theirs.
        let none = Checks::default();
        let checks = if rs.edit.checks.for_revision == batch.revision() { &rs.edit.checks } else { &none };
        // The invalid cells and the revision they were found for, from one snapshot.
        let invalid = self.invalid_cells(t, rs);
        let facts = Facts {
            whole: &whole,
            matches: &checks.matches,
            choices: &checks.choices,
            invalid: &invalid.set,
            invalid_for: invalid.revision,
        };
        let planned = planned(target, batch, &*f, checks, &facts);
        let words = |e: &PlanError| self.plan_error_text(rs, &checks.matches, &invalid, e);
        let text = |e: &PlanError| words(e).0;
        // Every row's problem; rows still to be counted are none.
        let mut problems = Vec::new();
        if let Some(error) = &checks.failed {
            let text = i18n.msg(&Msg::EditPreviewCountFailed { error: error.clone() }).to_string();
            let text = crate::text::escape_controls(&text);
            problems.push(Problem { bare: text.clone(), text, at: None, choice: false });
        }
        let all = edit::plan_problems(target, batch, &*f, &facts);
        let refused = match &planned {
            Planned::Refused(e) if !all.contains(e) => Some(e),
            _ => None,
        };
        for e in all.iter().filter(|e| !matches!(e, PlanError::NotCounted { .. })).chain(refused) {
            let choice = matches!(e, PlanError::Choose { .. });
            let (text, bare) = words(e);
            problems.push(Problem { text, bare, at: problem_at(e), choice });
        }
        let list = list_lines(target, batch, &names);
        let entries = entries(&problems, &list, |at| row_pointer(target, batch, &names, at));
        let mut sql = Vec::new();
        let why_not = match &planned {
            Planned::Counting => Some(i18n.label(Label::EditPreviewCounting).to_string()),
            Planned::Failed(error) => Some(i18n.msg(&Msg::EditPreviewCountFailed { error: error.clone() }).to_string()),
            Planned::NoDefault(errors) => Some(text(&errors[0])),
            Planned::Refused(e) => Some(text(e)),
            Planned::Ready(_) | Planned::Sent(_) => None,
        };
        match &planned {
            Planned::Counting => sql.push(PreviewLine::Note(i18n.label(Label::EditPreviewCounting).to_string())),
            Planned::Failed(error) => sql.push(PreviewLine::Problem(
                i18n.msg(&Msg::EditPreviewCountFailed { error: error.clone() }).to_string(),
            )),
            Planned::NoDefault(errors) => {
                for e in errors {
                    sql.push(PreviewLine::Problem(text(e)));
                }
            }
            Planned::Refused(e @ PlanError::Choose { matches, .. }) => {
                let keys = i18n.msg(&Msg::EditPreviewChooseKeys { count: *matches }).to_string();
                sql.push(PreviewLine::Choose { text: text(e), keys });
            }
            Planned::Refused(e) => sql.push(PreviewLine::Problem(text(e))),
            Planned::Ready(plan) => sql = self.step_lines(&plan.steps, dialect),
            Planned::Sent(steps) => sql = self.step_lines(steps, dialect),
        }
        ChangesModel { title, summary, problems, list, entries, sql, dialect, planned, why_not }
    }

    /// Statements `steps` (in dialect `dialect`) as the preview shows them: the row checks, then
    /// the writes, each with the count it must give and the text values it carries read back.
    fn step_lines(&self, steps: &[edit::Step], dialect: Dialect) -> Vec<PreviewLine> {
        let i18n = &self.i18n;
        let mut sql = Vec::new();
        let checks: Vec<&edit::Step> = steps.iter().filter(|s| matches!(s.expect, Expect::Rows(_))).collect();
        if !checks.is_empty() {
            let nowait = checks.iter().all(|s| s.sql.ends_with(" NOWAIT"));
            let head = if nowait { Label::EditPreviewChecks } else { Label::EditPreviewChecksWait };
            sql.push(PreviewLine::Head(i18n.label(head).to_string()));
        }
        let mut writes = false;
        for s in steps {
            let n = match s.expect {
                Expect::Rows(n) => n,
                Expect::Affected(n) => {
                    if !writes {
                        writes = true;
                        sql.push(PreviewLine::Head(i18n.label(Label::EditPreviewWrites).to_string()));
                    }
                    n
                }
            };
            let delete = matches!(s.expect, Expect::Affected(_)) && s.sql.starts_with("DELETE ");
            sql.push(PreviewLine::Step {
                sql: s.sql.clone(),
                expect: i18n.msg(&Msg::EditPreviewExpect { count: n }).to_string(),
                risky: delete || n > 1,
            });
            if let Some(text) = self.read_back(&s.sql, dialect) {
                sql.push(PreviewLine::Values(text));
            }
        }
        sql
    }

    /// Row `at` of `rs` named as the Changes tab's list names it (`id=5`, `row 31`, `new 1`).
    pub(super) fn row_name(&self, rs: &ResultSet, at: RowAt) -> String {
        let p = match &rs.edit.target {
            Target::Ready(target) => {
                let names: Vec<String> = rs.columns.iter().map(|c| c.meta.name.clone()).collect();
                row_pointer(target, &rs.edit.batch, &names, at)
            }
            _ => Pointer::Row(shown_row(rs, at).map_or(0, |r| r + 1)),
        };
        pointer_text(&self.i18n, &p)
    }

    /// Problem `problem` (in words, its control characters spelled out) of row `at` of `rs`,
    /// the row named as the list names it.
    pub(super) fn at_row(&self, rs: &ResultSet, at: RowAt, problem: &str) -> String {
        let row = self.row_name(rs, at);
        self.i18n.msg(&Msg::EditPreviewAtRow { row, problem: problem.to_string() }).to_string()
    }

    /// The name of result column `c` of `rs`.
    pub(super) fn preview_column(&self, rs: &ResultSet, c: usize) -> String {
        rs.columns.get(c).map(|c| c.meta.name.clone()).unwrap_or_default()
    }

    /// Why the plan of `rs`'s changes was refused (`matches`: the rows counted), in words: with
    /// the row named as the list names it, and without it (the same when it is about no row).
    /// Control characters are spelled out.
    fn plan_error_text(
        &self,
        rs: &ResultSet,
        matches: &BTreeMap<usize, u64>,
        invalid: &super::Invalids,
        e: &PlanError,
    ) -> (String, String) {
        let column = |c: usize| self.preview_column(rs, c);
        let msg = match e {
            PlanError::Invalid { origin, column: c } => {
                let at = match *origin {
                    Origin::Row(r) => RowAt::Read(r),
                    Origin::New(id) => RowAt::New(id),
                };
                match invalid.cells.get(&(at, *c)).cloned() {
                    Some(reason) => Msg::EditPreviewInvalidWhy { column: column(*c), reason },
                    None => Msg::EditPreviewInvalid { column: column(*c) },
                }
            }
            PlanError::Column { column: c, why, .. } => column_reason(*why, column(*c)),
            PlanError::NotWhole { column: c, .. } => Msg::EditPreviewNotWhole { column: column(*c) },
            PlanError::NotCounted { .. } => Label::EditPreviewNotCounted.into(),
            PlanError::Gone { .. } => Label::EditPreviewGone.into(),
            PlanError::Ambiguous { matches, .. } => Msg::EditPreviewAmbiguous { count: *matches },
            PlanError::Choose { matches, .. } => Msg::EditPreviewChoose { count: *matches },
            PlanError::NoOneOfK { row: r } => {
                let count = matches.get(r).copied().unwrap_or(0);
                Msg::EditPreviewNoOneOfK { count }
            }
            PlanError::NoDefault { column: c, .. } => Msg::EditPreviewNoDefault { column: column(*c) },
            PlanError::NotShown { column, .. } => Msg::EditPreviewNotShown { column: column.clone() },
            PlanError::Internal(_) => Label::EditPreviewInternal.into(),
        };
        let bare = crate::text::escape_controls(&self.i18n.msg(&msg).to_string());
        match problem_at(e) {
            Some((at, _)) => (self.at_row(rs, at, &bare), bare),
            None => (bare.clone(), bare),
        }
    }

    /// Read-back values as the preview says them, by where they are in their statement (`set
    /// note = 'text', … · where name = 'text'`): `column = 'text'` (a line break as one
    /// character, a long text cut), or how many bytes a value that is not text has.
    fn values_text(&self, values: &[(literals::Part, String, literals::Decoded)]) -> String {
        let one = |column: &str, v: &literals::Decoded| match v {
            literals::Decoded::Text(t) => literals::text_pair(column, t, VALUE_W),
            literals::Decoded::Bytes(n) => {
                let not_text = self.i18n.msg(&Msg::EditPreviewNotText { count: *n as u64 });
                format!("{} = {not_text}", crate::text::sanitize_cell(column))
            }
        };
        let mut groups: Vec<(literals::Part, Vec<String>)> = Vec::new();
        for (part, column, v) in values {
            match groups.last_mut() {
                Some((p, pairs)) if p == part => pairs.push(one(column, v)),
                _ => groups.push((*part, vec![one(column, v)])),
            }
        }
        let label = |p: literals::Part| {
            self.i18n.label(match p {
                literals::Part::Set => Label::EditPreviewPartSet,
                literals::Part::Where => Label::EditPreviewPartWhere,
                literals::Part::Values => Label::EditPreviewPartValues,
            })
        };
        groups
            .iter()
            .map(|(p, pairs)| format!("{} {}", label(*p), pairs.join(", ")))
            .collect::<Vec<_>>()
            .join(" \u{B7} ")
    }

    /// The text values statement `sql` carries as bytes, read back as the preview says them
    /// (`None`: it carries none).
    fn read_back(&self, sql: &str, d: Dialect) -> Option<String> {
        let values = literals::text_literals(sql, d);
        (!values.is_empty()).then(|| self.values_text(&values))
    }
}

/// Where the plan of `batch`'s changes to `target` stands, with what was counted (`checks`).
fn planned(target: &EditTarget, batch: &EditBatch, f: &dyn edit::EditSql, checks: &Checks, facts: &Facts) -> Planned {
    let missing: Vec<PlanError> = edit::no_default(target, batch)
        .into_iter()
        .map(|(id, column)| PlanError::NoDefault { origin: Origin::New(id), column })
        .chain(
            edit::not_shown(target, batch)
                .into_iter()
                .map(|(id, column)| PlanError::NotShown { origin: Origin::New(id), column }),
        )
        .collect();
    if !missing.is_empty() {
        return Planned::NoDefault(missing);
    }
    if let Some(e) = &checks.failed {
        return Planned::Failed(e.clone());
    }
    if !checks.asked.is_empty() {
        return Planned::Counting;
    }
    match edit::plan(target, batch, f, facts) {
        Ok(plan) => Planned::Ready(plan),
        Err(e) => Planned::Refused(e),
    }
}

/// The row and the column problem `e` is about.
fn problem_at(e: &PlanError) -> Option<(RowAt, Option<usize>)> {
    let at = |o: &Origin| match *o {
        Origin::Row(r) => RowAt::Read(r),
        Origin::New(id) => RowAt::New(id),
    };
    match e {
        PlanError::Column { origin, column, .. }
        | PlanError::NoDefault { origin, column }
        | PlanError::Invalid { origin, column } => Some((at(origin), Some(*column))),
        PlanError::NotShown { origin, .. } => Some((at(origin), None)),
        PlanError::NotWhole { row, column } => Some((RowAt::Read(*row), Some(*column))),
        PlanError::NotCounted { row }
        | PlanError::Gone { row }
        | PlanError::Ambiguous { row, .. }
        | PlanError::Choose { row, .. }
        | PlanError::NoOneOfK { row } => Some((RowAt::Read(*row), None)),
        PlanError::Internal(_) => None,
    }
}

#[cfg(test)]
mod tests;
