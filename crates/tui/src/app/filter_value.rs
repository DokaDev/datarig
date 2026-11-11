//! Filter a table tab by a cell's value (`F`, `Space r F`, and "filter by value" in the grid's
//! menu): a menu of conditions on the cursor's cell, or on the cells selected in one column, each
//! of which adds its condition to the filter of the rows shown and runs the tab again.
//!
//! The condition is written by the query model ([`query::render`]: literals for the column's
//! kind through the dialect, NULL compared only with `IS NULL` or the NULL-safe forms, patterns
//! escaped with `!`) and added to the filter of the rows shown with `AND`, a filter whose top
//! level is not a plain chain of `AND`s put in parentheses first ([`query::and_filter`]). The new
//! text runs as the filter dialog runs a filter ([`App::table_run`]: the checks, the questions
//! about functions, row locks and pending edits, read-only profiles); the dialog shows it after.
//!
//! A value is taken only as the server holds it: a cell shown otherwise than its value (as a copy
//! as SQL judges it), a new row, a cell with an edit on it, or rows that cannot be read back give
//! no filter. An entry that cannot apply to the value (a type whose comparison is not known,
//! binary data over 64 KiB, more than 1000 cells) is shown dimmed with why. Nothing is added
//! while the query applied is one whose run brought no rows (refused, cancelled), over the
//! earlier rows still shown.

use super::menu::{MenuItem, MenuTarget, Off};
use super::table_query::{FIELD_CAP, filter_text, refusal_why};
use super::*;
use crate::widgets::grid::Shape;
use crate::widgets::text_input::TextInput;
use datarig_core::driver::{ColumnMeta, ReadIn, ValueKind};
use datarig_core::export;
use datarig_core::query::{self, CaseMode, CmpOp, Cond, Filter, Query, RenderError, Value};

/// The most cells of a selection a filter takes values from.
pub const MAX_VALUES: usize = 1000;

/// The largest binary value a filter takes, in bytes.
pub const MAX_BYTES: usize = 64 * 1024;

/// Display columns of a value in a menu entry's label (cut with `…` past them).
const VALUE_W: usize = 24;

/// Display columns of the values of a list in a menu entry's label.
const VALUES_W: usize = 32;

/// What an entry of the menu filters by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ByValue {
    /// `= value`
    Eq,
    /// `<> value` (rows whose value is NULL are left out).
    Ne,
    /// Not equal, NULL-safe: rows whose value is NULL are kept.
    NeNulls,
    Lt,
    Gt,
    Le,
    Ge,
    IsNull,
    IsNotNull,
    /// One of the values selected (NULL among them tested on its own).
    In,
    /// None of the values selected (rows whose value is NULL are left out).
    NotIn,
    /// None of the values selected, rows whose value is NULL kept.
    NotInNulls,
    /// Text that starts with the value (`LIKE 'value%'`).
    StartsWith,
    /// Text that contains the value, whatever the case.
    Contains,
}

/// An entry of the menu as built: what it filters by, its label, a hint shown with it, and the
/// filter text it runs (or why it cannot).
pub struct Entry {
    pub by: ByValue,
    pub label: Localized,
    pub hint: Option<Localized>,
    pub text: Result<String, Off>,
}

/// The menu for the cursor's cell or the selection: the column's name (its heading) and the
/// entries.
pub struct ByValueMenu {
    pub column: String,
    pub cells: usize,
    pub entries: Vec<Entry>,
}

