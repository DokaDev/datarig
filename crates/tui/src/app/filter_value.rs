//! Filter a table tab by the value of a cell (`=` only this value, `!` everything but it, and
//! their entries in the column menu): the cursor's cell, or the cells selected in one column.
//!
//! The condition is written by the query model ([`query::render`]: literals for the column's
//! kind through the dialect, NULL compared only with `IS NULL` or the NULL-safe forms) into the
//! filter of the rows shown, as the filter dialog writes a term: the filter's other terms stay
//! as they are written ([`Where`]). The new text runs as the filter dialog runs a filter
//! ([`App::table_run`]: the checks, the questions about functions, row locks and pending
//! edits, read-only profiles).
//!
//! A value is taken only as the server holds it ([`admit`]): a cell shown otherwise than its
//! value (as a copy as SQL judges it), a new row, a cell with an edit on it, or rows that cannot
//! be read back give no filter; nor do a type whose comparison is not known, binary data over
//! 64 KiB, or more than 1000 cells. Nothing is added while the query applied is one whose run
//! brought no rows (refused, cancelled), over the earlier rows still shown.

use super::filter_dialog::draft::{Op, Row, Where};
use super::menu::Off;
use super::table_query::{FIELD_CAP, filter_text};
use super::*;
use crate::widgets::grid::Shape;
use crate::widgets::text_input::TextInput;
use datarig_core::driver::{ColumnMeta, ReadIn, ValueKind};
use datarig_core::export;
use datarig_core::query::{self, Filter, Query, RenderError, Target, Value};
use std::ops::Range;

/// The most cells of a selection a filter takes values from.
pub const MAX_VALUES: usize = 1000;

/// The largest binary value a filter takes, in bytes.
pub const MAX_BYTES: usize = 64 * 1024;

/// Display columns of a value in a menu entry's label (cut with `…` past them).
const VALUE_W: usize = 24;

/// Display columns of the values of a list in a menu entry's label.
const VALUES_W: usize = 32;

/// What a filter by the value of the grid's cells keeps ([`App::grid_pick`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pick {
    /// Only the rows with the value (`=`): the column's own conditions replaced.
    Only,
    /// Every row but those with the value or values (`!`), added to the other conditions: rows
    /// whose value is NULL are kept unless NULL is among the cells (a NULL cell alone is `IS
    /// NOT NULL`).
    Exclude,
}

/// A filter by the value of the grid's cells, ready to run: the column, its values as a menu
/// entry names them, and the whole filter text it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    pub column: String,
    pub shown: String,
    pub text: String,
}

/// Bytes written in hexadecimal after `prefix` (`\x` on PostgreSQL, `0x` on MySQL).
fn hex_bytes(v: &str, prefix: &str) -> Option<Vec<u8>> {
    let digits = v.strip_prefix(prefix)?.as_bytes();
    if digits.len() % 2 != 0 {
        return None;
    }
    digits.chunks(2).map(|p| std::str::from_utf8(p).ok().and_then(|s| u8::from_str_radix(s, 16).ok())).collect()
}

/// `s` cut to `w` display columns, `…` at the end when cut.
fn cut(s: &str, w: usize) -> String {
    if crate::text::width(s) <= w {
        return s.to_string();
    }
    crate::text::fit(s, w, crate::text::Align::Left)
}

/// What a list of a column's values was read with: the dialect, the result column as the
/// session described it, and the session's values its rows were read in (time zone, character
/// set). A value is taken as a filter's literal only as read with these ([`admit`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ValuesFrom<'a> {
    pub(crate) dialect: Dialect,
    pub(crate) meta: &'a ColumnMeta,
    pub(crate) read_in: &'a ReadIn,
}

/// Why a value read is not taken as a filter's literal ([`admit`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NotAdmitted {
    /// Shown otherwise than the server holds it, as a copy as SQL judges it (a MySQL `FLOAT`,
    /// text read in another character set than UTF-8, a `TIMESTAMP` read in another time zone
    /// than the session's now or in one that is not a fixed offset, bytes not in their shown
    /// form, values read before the session said its values or across a change of them).
    NotWhole(export::Refused),
    /// Binary data over [`MAX_BYTES`].
    TooBig,
}

