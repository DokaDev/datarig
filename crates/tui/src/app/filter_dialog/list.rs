//! The value list of the dialog's selected column ([`ValuesView`]) and the ticks in it.
//!
//! The list is the server's count of the column's values when one was asked for ([`count`]),
//! else the values of the rows loaded (approximate: only the rows the tab holds of the query
//! shown, and of a result that spilled to its file only those in memory, said so: the list is
//! made while the dialog is drawn and never reads the file), and under them, once the column has a condition of its own in that query, the values
//! an earlier load of the table without it showed ([`values::Memory`]). The values of a column of
//! numbers that are mostly distinct are not listed: their range is shown instead.
//!
//! A value is ticked into the column's condition only as [`admit`] takes it, read as its list
//! was read (the result's column, the session's values then) and compared with the session now;
//! a long value, one the server may have grouped with others, one of a count whose scope is not
//! the dialog's now, or one of a column that compares no value is shown with why, never ticked.
//! Ticking toggles the value in the column's `=` / `in list` condition (a new `=` when it has
//! none); typing in that condition's value field narrows the list, and a tick takes the place of
//! what was typed.

use super::draft::{Op, Row};
use super::values::{Counted, Counter, LIST_VALUES};
use super::*;
use crate::app::filter_value::{NotAdmitted, ValuesFrom, admit};
use datarig_core::driver::{ColumnMeta, ValueKind};
use datarig_core::query::{self, Value};

/// What a count of the rows loaded is of: the column, the tab's run and the rows it held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TallyOf {
    column: String,
    run: u64,
    rows: usize,
    /// The rows counted: those in memory.
    held: std::ops::Range<usize>,
}

/// Where a value of the list comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Loaded,
    Earlier,
    Exact,
}

/// Why a value of the list is not ticked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NotTicked {
    /// The column's type has no equality a value is compared with.
    NoEquality,
    /// Not taken as a filter's literal ([`admit`]).
    NotAdmitted(NotAdmitted),
    /// Longer than a list keeps: its length in bytes.
    Long(u64),
    /// Sent by the server written otherwise than the grid writes the column's cell (not shown):
    /// its length in bytes as the server measures it.
    Inexact(u64),
    /// The server may count other values that start with the same bytes in its group.
    Prefix,
    /// Of a count whose scope is not the dialog's now.
    Earlier,
    /// Read by an earlier load as a column of another type.
    TypeChanged,
}

/// A value of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    /// As the session shows it; `None` for NULL, or a long value ([`NotTicked::Long`]).
    pub(crate) value: Option<String>,
    pub(crate) rows: u64,
    pub(crate) source: Source,
    /// The value as a condition compares it, or why it is not ticked.
    pub(crate) tick: Result<Value, NotTicked>,
}

/// What the list's heading says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Heading {
    /// Nothing is listed, and why.
    Nothing(Label),
    /// Counted in the rows loaded (approximate), with values of an earlier load under them;
    /// `held`: only so many of them were counted, those in memory.
    Loaded { rows: u64, earlier: bool, held: Option<u64> },
    /// Counted by the server, under the other conditions when there are some.
    Exact { others: bool },
    /// Counted by the server for another scope: the other conditions changed (`true`), or the
    /// session, the table or the values' reading did.
    EarlierCount { others: bool },
}

/// The selected column's value list as the dialog shows it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ValuesView {
    pub(crate) heading: Heading,
    /// The values listed, narrowed by what is typed.
    pub(crate) items: Vec<Item>,
    /// What the list is narrowed by.
    pub(crate) narrow: Option<String>,
    /// A column of numbers whose values are mostly distinct: the smallest, the largest, how
    /// many different values; nothing listed.
    pub(crate) range: Option<(String, String, u64)>,
    /// Values too long to be listed, counted (rows).
    pub(crate) long: u64,
    /// The column has more values than listed.
    pub(crate) more: bool,
    /// A count of the column's values runs.
    pub(crate) counting: bool,
    /// The count's other conditions kept an expression that names the column.
    pub(crate) kept_expr: bool,
    /// Why the server cannot count the column's values (`# count exactly` dimmed).
    pub(crate) count_off: Option<Label>,
    /// The values ticked: those of the column's `=` / `in list` condition.
    pub(crate) ticked: Vec<Value>,
}

