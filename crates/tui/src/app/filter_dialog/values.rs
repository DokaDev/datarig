//! The values of a column the filter dialog lists under its conditions, as data: counted from
//! the rows loaded ([`tally`]), remembered from earlier loads of the table in this session
//! ([`Memory`]), or counted by the server (kept by the dialog with their scope).
//!
//! Every count is bounded: a value longer than [`VALUE_BYTES`] is counted as a long value and
//! never kept as text, the values kept while counting stop at [`TALLY_DISTINCT`] values or
//! [`TALLY_BYTES`] bytes (the rows of values past them are counted apart), and what a load leaves
//! in the session's memory is bounded per column, per table and for the session, in bytes
//! counted with a cost for every table, column and value kept besides their text (so NULL and
//! empty values cost too), and in tables.

use std::collections::HashMap;

use crate::app::tabs::TabId;
use datarig_core::driver::{ColumnMeta, ReadIn};
use datarig_core::profile::ProfileId;
use datarig_core::query::{GROUP_PREVIEW, GROUP_VALUES};
use datarig_core::sql::dialect::Dialect;
use datarig_core::sql::lexer::{Tok, lex_in};

/// The most values a list shows.
pub(crate) const LIST_VALUES: usize = GROUP_VALUES;

/// The longest value, in bytes, kept as text: a longer one is a long value, counted only.
pub(crate) const VALUE_BYTES: usize = GROUP_PREVIEW;

/// The most different values a count of loaded rows keeps.
pub(crate) const TALLY_DISTINCT: usize = 10_000;

/// The most bytes of values a count of loaded rows keeps.
pub(crate) const TALLY_BYTES: usize = 4 << 20;

/// The most rows a count of the loaded rows reads (the first of those in memory): it is made
/// while the dialog is drawn, whatever the result's window in memory.
pub(crate) const TALLY_ROWS: usize = 20_000;

/// The fewest values (NULL left out) a column's values are judged mostly distinct from.
pub(crate) const MOSTLY_MIN: u64 = 50;

/// The most values of a column an earlier load leaves in the memory.
pub(crate) const MEMORY_VALUES: usize = 200;

/// The most columns of a table an earlier load leaves in the memory.
pub(crate) const MEMORY_COLUMNS: usize = 64;

/// The most bytes of values one table leaves in the memory.
pub(crate) const MEMORY_TABLE_BYTES: usize = 1 << 20;

/// The most bytes of values the memory holds for the session.
pub(crate) const MEMORY_BYTES: usize = 4 << 20;

/// The most tables the memory holds for the session.
pub(crate) const MEMORY_TABLES: usize = 64;

/// What the memory counts for a table it keeps besides its names: its entry and its load.
const TABLE_COST: usize = 512;

/// What the memory counts for a column it keeps besides its names: its entry, its result
/// column and the session's values it was read in.
const COLUMN_COST: usize = 512;

/// What the memory counts for a value it keeps besides its text: its entry and its rows.
const VALUE_COST: usize = 64;

/// What the memory counts for value `v` kept.
fn value_cost(v: Option<&str>) -> usize {
    VALUE_COST + v.map_or(0, str::len)
}

/// A value of a list and the rows that hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Counted {
    /// The value as the session shows it; `None` for NULL.
    pub(crate) value: Option<String>,
    pub(crate) rows: u64,
}

/// The values of one column counted in rows: most rows first (then by text), NULL among them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    /// Each value kept and its rows, most rows first.
    pub(crate) values: Vec<Counted>,
    /// The rows counted.
    pub(crate) rows: u64,
    /// The rows whose value is longer than [`VALUE_BYTES`] (never kept as text).
    pub(crate) long: u64,
    /// The rows of values not kept (past [`TALLY_DISTINCT`] or [`TALLY_BYTES`]).
    pub(crate) others: u64,
    /// The values not NULL counted, long ones included.
    pub(crate) not_null: u64,
    /// The smallest and the largest value of a column of whole or decimal numbers, as shown;
    /// `None` when a value of it is not one written in plain digits (those are listed).
    pub(crate) range: Option<(String, String)>,
}