/// Value `v` (as read, not NULL) of the column `from` describes, as a filter compares it; or why
/// it is not taken. The one judge of every value a filter takes from what was read: a filter by
/// a cell's value and a value ticked in the filter dialog (from the rows loaded, from an earlier
/// load, from a count of the server's). `server_zone`: the server's own time zone (for MySQL's
/// `SYSTEM`); `now`: the session's values now, which a MySQL `TIMESTAMP` must have been read in.
/// A MySQL `TIMESTAMP` is taken with the offset of the zone it was read in.
pub(crate) fn admit(
    from: &ValuesFrom<'_>,
    server_zone: Option<&str>,
    now: Option<&datarig_core::driver::SessionValues>,
    v: &str,
) -> Result<Value, NotAdmitted> {
    let (d, m) = (from.dialect, from.meta);
    let cols = [export::SqlColumn { name: &m.name, type_name: &m.type_name, kind: m.kind, auto_increment: false }];
    let cells = export::sql_cells(d, &cols, from.read_in, server_zone).map_err(NotAdmitted::NotWhole)?;
    let cell = cells.cell(0, v).map_err(NotAdmitted::NotWhole)?;
    let mysql = matches!(d, Dialect::MySql(_));
    let refused = |why| NotAdmitted::NotWhole(export::Refused { column: m.name.clone(), why });
    // A MySQL TIMESTAMP is shown in the session's zone: the session reads it back the same only
    // in the zone it was read in.
    if mysql && m.kind == ValueKind::TimestampTz {
        let read = match from.read_in {
            ReadIn::Values(s) => Some(&s.time_zone),
            _ => None,
        };
        if read.is_none() || now.map(|s| &s.time_zone) != read {
            return Err(refused(export::NotWhole::SessionChanged));
        }
    }
    Ok(match m.kind {
        ValueKind::Integer | ValueKind::Decimal | ValueKind::Float => Value::Number(v.to_string()),
        // With the offset of the zone it was read in (as a copy as SQL writes it): the same
        // instant in any session's zone, as the filter is run again from the history.
        ValueKind::TimestampTz if mysql => Value::Text(cell.into_owned()),
        ValueKind::Bytes => {
            let bytes = hex_bytes(v, if mysql { "0x" } else { "\\x" })
                .ok_or_else(|| refused(export::NotWhole::NotShownWhole))?;
            if bytes.len() > MAX_BYTES {
                return Err(NotAdmitted::TooBig);
            }
            Value::Bytes(bytes)
        }
        _ => Value::Text(v.to_string()),
    })
}

impl App {
    /// `=` (`Pick::Only`) and `!` (`Pick::Exclude`) on the active table tab's grid: the filter
    /// by the value of the cursor's cell (or of the cells selected in one column) runs; why not,
    /// said, when there is none.
    pub(super) fn grid_filter(&mut self, pick: Pick) {
        let id = self.tab().id;
        self.close_search_prompt();
        if self.tab_busy(id) {
            return self.flash_busy();
        }
        match self.grid_pick(id, pick) {
            Ok(p) => self.grid_filter_run(p.text),
            Err(off) => self.flash(off.notice),
        }
    }

    /// The filter `text` (a filter by value, written from the cells of the rows shown) becomes
    /// the draft's and runs, as the filter dialog runs it. Nothing is written again from the
    /// cells. Refused while the tab runs, or while the rows shown are not those of the query
    /// applied.
    pub(super) fn grid_filter_run(&mut self, text: String) {
        let id = self.tab().id;
        if self.tab_busy(id) {
            return self.flash_busy();
        }
        if !self.rows_shown_applied(id) {
            return self.flash(Notice::new(Label::TableFilterByValueNotShown, Level::Warning));
        }
        let Some(tq) = self.table_query(id) else { return };
        let (Some(target), Some(shown)) = (self.table_target(id), tq.shown.clone()) else { return };
        let recognised = query::recognise(&text, &target);
        let filter = (!text.trim().is_empty()).then_some(Filter { text, recognised });
        let Some(tq) = self.table_query_mut(id) else { return };
        tq.draft = Query { filter, ..shown };
        tq.refusal = None;
        let q = tq.draft.clone();
        self.table_run(id, q);
    }