/// Whether value text `v` is one the list narrowed by `narrow` keeps (case aside); NULL is
/// kept when `NULL` holds what is typed.
fn kept(v: Option<&str>, narrow: Option<&str>) -> bool {
    let Some(n) = narrow.map(str::to_lowercase) else { return true };
    v.unwrap_or("NULL").to_lowercase().contains(&n)
}

/// The condition of column `col` ticks go into: the one whose value field narrows the list,
/// else its first `=`, `in list` or `is NULL` condition.
pub(crate) fn tick_row(d: &FilterDialog, col: &str) -> Option<usize> {
    let ticks = |t: usize| {
        d.draft
            .where_
            .terms()
            .get(t)
            .and_then(|t| t.row.as_ref())
            .is_some_and(|r| r.col == col && matches!(r.op, Op::Eq | Op::InList | Op::IsNull))
    };
    if let Some((t, _)) = d.narrow.as_ref().filter(|(t, _)| ticks(*t)) {
        return Some(*t);
    }
    d.draft.where_.rows_of(col).into_iter().find(|&t| ticks(t))
}

/// The values of row `r` as ticks: `is NULL` is NULL ticked.
fn ticked(r: &Row) -> Vec<Value> {
    match r.op {
        Op::IsNull => vec![Value::Null],
        _ => r.values.clone(),
    }
}

/// The text typed for a condition of operator `op` that the list narrows by: what is typed for
/// `=`, the value being typed (after the last comma) of `in list`; nothing for the others.
pub(crate) fn fragment(op: Op, typed: &str) -> Option<String> {
    let f = match op {
        Op::Eq => typed.trim(),
        Op::InList => {
            let last = draft::list_items(typed).pop()?;
            return Some(last.quoted.unwrap_or(last.written)).filter(|f| !f.trim().is_empty());
        }
        _ => return None,
    };
    (!f.is_empty()).then(|| f.to_string())
}

/// The values of `counted` from `source`, ticked as `tick` judges each value text.
fn items(
    counted: &[Counted],
    source: Source,
    narrow: Option<&str>,
    tick: &dyn Fn(&str) -> Result<Value, NotTicked>,
    null: &Result<Value, NotTicked>,
) -> Vec<Item> {
    counted
        .iter()
        .filter(|c| kept(c.value.as_deref(), narrow))
        .map(|c| Item {
            value: c.value.clone(),
            rows: c.rows,
            source,
            tick: match &c.value {
                Some(v) => tick(v),
                None => null.clone(),
            },
        })
        .collect()
}

impl App {
    /// The open dialog's list made for its selected column as things are now (the count of the
    /// rows loaded made first when it is not); the dialog keeps how many values it lists.
    pub(crate) fn filter_dialog_list(&mut self) -> Option<ValuesView> {
        self.filter_dialog_tally();
        let view = self.filter_dialog_view();
        let d = self.filter_dialog_mut()?;
        d.listed = view.as_ref().map_or(0, |v| v.items.len());
        view
    }

    /// The selected column's values counted in the rows loaded that are in memory, when the
    /// count held is not of that column and those rows, and the server's count does not stand
    /// in its place; another column than the list's last one starts its list from the top.
    fn filter_dialog_tally(&mut self) {
        let Some(d) = self.filter_dialog() else { return };
        let tab = d.tab;
        let col = d.column().filter(|c| !c.invisible).map(|c| c.name.clone());
        if d.list_of != col
            && let Some(d) = self.filter_dialog_mut()
        {
            d.list_of = col.clone();
            d.narrow = None;
            d.values_scroll = 0;
        }
        let Some(col) = col else { return };
        if self.filter_dialog().is_some_and(|d| d.exact.contains_key(&col)) {
            return;
        }
        let Some(t) = self.tabs.get(tab) else { return };
        let Results::Rows(rs) = &t.results else { return };
        if self.table_query(tab).is_none_or(|tq| tq.shown.is_none()) {
            return;
        }
        let Some(ci) = rs.columns.iter().position(|c| c.meta.name == col) else { return };
        let held = rs.rows.resident();
        let of = TallyOf { column: col, run: t.exec.query_id, rows: rs.rows.len(), held: held.clone() };
        if self.filter_dialog().and_then(|d| d.tally.as_ref()).is_some_and(|(k, _)| *k == of) {
            return;
        }
        let exact = matches!(rs.columns[ci].meta.kind, ValueKind::Integer | ValueKind::Decimal);
        let mut counter = Counter::new(exact);
        // Only the rows in memory: drawing never waits on the spill file.
        let _ = rs.rows.for_each_chunk(held, 256, |rows| {
            for r in rows {
                counter.add(r.get(ci).and_then(Option::as_deref));
            }
        });
        let tally = counter.finish();
        if let Some(d) = self.filter_dialog_mut() {
            d.tally = Some((of, tally));
        }
    }