impl Tally {
    /// The values of a number column are mostly distinct: at least [`MOSTLY_MIN`] values (NULL
    /// left out) of which the different ones are more than 80 %.
    pub(crate) fn mostly_distinct(&self) -> bool {
        let distinct = self.values.iter().filter(|c| c.value.is_some()).count() as u64;
        self.range.is_some() && self.not_null >= MOSTLY_MIN && distinct * 5 > self.not_null * 4
    }

    /// Some values are not listed: past the list's length, or not kept while counting.
    pub(crate) fn more(&self) -> bool {
        self.values.len() > LIST_VALUES || self.others > 0
    }
}

/// The values of one column counted one cell at a time ([`Counter::add`]), kept as [`Tally`]
/// says.
#[derive(Debug, Default)]
pub(crate) struct Counter {
    counts: HashMap<Option<String>, u64>,
    bytes: usize,
    /// The range is kept: a column of numbers compared exactly, every value so far one.
    ranged: bool,
    /// The smallest and the largest number so far, as shown.
    range: Option<(String, String)>,
    tally: Tally,
}

impl Counter {
    /// A count of a column of whole or decimal numbers (`exact`: their range is kept, each
    /// compared digit by digit) or of other values (floating-point numbers among them: a range
    /// of rounded values could say a wrong one).
    pub(crate) fn new(exact: bool) -> Self {
        Self { ranged: exact, ..Self::default() }
    }

    /// One more cell, as read (`None`: NULL).
    pub(crate) fn add(&mut self, cell: Option<&str>) {
        let t = &mut self.tally;
        t.rows += 1;
        if let Some(v) = cell {
            t.not_null += 1;
            if v.len() > VALUE_BYTES {
                t.long += 1;
                self.ranged = false;
                return;
            }
            if self.ranged {
                match (Decimal::of(v), &mut self.range) {
                    (None, _) => self.ranged = false,
                    (Some(_), None) => self.range = Some((v.to_string(), v.to_string())),
                    (Some(n), Some((lo, hi))) => {
                        if Decimal::of(lo).is_some_and(|l| n < l) {
                            *lo = v.to_string();
                        }
                        if Decimal::of(hi).is_some_and(|h| n > h) {
                            *hi = v.to_string();
                        }
                    }
                }
            }
        }
        let key = cell.map(str::to_string);
        if let Some(n) = self.counts.get_mut(&key) {
            *n += 1;
        } else if self.counts.len() < TALLY_DISTINCT && self.bytes + cell.map_or(0, str::len) <= TALLY_BYTES {
            self.bytes += cell.map_or(0, str::len);
            self.counts.insert(key, 1);
        } else {
            t.others += 1;
        }
    }

    /// What was counted.
    pub(crate) fn finish(self) -> Tally {
        let mut t = self.tally;
        t.range = self.range.filter(|_| self.ranged);
        t.values = self.counts.into_iter().map(|(value, rows)| Counted { value, rows }).collect();
        sort(&mut t.values);
        t
    }
}

/// A number written in plain digits (an optional sign, digits with at most one point), as it
/// compares: exactly, whatever its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Decimal<'a> {
    negative: bool,
    /// The digits before the point, without leading zeros.
    whole: &'a str,
    /// The digits after the point, without trailing zeros.
    fraction: &'a str,
}

impl<'a> Decimal<'a> {
    fn of(text: &'a str) -> Option<Self> {
        let s = text.trim();
        let (negative, unsigned) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        };
        let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
        let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if whole.is_empty() && fraction.is_empty() || !digits(whole) || !digits(fraction) {
            return None;
        }
        let (whole, fraction) = (whole.trim_start_matches('0'), fraction.trim_end_matches('0'));
        // Zero has no sign.
        let negative = negative && !(whole.is_empty() && fraction.is_empty());
        Some(Self { negative, whole, fraction })
    }
}

impl Ord for Decimal<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let size = |d: &Self| {
            d.whole
                .len()
                .cmp(&other.whole.len())
                .then_with(|| d.whole.cmp(other.whole))
                .then_with(|| d.fraction.cmp(other.fraction))
        };
        match (self.negative, other.negative) {
            (false, true) => std::cmp::Ordering::Greater,
            (true, false) => std::cmp::Ordering::Less,
            (false, false) => size(self),
            (true, true) => size(self).reverse(),
        }
    }
}

impl PartialOrd for Decimal<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The values of `cells` (one column's, as read) counted ([`Counter`]).
#[cfg(test)]
pub(crate) fn tally<'a>(cells: impl IntoIterator<Item = Option<&'a str>>, number: bool) -> Tally {
    let mut c = Counter::new(number);
    for cell in cells {
        c.add(cell);
    }
    c.finish()
}