    /// The cells of table tab `id`'s grid a filter by value takes its values from (the
    /// selection's, else the cursor's): their column and rows; or why there are none.
    fn value_cells(&self, id: TabId) -> Result<(usize, Range<usize>), Box<Off>> {
        let off = |short: Label, l: Label| Box::new(Off::new(short, Notice::new(l, Level::Warning)));
        let no_rows = || off(Label::TableFilterByValueOffNoRows, Label::TableFilterByValueNoRows);
        let t = self
            .tabs
            .get(id)
            .filter(|t| t.is_table())
            .ok_or_else(|| off(Label::MenuOffOnlyTableTabs, Label::TableFilterOnlyTableTabs))?;
        let tq = t.doc.table_query.as_ref().ok_or_else(no_rows)?;
        tq.shown.as_ref().ok_or_else(no_rows)?;
        let Results::Rows(rs) = &t.results else { return Err(no_rows()) };
        if rs.rows.is_empty() || rs.columns.is_empty() || t.exec.view != super::tabs::ResultView::Rows {
            return Err(no_rows());
        }
        // Nothing is added to the filter of rows that are not of the query applied.
        if !self.rows_shown_applied(id) {
            return Err(off(Label::TableFilterByValueOffNotShown, Label::TableFilterByValueNotShown));
        }
        let g = &t.grid;
        if !g.shown(g.col) {
            return Err(Box::new(Off::said(self.no_cell_notice())));
        }
        let len = rs.rows.len();
        let on_new = g.row >= len || g.anchor.filter(|_| g.shape != Shape::Cols).is_some_and(|a| a.0 >= len);
        if on_new {
            return Err(off(Label::TableFilterByValueOffNewRow, Label::TableFilterByValueNewRows));
        }
        let cursor = (g.row, g.col.min(rs.columns.len() - 1));
        let (rows, cols) = match g.anchor {
            Some(a) => g.shape.span(a, cursor, len, rs.columns.len()),
            None => (cursor.0..cursor.0 + 1, cursor.1..cursor.1 + 1),
        };
        // The columns the grid shows of the span: whole rows over one shown column are its.
        match g.shown_in(cols)[..] {
            [col] => Ok((col, rows)),
            _ => Err(off(Label::TableFilterByValueOffOneColumn, Label::TableFilterByValueOneColumn)),
        }
    }