    /// The list of the open dialog's selected column ([`ValuesView`]), from what is held.
    fn filter_dialog_view(&self) -> Option<ValuesView> {
        let d = self.filter_dialog()?;
        let c = d.column()?;
        let t = self.tabs.get(d.tab)?;
        let dialect = d.dialect;
        let tq = self.table_query(d.tab);
        let shown = tq.and_then(|q| q.shown.as_ref());
        let rs = match &t.results {
            Results::Rows(rs) if shown.is_some() => Some(rs),
            _ => None,
        };
        let held = rs.and_then(|rs| rs.columns.iter().find(|x| x.meta.name == c.name)).map(|x| &x.meta);
        let now = t.exec.values.as_ref();
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        let ticked = tick_row(d, &c.name).and_then(|i| d.draft.where_.terms()[i].row.as_ref()).map(ticked);
        let narrow = d.narrow.as_ref().map(|(_, s)| s.clone());
        let counting = d.counting.as_ref().is_some_and(|k| k.scope.column == c.name)
            && t.exec.running.is_some_and(|r| d.counting.as_ref().is_some_and(|k| k.id == r.id));
        // A column with no equality ticks nothing; one whose type is not known yet ticks as its
        // values are judged.
        let equality = match c.meta.clone().or_else(|| held.cloned()).or_else(|| c.count_meta(dialect)).as_ref() {
            Some(m) if query::compares(dialect, m, c.enum_known).is_err() => Err(NotTicked::NoEquality),
            _ => Ok(()),
        };
        let judge = |meta: &ColumnMeta, read_in: &datarig_core::driver::ReadIn, v: &str| {
            equality.clone()?;
            admit(&ValuesFrom { dialect, meta, read_in }, zone, now, v).map_err(NotTicked::NotAdmitted)
        };
        let null = equality.clone().map(|()| Value::Null);
        let count_off = if c.meta.is_none() && c.count_meta(dialect).is_none() {
            Some(Label::TableDialogCountUnknown)
        } else {
            equality.as_ref().err().map(|_| Label::TableDialogOpOffNoEquality)
        };
        let mut view = ValuesView {
            heading: Heading::Nothing(Label::TableDialogValuesNoRows),
            items: Vec::new(),
            narrow: narrow.clone(),
            range: None,
            long: 0,
            more: false,
            counting,
            kept_expr: false,
            count_off,
            ticked: ticked.unwrap_or_default(),
        };
        let narrow = narrow.as_deref();
        // The server's count, when there is one: it replaces the others.
        if let Some(e) = d.exact.get(&c.name) {
            let now_scope = self.filter_dialog_scope(d, &c.name);
            let current = now_scope.as_ref() == Some(&e.scope);
            view.heading = match current {
                true => Heading::Exact { others: e.scope.other.is_some() },
                false => Heading::EarlierCount { others: now_scope.is_some_and(|s| e.scope.others_changed(&s)) },
            };
            view.kept_expr = e.kept_expr;
            view.more = e.counts.more;
            for g in &e.counts.values {
                let value = if g.whole { g.value.clone() } else { None };
                if !kept(value.as_deref(), narrow) || !g.whole && narrow.is_some() {
                    continue;
                }
                let tick = if !current {
                    Err(NotTicked::Earlier)
                } else if g.inexact {
                    Err(NotTicked::Inexact(g.bytes.unwrap_or(0)))
                } else if !g.whole {
                    Err(NotTicked::Long(g.bytes.unwrap_or(0)))
                } else if g.prefix {
                    Err(NotTicked::Prefix)
                } else {
                    match &value {
                        Some(v) => judge(&e.meta, &e.scope.read_in, v),
                        None => null.clone(),
                    }
                };
                view.items.push(Item { value, rows: g.rows, source: Source::Exact, tick });
            }
            view.items.truncate(LIST_VALUES);
            return Some(view);
        }
        if c.invisible {
            view.heading = Heading::Nothing(Label::TableDialogValuesInvisible);
            return Some(view);
        }
        let (Some(rs), Some(meta)) = (rs, held) else { return Some(view) };
        let Some((_, tally)) = d.tally.as_ref().filter(|(k, _)| k.column == c.name) else { return Some(view) };
        view.long = tally.long;
        let distinct = tally.mostly_distinct();
        if distinct && narrow.is_none() {
            let (lo, hi) = tally.range.clone().unwrap_or_default();
            let n = tally.values.iter().filter(|v| v.value.is_some()).count() as u64;
            view.range = Some((lo, hi, n));
        } else {
            let tick = |v: &str| judge(meta, &rs.read_in, v);
            view.items = items(&tally.values, Source::Loaded, narrow, &tick, &null);
        }
        view.more = tally.more();
        // The values an earlier load showed, once the rows loaded are filtered by the column.
        let own = shown.and_then(|q| q.filter.as_ref()).is_some_and(|f| {
            let target = self.table_target(d.tab);
            target.is_some_and(|target| {
                let w = draft::Where::from_text(&f.text, &target);
                !w.rows_of(&c.name).is_empty()
                    || w.terms().iter().any(|t| {
                        t.is_expr() && t.text.as_deref().is_some_and(|x| values::mentions(x, &c.name, dialect))
                    })
            })
        });
        let earlier = (own && !distinct)
            .then(|| self.values_key(d.tab))
            .flatten()
            .and_then(|key| self.value_memory.column(&key, &c.name));
        let mut with_earlier = false;
        if let Some(r) = earlier {
            let changed = r.meta.type_name != meta.type_name || r.meta.kind != meta.kind;
            let tick = |v: &str| if changed { Err(NotTicked::TypeChanged) } else { judge(&r.meta, &r.read_in, v) };
            let loaded: Vec<Option<String>> = tally.values.iter().map(|v| v.value.clone()).collect();
            let rest: Vec<Counted> = r.values().into_iter().filter(|v| !loaded.contains(&v.value)).collect();
            let more = items(&rest, Source::Earlier, narrow, &tick, &null);
            with_earlier = !more.is_empty();
            view.items.extend(more);
        }
        view.more |= view.items.len() > LIST_VALUES;
        view.items.truncate(LIST_VALUES);
        let counted = rs.rows.resident().len();
        let held = (counted < rs.rows.len()).then_some(counted as u64);
        view.heading = Heading::Loaded { rows: rs.rows.len() as u64, earlier: with_earlier, held };
        Some(view)
    }