/// Most rows first, then NULL, then by text.
pub(crate) fn sort(values: &mut [Counted]) {
    values.sort_by(|a, b| b.rows.cmp(&a.rows).then_with(|| a.value.cmp(&b.value)));
}

/// Term `text` of dialect `d` names column `col` (an identifier, quoted or not, of that name,
/// whatever its case): a condition that may restrict the column's values.
pub(crate) fn mentions(text: &str, col: &str, d: Dialect) -> bool {
    lex_in(text, d).iter().any(|t| match t.kind {
        Tok::Ident | Tok::Keyword => t.text(text).eq_ignore_ascii_case(col),
        Tok::QuotedIdent => {
            let q = t.text(text);
            let (open, inner) = q.split_at(q.chars().next().map_or(0, char::len_utf8));
            let inner = inner.strip_suffix(open).unwrap_or(inner);
            inner.replace(&format!("{open}{open}"), open).eq_ignore_ascii_case(col)
        }
        _ => false,
    })
}

/// A table whose earlier loads the memory keeps: the tab's profile and database, the table.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TableKey {
    pub(crate) profile: ProfileId,
    pub(crate) database: Option<String>,
    pub(crate) schema: String,
    pub(crate) table: String,
}

/// The values one column had in an earlier load of its table without a condition of its own,
/// with what they were read with (the result's column and the session's values).
#[derive(Clone, Debug)]
pub(crate) struct Remembered {
    pub(crate) meta: ColumnMeta,
    pub(crate) read_in: ReadIn,
    values: HashMap<Option<String>, u64>,
}

impl Remembered {
    /// The values and their rows, most rows first.
    pub(crate) fn values(&self) -> Vec<Counted> {
        let mut v: Vec<Counted> =
            self.values.iter().map(|(value, rows)| Counted { value: value.clone(), rows: *rows }).collect();
        sort(&mut v);
        v
    }

    /// What the memory counts for it: the column and every value kept.
    fn cost(&self) -> usize {
        column_cost(&self.meta) + self.values.keys().map(|v| value_cost(v.as_deref())).sum::<usize>()
    }
}

/// What the memory counts for a column of result column `meta`, without its values.
fn column_cost(meta: &ColumnMeta) -> usize {
    let origin = match &meta.origin {
        Some(datarig_core::driver::ColumnOrigin::Named { schema, table, column }) => {
            schema.len() + table.len() + column.len()
        }
        _ => 0,
    };
    COLUMN_COST + 2 * meta.name.len() + meta.type_name.len() + origin
}

impl TableKey {
    /// What the memory counts for a table of this key, without its columns.
    fn cost(&self) -> usize {
        TABLE_COST + self.database.as_ref().map_or(0, String::len) + self.schema.len() + self.table.len()
    }
}

/// What the memory keeps of one table: its columns' values and the load they come from.
#[derive(Clone, Debug, Default)]
struct TableMemory {
    /// The load counted last (its tab and its result), and how many of its rows were counted.
    load: Option<(TabId, u64)>,
    counted: usize,
    columns: HashMap<String, Remembered>,
    /// What the memory counts for the table: its key and its columns with their values.
    bytes: usize,
}

/// A load of a table being counted into the memory ([`Memory::load`]).
pub(crate) struct Load<'a> {
    pub(crate) table: TableKey,
    pub(crate) tab: TabId,
    /// The result the rows are in: the same for every page of it, whether its portal was held
    /// or its statement ran again for the page, and another for every new run.
    pub(crate) result: u64,
    /// How many rows the result holds now.
    pub(crate) held: usize,
    /// The result's columns with whether each had no condition of its own in the load.
    pub(crate) columns: Vec<(&'a ColumnMeta, bool)>,
    pub(crate) read_in: &'a ReadIn,
}

/// The values the loads of tables in this session showed, per column with no condition of its
/// own in the load: bounded (see the module docs), the tables used least recently dropped
/// first, a table dropped as its structure is read again.
#[derive(Clone, Debug, Default)]
pub(crate) struct Memory {
    /// Least recently used first.
    tables: Vec<(TableKey, TableMemory)>,
}