/// What a column's values compare by, as far as a filter writes them: `Ok(true)` with an order
/// too (`<`, `>`), `Ok(false)` equality only; `Err` with why not ([`query::compares`]).
fn compares(d: Dialect, m: &ColumnMeta, enum_known: bool) -> Result<bool, Label> {
    match query::compares(d, m, enum_known) {
        Ok(c) => Ok(c == query::Compares::Order),
        Err(query::NoEquality) => Err(Label::TableFilterByValueOffType),
    }
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

impl App {
    /// `F`: the menu of filters by the value of the active table tab's cursor cell or selection,
    /// next to the cell; why not, said, when no filter can be built there.
    pub(super) fn filter_by_value(&mut self) {
        let id = self.tab().id;
        self.close_search_prompt();
        // The rows shown are about to be replaced: a menu of their cells would be out of date.
        if self.tab().is_table() && self.tab_busy(id) {
            return self.flash_busy();
        }
        // The rows first: the menu is about the grid's cells.
        if self.chart_shown() || self.plan_shown() || self.changes_shown() {
            self.tab_mut().exec.view = super::tabs::ResultView::Rows;
        }
        if self.focus != Focus::Inspector {
            self.focus = Focus::Results;
        }
        let menu = match self.by_value_menu(id) {
            Ok(m) => m,
            Err(why) => return self.flash(why),
        };
        let (Some(grid), Some(result)) = (self.menu_grid_target(), self.grid_result_id()) else { return };
        let title = match menu.cells {
            1 => self.i18n.msg(&Msg::TableFilterByValueTitle { column: crate::text::escape_controls(&menu.column) }),
            n => self.i18n.msg(&Msg::TableFilterByValueTitleMany {
                column: crate::text::escape_controls(&menu.column),
                count: n as u64,
            }),
        };
        let at = self.menu_at_cell();
        self.open_by_value_menu(MenuTarget::Filter { grid: Box::new(grid), result }, title, menu.entries, at);
    }

    /// An entry of the menu was picked, on what it was opened on still (the same cells of the same
    /// result): `text`, the filter it showed, becomes the draft's and runs, as the filter dialog
    /// runs it. Nothing is written again from the cells. Refused while the tab runs, or while
    /// the rows shown are not those of the query applied.
    pub(super) fn filter_by_value_run(&mut self, text: String) {
        let id = self.tab().id;
        if self.tab_busy(id) {
            return self.flash_busy();
        }
        if !self.rows_shown_applied(id) {
            return self.flash(Notice::new(Label::TableFilterByValueNotShown, Level::Warning));
        }
        let Some(tq) = self.table_query(id) else { return };
        let (Some(target), Some(shown)) = (self.table_target(id), tq.shown.clone()) else { return };
        // Only a text shown on one line as it is runs: the dialog shows what runs.
        if !TextInput::holds(&text) {
            return self.flash(Notice::new(Label::TableFilterByValueNotOneLine, Level::Warning));
        }
        let recognised = query::recognise(&text, &target);
        let Some(tq) = self.table_query_mut(id) else { return };
        tq.draft = Query { filter: Some(Filter { text, recognised }), ..shown };
        tq.refusal = None;
        let q = tq.draft.clone();
        self.table_run(id, q);
    }

    /// The menu of filters by value for table tab `id`'s cursor cell or selection, or why there
    /// is none (said as it is).
    pub(super) fn by_value_menu(&self, id: TabId) -> Result<ByValueMenu, Notice> {
        let warn = |l: Label| Notice::new(l, Level::Warning);
        let t = self.tabs.get(id).filter(|t| t.is_table()).ok_or_else(|| warn(Label::TableFilterOnlyTableTabs))?;
        let tq = t.doc.table_query.as_ref().ok_or_else(|| warn(Label::TableFilterByValueNoRows))?;
        let shown = tq.shown.as_ref().ok_or_else(|| warn(Label::TableFilterByValueNoRows))?;
        let Results::Rows(rs) = &t.results else { return Err(warn(Label::TableFilterByValueNoRows)) };
        if rs.rows.is_empty() || rs.columns.is_empty() || t.exec.view != super::tabs::ResultView::Rows {
            return Err(warn(Label::TableFilterByValueNoRows));
        }
        // Nothing is added to the filter of rows that are not of the query applied.
        if !self.rows_shown_applied(id) {
            return Err(warn(Label::TableFilterByValueNotShown));
        }
        // The cells: the selection's, else the cursor's; read rows of one column only.
        let g = &t.grid;
        let len = rs.rows.len();
        let on_new = g.row >= len || g.anchor.filter(|_| g.shape != Shape::Cols).is_some_and(|a| a.0 >= len);
        if on_new {
            return Err(warn(Label::TableFilterByValueNewRows));
        }
        let cursor = (g.row, g.col.min(rs.columns.len() - 1));
        let (rows, cols) = match g.anchor {
            Some(a) => g.shape.span(a, cursor, len, rs.columns.len()),
            None => (cursor.0..cursor.0 + 1, cursor.1..cursor.1 + 1),
        };
        if cols.len() != 1 {
            return Err(warn(Label::TableFilterByValueOneColumn));
        }
        let col = cols.start;
        let m = &rs.columns[col].meta;
        let target = self.table_target(id).ok_or_else(|| warn(Label::TableFilterByValueNoRows))?;
        let d = target.dialect;
        let column = m.name.clone();
        let cells = rows.len();
        let base = filter_text(shown).unwrap_or_default();
        // A filter is shown on one line: a name with a line break would not read back as the
        // same name there (nor run as it is shown).
        let not_one_line = || {
            let n = Msg::TableFilterByValueName { column: crate::text::escape_controls(&column) };
            Off::new(Label::TableFilterByValueOffName, Notice::new(n, Level::Warning))
        };
        // The filter text of `c`, or why not.
        let text_of = |c: &Cond| -> Result<String, Off> {
            if !TextInput::holds(&column) {
                return Err(not_one_line());
            }
            let cond = query::render(c, &target).map_err(|e| match e {
                RenderError::CaseInsensitiveRefused(_) => {
                    let collation = target.column(&column).and_then(|f| f.collation.clone());
                    let n = match collation {
                        Some(collation) => Msg::TableFilterByValueNondeterministic { collation },
                        None => Msg::TableFilterByValueNondeterministicColumn { column: column.clone() },
                    };
                    Off::new(Label::TableFilterByValueOffCollation, Notice::new(n, Level::Warning))
                }
                RenderError::CollationUnknown(_) => {
                    let n =
                        Notice::new(Msg::TableFilterByValueCollationUnknown { column: column.clone() }, Level::Warning);
                    Off::new(Label::TableFilterByValueOffCollationUnknown, n)
                }
                RenderError::LikeRefused(_) => {
                    let n = Notice::new(Msg::TableFilterByValueLikeRefused { column: column.clone() }, Level::Warning);
                    Off::new(Label::TableFilterByValueOffCollation, n)
                }
                RenderError::LikeUnknown(_) => {
                    let n = Notice::new(Msg::TableFilterByValueLikeUnknown { column: column.clone() }, Level::Warning);
                    Off::new(Label::TableFilterByValueOffCollationUnknown, n)
                }
                _ => Off::new(Label::TableFilterByValueOffType, self.by_value_type_notice(m)),
            })?;
            // The filter shown is one condition (it ran): refused, never repaired, when not.
            let text = query::and_filter(base, &cond, d).map_err(|r| {
                let why = refusal_why(&self.i18n, &r);
                Off::new(
                    Label::TableFilterByValueOffBase,
                    Notice::new(Msg::TableFilterByValueBase { why }, Level::Warning),
                )
            })?;
            match text.len() > FIELD_CAP {
                true => Err(Off::new(Label::TableFilterByValueOffTooLong, warn(Label::TableFilterByValueTooLong))),
                false if !TextInput::holds(&text) => Err(not_one_line()),
                false => Ok(text),
            }
        };
        // Whether the column's values compare, and with an order.
        let enum_known = self
            .table_structure_of(id)
            .and_then(|s| s.columns.iter().find(|c| c.name == column))
            .is_some_and(|c| !c.enum_labels.is_empty());
        let compares = compares(d, m, enum_known).map_err(|short| Off::new(short, self.by_value_type_notice(m)));
        let ordered = compares.clone().and_then(|o| match o {
            true => Ok(()),
            false => Err(Off::new(Label::TableFilterByValueOffOrder, self.by_value_order_notice(m))),
        });
        let equality = compares.map(drop);
        let col_of = || column.clone();
        let mut entries = Vec::new();
        let entry = |by: ByValue, label: Localized, hint: Option<Localized>, text: Result<String, Off>| Entry {
            by,
            label,
            hint,
            text,
        };
        let label = |l: Label| self.i18n.label(l);
        if cells > MAX_VALUES {
            let off = || {
                let max = datarig_i18n::fmt_count(MAX_VALUES as u64);
                let n = Notice::new(Msg::TableFilterByValueTooMany { max }, Level::Warning);
                Off::new(Label::TableFilterByValueOffTooMany, n)
            };
            let many = Localized::verbatim("…");
            entries.push(entry(
                ByValue::In,
                self.i18n.msg(&Msg::TableFilterByValueIn { values: many.to_string() }),
                None,
                Err(off()),
            ));
            entries.push(entry(
                ByValue::NotIn,
                self.i18n.msg(&Msg::TableFilterByValueNotIn { values: many.to_string() }),
                None,
                Err(off()),
            ));
            return Ok(ByValueMenu { column, cells, entries });
        }
        // The values as read: an edit over one, or rows that cannot be read back, give none.
        if rows.clone().any(|r| rs.over(r, col).is_some()) {
            return Err(warn(Label::TableFilterByValueEdited));
        }
        let mut cells_read: Vec<Option<String>> = Vec::with_capacity(cells);
        rs.rows
            .for_each_chunk(rows.clone(), 256, |chunk| {
                cells_read.extend(chunk.iter().map(|r| r.get(col).cloned().flatten()));
            })
            .map_err(|_| warn(Label::TableFilterByValueUnread))?;
        if cells_read.len() != cells {
            return Err(warn(Label::TableFilterByValueUnread));
        }
        // Each value once, in the order selected; NULL apart.
        let mut seen = std::collections::HashSet::new();
        let distinct: Vec<&str> = cells_read.iter().flatten().map(String::as_str).filter(|v| seen.insert(*v)).collect();
        let null = cells_read.iter().any(Option::is_none);
        let values: Result<Vec<Value>, Off> =
            distinct.iter().map(|v| self.by_value_value(t, rs, col, v).map_err(|o| *o)).collect();
        // As the grid shows it: bytes in their hexadecimal form, text with its control
        // characters spelled out.
        let shown_value = |v: &str| {
            let v = if m.kind == ValueKind::Bytes { v.to_string() } else { crate::text::escape_controls(v) };
            match m.kind.is_number() {
                true => cut(&v, VALUE_W),
                false => format!("'{}'", cut(&v, VALUE_W)),
            }
        };
        let float = m.kind == ValueKind::Float;
        let text_col = m.kind == ValueKind::Text;
        let hint = |l: Label| Some(label(l));
        if distinct.is_empty() {
            // Only NULL: tested as NULL, never with `=`.
            for (by, l, negated) in [
                (ByValue::IsNull, Label::TableFilterByValueIsNull, false),
                (ByValue::IsNotNull, Label::TableFilterByValueIsNotNull, true),
            ] {
                let c = Cond::IsNull { col: col_of(), negated };
                entries.push(entry(by, label(l), None, text_of(&c)));
            }
            return Ok(ByValueMenu { column, cells, entries });
        }
        if cells == 1 {
            let v = distinct[0];
            let shown = shown_value(v);
            let value = values.map(|mut v| v.remove(0));
            let with = |ok: &Result<(), Off>, c: &dyn Fn(Value) -> Cond| -> Result<String, Off> {
                ok.clone()?;
                text_of(&c(value.clone()?))
            };
            let cmp = |op: CmpOp| move |v: Value| Cond::Cmp { col: col_of(), op, value: v };
            let case_hint = if text_col { hint(Label::TableFilterByValueHintServerCase) } else { None };
            let float_hint = if float { hint(Label::TableFilterByValueHintFloat) } else { None };
            entries.push(entry(
                ByValue::Eq,
                self.i18n.msg(&Msg::TableFilterByValueEq { value: shown.clone() }),
                float_hint.clone().or(case_hint.clone()),
                with(&equality, &cmp(CmpOp::Eq)),
            ));
            entries.push(entry(
                ByValue::Ne,
                self.i18n.msg(&Msg::TableFilterByValueNe { value: shown.clone() }),
                float_hint.clone().or(hint(Label::TableFilterByValueHintDropsNulls)),
                with(&equality, &cmp(CmpOp::Ne)),
            ));
            entries.push(entry(
                ByValue::NeNulls,
                self.i18n.msg(&Msg::TableFilterByValueNeNulls { value: shown.clone() }),
                float_hint,
                with(&equality, &|v| Cond::NotDistinct { col: col_of(), value: v, negated: true }),
            ));
            for (by, op, msg) in [
                (ByValue::Lt, CmpOp::Lt, Msg::TableFilterByValueLt { value: shown.clone() }),
                (ByValue::Gt, CmpOp::Gt, Msg::TableFilterByValueGt { value: shown.clone() }),
                (ByValue::Le, CmpOp::Le, Msg::TableFilterByValueLe { value: shown.clone() }),
                (ByValue::Ge, CmpOp::Ge, Msg::TableFilterByValueGe { value: shown.clone() }),
            ] {
                entries.push(entry(by, self.i18n.msg(&msg), None, with(&ordered, &cmp(op))));
            }
            if text_col {
                let like = |pattern: String, case: CaseMode| -> Result<String, Off> {
                    equality.clone()?;
                    value.clone()?;
                    text_of(&Cond::Like { col: col_of(), pattern, negated: false, case })
                };
                entries.push(entry(
                    ByValue::StartsWith,
                    self.i18n.msg(&Msg::TableFilterByValueStartsWith { value: shown.clone() }),
                    case_hint,
                    like(query::like_starts_with(v), CaseMode::Server),
                ));
                // PostgreSQL: `LIKE` on a collation known deterministic or a server known to take
                // it on any (18 and later), `ILIKE` only on a collation known deterministic, else
                // dimmed with why; MySQL: a collation not known by name is lowered on both sides.
                let known = target.column(&column).is_some_and(|f| f.collation.is_some());
                let contains_hint = match d {
                    Dialect::MySql(_) if !known => hint(Label::TableFilterByValueHintCollationLower),
                    _ => None,
                };
                entries.push(entry(
                    ByValue::Contains,
                    self.i18n.msg(&Msg::TableFilterByValueContains { value: shown }),
                    contains_hint,
                    like(query::like_contains(v), CaseMode::Insensitive),
                ));
            }
            return Ok(ByValueMenu { column, cells, entries });
        }
        // Several cells: the values as a list, NULL tested on its own.
        let mut list = String::new();
        for (i, v) in distinct.iter().enumerate() {
            let next = if i == 0 { shown_value(v) } else { format!("{list}, {}", shown_value(v)) };
            if crate::text::width(&next) > VALUES_W && i > 0 {
                list.push_str(", …");
                break;
            }
            list = next;
        }
        if null {
            list.push_str(", NULL");
        }
        let in_cond = |negated: bool, with_null: bool| -> Result<Cond, Off> {
            let mut vs = values.clone()?;
            if with_null {
                vs.push(Value::Null);
            }
            Ok(Cond::In { col: col_of(), values: vs, negated })
        };
        let listed = |c: Result<Cond, Off>| -> Result<String, Off> {
            equality.clone()?;
            text_of(&c?)
        };
        entries.push(entry(
            ByValue::In,
            self.i18n.msg(&Msg::TableFilterByValueIn { values: list.clone() }),
            None,
            listed(in_cond(false, null)),
        ));
        // With NULL selected, NOT IN leaves out the NULL rows as asked (and says it in its text);
        // without, plain NOT IN leaves them out too (said), and the other one keeps them.
        entries.push(entry(
            ByValue::NotIn,
            self.i18n.msg(&Msg::TableFilterByValueNotIn { values: list.clone() }),
            (!null).then(|| label(Label::TableFilterByValueHintDropsNulls)),
            listed(in_cond(true, null)),
        ));
        if !null {
            let keeps =
                in_cond(true, false).map(|c| Cond::Any(vec![c, Cond::IsNull { col: col_of(), negated: false }]));
            entries.push(entry(
                ByValue::NotInNulls,
                self.i18n.msg(&Msg::TableFilterByValueNotInNulls { values: list }),
                None,
                listed(keeps),
            ));
        }
        Ok(ByValueMenu { column, cells, entries })
    }

    /// Value `v` of column `col` of result `rs` (as read, not NULL) as the filter compares it,
    /// or why it is not taken: shown otherwise than the server holds it (as a copy as SQL judges
    /// it), read in another time zone than the session's now, or binary data over
    /// [`MAX_BYTES`]. A MySQL `TIMESTAMP` carries the offset of the zone it was read in.
    fn by_value_value(
        &self,
        t: &Tab,
        rs: &crate::widgets::grid::ResultSet,
        col: usize,
        v: &str,
    ) -> Result<Value, Box<Off>> {
        let m = &rs.columns[col].meta;
        let d = self.tab_dialect(t.id);
        let keys = self.tab_keys(t).catalog();
        let cols = [export::SqlColumn { name: &m.name, type_name: &m.type_name, kind: m.kind, auto_increment: false }];
        let not_whole = |r: &export::Refused| {
            let reason = self.not_whole_reason(&r.why);
            let n = Notice::new(Msg::TableFilterByValueNotWhole { column: r.column.clone(), reason }, Level::Warning);
            Box::new(Off::new(Label::TableFilterByValueOffNotWhole, n))
        };
        let cells =
            export::sql_cells(d, &cols, &rs.read_in, keys.and_then(|k| k.server_zone())).map_err(|r| not_whole(&r))?;
        let cell = cells.cell(0, v).map_err(|r| not_whole(&r))?;
        let mysql = matches!(d, Dialect::MySql(_));
        // A MySQL TIMESTAMP is shown in the session's zone: the session reads it back the same
        // only in the zone it was read in.
        if mysql && m.kind == ValueKind::TimestampTz {
            let read = match &rs.read_in {
                ReadIn::Values(s) => Some(&s.time_zone),
                _ => None,
            };
            if read.is_none() || t.exec.values.as_ref().map(|s| &s.time_zone) != read {
                let why = export::Refused { column: m.name.clone(), why: export::NotWhole::SessionChanged };
                return Err(not_whole(&why));
            }
        }
        Ok(match m.kind {
            ValueKind::Integer | ValueKind::Decimal | ValueKind::Float => Value::Number(v.to_string()),
            // With the offset of the zone it was read in (as a copy as SQL writes it): the same
            // instant in any session's zone, as the filter is run again from the history.
            ValueKind::TimestampTz if mysql => Value::Text(cell.into_owned()),
            ValueKind::Bytes => {
                let bytes = hex_bytes(v, if mysql { "0x" } else { "\\x" }).ok_or_else(|| {
                    not_whole(&export::Refused { column: m.name.clone(), why: export::NotWhole::NotShownWhole })
                })?;
                if bytes.len() > MAX_BYTES {
                    let n = Notice::new(Msg::TableFilterByValueTooBig { column: m.name.clone() }, Level::Warning);
                    return Err(Box::new(Off::new(Label::TableFilterByValueOffTooBig, n)));
                }
                Value::Bytes(bytes)
            }
            _ => Value::Text(v.to_string()),
        })
    }

    /// Why column `m`'s values are not compared by a filter by value, in full.
    fn by_value_type_notice(&self, m: &ColumnMeta) -> Notice {
        let (column, ty) = (m.name.clone(), m.type_name.clone());
        Notice::new(Msg::TableFilterByValueType { column, ty }, Level::Warning)
    }

    /// Why column `m`'s values are compared for equality only, in full.
    fn by_value_order_notice(&self, m: &ColumnMeta) -> Notice {
        let (column, ty) = (m.name.clone(), m.type_name.clone());
        Notice::new(Msg::TableFilterByValueOrder { column, ty }, Level::Warning)
    }

    /// An entry of the open filter menu runs (from [`App::menu_run`]), with the filter `text`
    /// the menu showed for it (`None`: it had none, nothing runs).
    pub(super) fn by_value_item(&mut self, text: Option<String>) {
        if let Some(text) = text {
            self.filter_by_value_run(text);
        }
    }

    /// Open the filter menu of `entries` for `target` at `at`, headed `title`: the entries that
    /// cannot apply dimmed with why, the others with their hints.
    fn open_by_value_menu(&mut self, target: MenuTarget, title: Localized, entries: Vec<Entry>, at: (u16, u16)) {
        let mut items = Vec::new();
        let mut off = Vec::new();
        let mut names = Vec::new();
        let mut hints = Vec::new();
        let mut filters = Vec::new();
        for e in entries {
            let item = MenuItem::Filter(e.by);
            items.push(item);
            names.push((item, e.label));
            if let Some(h) = e.hint {
                hints.push((item, h));
            }
            match e.text {
                Ok(text) => filters.push((item, text)),
                Err(o) => off.push((item, o)),
            }
        }
        self.push_filter_menu(target, title.to_string(), (items, off, names, hints, filters), at);
    }
}