    /// `Space` on value `i` of the list: ticked into the selected column's condition, or
    /// unticked out of it (a condition left with one value is `=`, with several `in list`, with
    /// none taken out); a new `=` condition when the column has none. What was typed to narrow
    /// the list gives way to the value. One undo step. A value not ticked says why.
    pub(crate) fn filter_dialog_tick(&mut self, i: usize) {
        let Some(view) = self.filter_dialog_list() else { return };
        let Some(item) = view.items.get(i) else { return };
        let value = match &item.tick {
            Ok(v) => v.clone(),
            Err(why) => {
                let n = self.not_ticked_notice(why);
                return self.say_over_filter_dialog(n);
            }
        };
        let shown = item.value.clone();
        let Some(d) = self.filter_dialog() else { return };
        let Some(col) = d.column().map(|c| c.name.clone()) else { return };
        let at = tick_row(d, &col);
        let narrow = d.narrow.clone();
        let dialect = d.dialect;
        self.filter_dialog_edit(None, |d, target| {
            let Some(t) = at else {
                d.draft.where_.add(Row { col, op: Op::Eq, values: vec![value], keep_nulls: false }, target);
                return;
            };
            let Some(row) = d.draft.where_.terms().get(t).and_then(|t| t.row.clone()) else { return };
            let mut values = ticked(&row);
            // What was typed to find the value is replaced by it.
            if let Some((_, typed)) = narrow.as_ref().filter(|(n, _)| *n == t)
                && values.last().is_some_and(|v| draft::value_text(v, dialect).trim() == typed.as_str())
            {
                values.pop();
            }
            match values.iter().position(|v| *v == value) {
                Some(p) => {
                    values.remove(p);
                }
                None => values.push(value),
            }
            let op = if values.len() > 1 { Op::InList } else { Op::Eq };
            match values.is_empty() {
                true => {
                    d.draft.where_.remove(t);
                }
                false => {
                    d.draft.where_.set(t, Row { op, values, keep_nulls: false, ..row }, target);
                }
            }
        });
        // The list in full again, the cursor kept on the value.
        if let Some(d) = self.filter_dialog_mut() {
            d.narrow = None;
        }
        let Some(view) = self.filter_dialog_list() else { return };
        let at = view.items.iter().position(|it| it.value == shown).unwrap_or(0);
        if let Some(d) = self.filter_dialog_mut() {
            d.focus = DialogFocus::Values(at.min(d.listed.saturating_sub(1)));
        }
    }