impl Memory {
    /// Load `l` is counted in: a new load (another result, or another tab) starts its unfiltered
    /// columns afresh (a column filtered in it keeps what an earlier load left), and so does a
    /// load holding fewer rows than were counted of it. Returns how many of its rows are counted
    /// already: [`Memory::add`] takes those after them, so each page counts only its own rows.
    pub(crate) fn start(&mut self, l: &Load<'_>) -> usize {
        let at = match self.tables.iter().position(|(k, _)| *k == l.table) {
            Some(i) => i,
            None => {
                self.tables.push((l.table.clone(), TableMemory::default()));
                self.tables.len() - 1
            }
        };
        let (key, mut m) = self.tables.remove(at);
        if m.load != Some((l.tab, l.result)) || m.counted > l.held {
            m.load = Some((l.tab, l.result));
            m.counted = 0;
            for (meta, free) in &l.columns {
                if *free {
                    m.columns.remove(&meta.name);
                }
            }
        }
        m.bytes = key.cost() + m.columns.values().map(Remembered::cost).sum::<usize>();
        let counted = m.counted;
        self.tables.push((key, m));
        self.bound();
        counted
    }

    /// The next rows of load `l` (after those counted, [`Memory::start`]), each a row of its
    /// result's columns, counted in its unfiltered columns.
    pub(crate) fn add(&mut self, l: &Load<'_>, rows: &[Vec<Option<String>>]) {
        let Some((_, m)) = self.tables.iter_mut().find(|(k, _)| *k == l.table) else { return };
        if m.load != Some((l.tab, l.result)) {
            return;
        }
        for (ci, (meta, free)) in l.columns.iter().enumerate() {
            if !free {
                continue;
            }
            if !m.columns.contains_key(&meta.name) {
                let cost = column_cost(meta);
                if m.columns.len() >= MEMORY_COLUMNS || m.bytes + cost > MEMORY_TABLE_BYTES {
                    continue;
                }
                m.bytes += cost;
                let col = Remembered { meta: (*meta).clone(), read_in: l.read_in.clone(), values: HashMap::new() };
                m.columns.insert(meta.name.clone(), col);
            }
            let Some(col) = m.columns.get_mut(&meta.name) else { continue };
            for row in rows {
                let v = row.get(ci).and_then(Option::as_deref);
                if v.is_some_and(|v| v.len() > VALUE_BYTES) {
                    continue;
                }
                let key = v.map(str::to_string);
                if let Some(n) = col.values.get_mut(&key) {
                    *n += 1;
                } else if col.values.len() < MEMORY_VALUES && m.bytes + value_cost(v) <= MEMORY_TABLE_BYTES {
                    m.bytes += value_cost(v);
                    col.values.insert(key, 1);
                }
            }
            // A column with no value kept is not kept.
            if col.values.is_empty() {
                m.bytes -= column_cost(meta);
                m.columns.remove(&meta.name);
            }
        }
        m.counted += rows.len();
        self.bound();
    }

    /// The session's bounds: a table with no column kept goes unless it is the one counted
    /// last; then the tables used least recently go first, never the one counted last.
    fn bound(&mut self) {
        let last = self.tables.len().saturating_sub(1);
        let mut i = 0;
        self.tables.retain(|(_, m)| {
            i += 1;
            i - 1 == last || !m.columns.is_empty()
        });
        while (self.bytes() > MEMORY_BYTES || self.tables.len() > MEMORY_TABLES) && self.tables.len() > 1 {
            self.tables.remove(0);
        }
    }

    /// What an earlier load left of column `col` of `table`.
    pub(crate) fn column(&self, table: &TableKey, col: &str) -> Option<&Remembered> {
        self.tables.iter().find(|(k, _)| k == table)?.1.columns.get(col)
    }

    /// Forget the tables of `profile` (in `database` only, when given: `Some(None)` its own).
    pub(crate) fn forget(&mut self, profile: ProfileId, database: Option<Option<&str>>) {
        self.tables.retain(|(k, _)| k.profile != profile || database.is_some_and(|db| k.database.as_deref() != db));
    }

    fn bytes(&self) -> usize {
        self.tables.iter().map(|(_, m)| m.bytes).sum()
    }

    #[cfg(test)]
    fn tables(&self) -> usize {
        self.tables.len()
    }
}

#[cfg(test)]
mod tests;