    /// The filter by value `pick` of table tab `id`'s cursor cell or selection, written into
    /// the filter of the rows shown, or why there is none (what an entry dimmed shows, and what
    /// its key says).
    ///
    /// Only the value (`=`): the column's own conditions (those the dialog shows as its rows)
    /// are replaced by `= value`, by `IS NULL` for a NULL cell, by `IN (…)` for several values
    /// (with `OR … IS NULL` when NULL is among them); every other term stays as written.
    /// Everything but the value (`!`) is added to them: `IS DISTINCT FROM` (MySQL `NOT (… <=>
    /// …)`) so that rows whose value is NULL stay, `IS NOT NULL` for a NULL cell, `NOT IN (…)`
    /// for several values, rows whose value is NULL kept unless NULL is among the cells.
    pub(crate) fn grid_pick(&self, id: TabId, pick: Pick) -> Result<Picked, Box<Off>> {
        let warn = |l: Label| Notice::new(l, Level::Warning);
        let (col, rows) = self.value_cells(id)?;
        let no_rows = || Box::new(Off::new(Label::TableFilterByValueOffNoRows, warn(Label::TableFilterByValueNoRows)));
        let t = self.tabs.get(id).ok_or_else(no_rows)?;
        let Results::Rows(rs) = &t.results else { return Err(no_rows()) };
        let shown = t.doc.table_query.as_ref().and_then(|q| q.shown.as_ref());
        let (Some(shown), Some(target)) = (shown, self.table_target(id)) else { return Err(no_rows()) };
        let m = &rs.columns[col].meta;
        let d = target.dialect;
        let column = m.name.clone();
        let cells = rows.len();
        // A filter is shown on one line: a name with a line break would not read back as the
        // same name there (nor run as it is shown).
        let not_one_line = || {
            let n = Msg::TableFilterByValueName { column: crate::text::escape_controls(&column) };
            Box::new(Off::new(Label::TableFilterByValueOffName, Notice::new(n, Level::Warning)))
        };
        if !TextInput::holds(&column) {
            return Err(not_one_line());
        }
        if cells > MAX_VALUES {
            let max = datarig_i18n::fmt_count(MAX_VALUES as u64);
            let n = Notice::new(Msg::TableFilterByValueTooMany { max }, Level::Warning);
            return Err(Box::new(Off::new(Label::TableFilterByValueOffTooMany, n)));
        }
        // The values as read: an edit over one, or rows that cannot be read back, give none.
        if rows.clone().any(|r| rs.over(r, col).is_some()) {
            return Err(Box::new(Off::new(Label::TableFilterByValueOffEdited, warn(Label::TableFilterByValueEdited))));
        }
        let unread = || Box::new(Off::new(Label::TableFilterByValueOffUnread, warn(Label::TableFilterByValueUnread)));
        let mut read: Vec<Option<String>> = Vec::with_capacity(cells);
        rs.rows
            .for_each_chunk(rows.clone(), 256, |chunk| read.extend(chunk.iter().map(|r| r.get(col).cloned().flatten())))
            .map_err(|_| unread())?;
        if read.len() != cells {
            return Err(unread());
        }
        // Each value once, in the order selected; NULL apart.
        let mut seen = std::collections::HashSet::new();
        let distinct: Vec<&str> = read.iter().flatten().map(String::as_str).filter(|v| seen.insert(*v)).collect();
        let null = read.iter().any(Option::is_none);
        // A value compares only on a column whose values have equality; NULL alone is tested
        // with `IS NULL`, whatever the type.
        if !distinct.is_empty() {
            let enum_known = self
                .table_structure_of(id)
                .and_then(|s| s.columns.iter().find(|c| c.name == column))
                .is_some_and(|c| !c.enum_labels.is_empty());
            query::compares(d, m, enum_known)
                .map_err(|_| Box::new(Off::new(Label::TableFilterByValueOffType, self.by_value_type_notice(m))))?;
        }
        let mut values = distinct.iter().map(|v| self.by_value_value(t, rs, col, v)).collect::<Result<Vec<_>, _>>()?;
        let row = match (pick, values.len(), null) {
            (Pick::Only, 0, _) => Row::new(column.clone(), Op::IsNull),
            (Pick::Exclude, 0, _) => Row::new(column.clone(), Op::IsNotNull),
            (Pick::Only, 1, false) => Row { values, ..Row::new(column.clone(), Op::Eq) },
            (Pick::Exclude, 1, false) => Row { values, ..Row::new(column.clone(), Op::NeOrNull) },
            (pick, _, null) => {
                if null {
                    values.push(Value::Null);
                }
                let op = if pick == Pick::Only { Op::InList } else { Op::NotInList };
                Row { values, keep_nulls: !null, ..Row::new(column.clone(), op) }
            }
        };
        let cond = row.cond().ok_or_else(no_rows)?;
        query::render(&cond, &target).map_err(|e| Box::new(self.render_off(e, &target, m)))?;
        let mut terms = Where::from_text(filter_text(shown).unwrap_or_default(), &target);
        match pick {
            Pick::Only => terms.replace_column(row, &target),
            Pick::Exclude => terms.add(row, &target),
        };
        let text = terms.text();
        if text.len() > FIELD_CAP {
            return Err(Box::new(Off::new(
                Label::TableFilterByValueOffTooLong,
                warn(Label::TableFilterByValueTooLong),
            )));
        }
        if !TextInput::holds(&text) {
            return Err(Box::new(Off::new(
                Label::TableFilterByValueOffLineBreak,
                warn(Label::TableFilterByValueNotOneLine),
            )));
        }
        Ok(Picked { column, shown: self.picked_values(m, &distinct, null), text })
    }