    /// Why a value of the list is not ticked, as the dialog says it.
    pub(crate) fn not_ticked_notice(&self, why: &NotTicked) -> Notice {
        let warn = |l: Label| Notice::new(l, Level::Warning);
        match why {
            NotTicked::NoEquality => warn(Label::TableDialogOpOffNoEquality),
            NotTicked::NotAdmitted(NotAdmitted::NotWhole(r)) => {
                let reason = self.not_whole_reason(&r.why);
                Notice::new(Msg::TableDialogTickNotWhole { reason }, Level::Warning)
            }
            NotTicked::NotAdmitted(NotAdmitted::TooBig) => warn(Label::TableDialogTickTooBig),
            NotTicked::Long(bytes) => {
                Notice::new(Msg::TableDialogTickLong { bytes: bytes.to_string() }, Level::Warning)
            }
            NotTicked::Inexact(_) => warn(Label::TableDialogTickInexact),
            NotTicked::Prefix => warn(Label::TableDialogTickPrefix),
            NotTicked::Earlier => warn(Label::TableDialogTickEarlier),
            NotTicked::TypeChanged => warn(Label::TableDialogTickTypeChanged),
        }
    }

    /// Where the values loads of table tab `id`'s table showed are remembered.
    pub(crate) fn values_key(&self, id: TabId) -> Option<values::TableKey> {
        let t = self.tabs.get(id)?;
        let name = self.table_name(id)?;
        Some(values::TableKey { profile: t.profile?, database: name.database, schema: name.schema, table: name.name })
    }

    /// Table tab `id` holds more rows of its query's load (its first page, or more): the
    /// values of each column with no condition of its own in that query are counted into the
    /// session's memory of the table's values.
    pub(crate) fn remember_values(&mut self, id: TabId) {
        let Some(key) = self.values_key(id) else { return };
        let Some(target) = self.table_target(id) else { return };
        let Some(t) = self.tabs.get(id) else { return };
        let Some(shown) = t.doc.table_query.as_ref().and_then(|q| q.shown.as_ref()) else { return };
        let Results::Rows(rs) = &t.results else { return };
        let d = target.dialect;
        let filter = shown.filter.as_ref().map(|f| draft::Where::from_text(&f.text, &target));
        let free = |name: &str| {
            filter.as_ref().is_none_or(|w| {
                w.rows_of(name).is_empty()
                    && !w
                        .terms()
                        .iter()
                        .any(|t| t.is_expr() && t.text.as_deref().is_some_and(|x| values::mentions(x, name, d)))
            })
        };
        let load = values::Load {
            table: key,
            tab: id,
            run: t.exec.query_id,
            columns: rs.columns.iter().map(|c| (&c.meta, free(&c.meta.name))).collect(),
            read_in: &rs.read_in,
        };
        let mut memory = std::mem::take(&mut self.value_memory);
        let from = memory.start(&load);
        let _ = rs.rows.for_each_chunk(from..rs.rows.len(), 256, |rows| memory.add(&load, rows));
        drop(load);
        self.value_memory = memory;
    }
}
