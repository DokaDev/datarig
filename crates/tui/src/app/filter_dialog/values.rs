//! The values of a column the filter dialog lists under its conditions, as data: counted from
//! the rows loaded ([`tally`]), remembered from earlier loads of the table in this session
//! ([`Memory`]), or counted by the server (kept by the dialog with their scope).
//!
//! Every count is bounded: a value longer than [`VALUE_BYTES`] is counted as a long value and
//! never kept as text, the values kept while counting stop at [`TALLY_DISTINCT`] values or
//! [`TALLY_BYTES`] bytes (the rows of values past them are counted apart), and what a load leaves
//! in the session's memory is bounded per column, per table and for the session.

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
    /// The smallest and the largest value of a number column, as shown.
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
    number: bool,
    /// The smallest and the largest number so far, as shown and as read.
    range: Option<(String, f64, String, f64)>,
    tally: Tally,
}

impl Counter {
    /// A count of a column of numbers (`number`: their range is kept) or of other values.
    pub(crate) fn new(number: bool) -> Self {
        Self { number, ..Self::default() }
    }

    /// One more cell, as read (`None`: NULL).
    pub(crate) fn add(&mut self, cell: Option<&str>) {
        let t = &mut self.tally;
        t.rows += 1;
        if let Some(v) = cell {
            t.not_null += 1;
            if v.len() > VALUE_BYTES {
                t.long += 1;
                return;
            }
            if self.number
                && let Ok(n) = v.trim().parse::<f64>()
            {
                match &mut self.range {
                    None => self.range = Some((v.to_string(), n, v.to_string(), n)),
                    Some((lo, l, hi, h)) => {
                        if n < *l {
                            (*lo, *l) = (v.to_string(), n);
                        }
                        if n > *h {
                            (*hi, *h) = (v.to_string(), n);
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
        t.range = self.range.map(|(lo, _, hi, _)| (lo, hi));
        t.values = self.counts.into_iter().map(|(value, rows)| Counted { value, rows }).collect();
        sort(&mut t.values);
        t
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

    fn bytes(&self) -> usize {
        self.values.keys().map(|v| v.as_ref().map_or(0, String::len)).sum()
    }
}

/// What the memory keeps of one table: its columns' values and the load they come from.
#[derive(Clone, Debug, Default)]
struct TableMemory {
    /// The load counted last (its tab and its run), and how many of its rows were counted.
    load: Option<(TabId, u64)>,
    counted: usize,
    columns: HashMap<String, Remembered>,
    bytes: usize,
}

/// A load of a table being counted into the memory ([`Memory::load`]).
pub(crate) struct Load<'a> {
    pub(crate) table: TableKey,
    pub(crate) tab: TabId,
    pub(crate) run: u64,
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
    /// Load `l` is counted in: a new load (another run, or another tab) starts its unfiltered
    /// columns afresh (a column filtered in it keeps what an earlier load left). Returns how many
    /// of its rows are counted already: [`Memory::add`] takes those after them.
    pub(crate) fn start(&mut self, l: &Load<'_>) -> usize {
        let at = match self.tables.iter().position(|(k, _)| *k == l.table) {
            Some(i) => i,
            None => {
                self.tables.push((l.table.clone(), TableMemory::default()));
                self.tables.len() - 1
            }
        };
        let (key, mut m) = self.tables.remove(at);
        if m.load != Some((l.tab, l.run)) {
            m.load = Some((l.tab, l.run));
            m.counted = 0;
            for (meta, free) in &l.columns {
                if *free {
                    m.columns.remove(&meta.name);
                }
            }
            m.bytes = m.columns.values().map(Remembered::bytes).sum();
        }
        let counted = m.counted;
        self.tables.push((key, m));
        counted
    }

    /// The next rows of load `l` (after those counted, [`Memory::start`]), each a row of its
    /// result's columns, counted in its unfiltered columns.
    pub(crate) fn add(&mut self, l: &Load<'_>, rows: &[Vec<Option<String>>]) {
        let Some((_, m)) = self.tables.iter_mut().find(|(k, _)| *k == l.table) else { return };
        if m.load != Some((l.tab, l.run)) {
            return;
        }
        for (ci, (meta, free)) in l.columns.iter().enumerate() {
            if !free || !m.columns.contains_key(&meta.name) && m.columns.len() >= MEMORY_COLUMNS {
                continue;
            }
            let col = m.columns.entry(meta.name.clone()).or_insert_with(|| Remembered {
                meta: (*meta).clone(),
                read_in: l.read_in.clone(),
                values: HashMap::new(),
            });
            for row in rows {
                let v = row.get(ci).and_then(Option::as_deref);
                if v.is_some_and(|v| v.len() > VALUE_BYTES) {
                    continue;
                }
                let key = v.map(str::to_string);
                if let Some(n) = col.values.get_mut(&key) {
                    *n += 1;
                } else if col.values.len() < MEMORY_VALUES && m.bytes + v.map_or(0, str::len) <= MEMORY_TABLE_BYTES {
                    m.bytes += v.map_or(0, str::len);
                    col.values.insert(key, 1);
                }
            }
        }
        m.counted += rows.len();
        // The session's bound: the tables used least recently go first, never the one counted.
        while self.bytes() > MEMORY_BYTES && self.tables.len() > 1 {
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