    /// The values of a filter by value as its menu entry names them: one value as the grid
    /// shows it (text quoted), several as a list cut with `…`, NULL as `NULL`.
    fn picked_values(&self, m: &ColumnMeta, distinct: &[&str], null: bool) -> String {
        // As the grid shows it: bytes in their hexadecimal form, text with its control
        // characters spelled out.
        let shown_value = |v: &str| {
            let v = if m.kind == ValueKind::Bytes { v.to_string() } else { crate::text::escape_controls(v) };
            match m.kind.is_number() {
                true => cut(&v, VALUE_W),
                false => format!("'{}'", cut(&v, VALUE_W)),
            }
        };
        let mut list = String::new();
        for (i, v) in distinct.iter().enumerate() {
            let next = if i == 0 { shown_value(v) } else { format!("{list}, {}", shown_value(v)) };
            if crate::text::width(&next) > VALUES_W && i > 0 {
                list.push_str(", …");
                break;
            }
            list = next;
        }
        match (list.is_empty(), null) {
            (true, _) => "NULL".to_string(),
            (false, true) => format!("{list}, NULL"),
            (false, false) => list,
        }
    }

    /// Why a condition on column `m` of table `target` is not written ([`query::render`]), as an
    /// entry dimmed shows it and its key says it.
    fn render_off(&self, e: RenderError, target: &Target, m: &ColumnMeta) -> Off {
        let column = m.name.clone();
        let warn = |n: Msg| Notice::new(n, Level::Warning);
        match e {
            RenderError::CaseInsensitiveRefused(_) => {
                let collation = target.column(&column).and_then(|f| f.collation.clone());
                let n = match collation {
                    Some(collation) => Msg::TableFilterByValueNondeterministic { collation },
                    None => Msg::TableFilterByValueNondeterministicColumn { column },
                };
                Off::new(Label::TableFilterByValueOffCollation, warn(n))
            }
            RenderError::CollationUnknown(_) => Off::new(
                Label::TableFilterByValueOffCollationUnknown,
                warn(Msg::TableFilterByValueCollationUnknown { column }),
            ),
            RenderError::LikeRefused(_) => {
                Off::new(Label::TableFilterByValueOffCollation, warn(Msg::TableFilterByValueLikeRefused { column }))
            }
            RenderError::LikeUnknown(_) => Off::new(
                Label::TableFilterByValueOffCollationUnknown,
                warn(Msg::TableFilterByValueLikeUnknown { column }),
            ),
            _ => Off::new(Label::TableFilterByValueOffType, self.by_value_type_notice(m)),
        }
    }

    /// Whether tab `t`'s server reads a time zone offset in a date and time literal, as a session
    /// of its profile said (`DbEvent::ZoneOffsets`); not known is no.
    pub(super) fn zone_offsets(&self, t: &Tab) -> bool {
        t.profile.and_then(|p| self.conns.get(p)).and_then(|c| c.zone_offsets) == Some(true)
    }

    /// Value `v` of column `col` of result `rs` (as read, not NULL) as the filter compares it,
    /// or why it is not taken ([`admit`], with the rows' session values and the tab's now).
    fn by_value_value(
        &self,
        t: &Tab,
        rs: &crate::widgets::grid::ResultSet,
        col: usize,
        v: &str,
    ) -> Result<Value, Box<Off>> {
        let m = &rs.columns[col].meta;
        let from = ValuesFrom { dialect: self.tab_dialect(t.id), meta: m, read_in: &rs.read_in };
        let zone = self.tab_keys(t).catalog().and_then(|k| k.server_zone());
        admit(&from, zone, t.exec.values.as_ref(), v).map_err(|why| {
            let n = match &why {
                NotAdmitted::NotWhole(r) => {
                    let reason = self.not_whole_reason(&r.why);
                    Notice::new(Msg::TableFilterByValueNotWhole { column: r.column.clone(), reason }, Level::Warning)
                }
                NotAdmitted::TooBig => {
                    Notice::new(Msg::TableFilterByValueTooBig { column: m.name.clone() }, Level::Warning)
                }
            };
            let short = match why {
                NotAdmitted::NotWhole(_) => Label::TableFilterByValueOffNotWhole,
                NotAdmitted::TooBig => Label::TableFilterByValueOffTooBig,
            };
            Box::new(Off::new(short, n))
        })
    }

    /// Why column `m`'s values are not compared by a filter by value, in full.
    fn by_value_type_notice(&self, m: &ColumnMeta) -> Notice {
        let (column, ty) = (m.name.clone(), m.type_name.clone());
        Notice::new(Msg::TableFilterByValueType { column, ty }, Level::Warning)
    }
}

#[cfg(test)]
mod tests;
