//! The filter dialog's own keys (those no action of the key map takes): moving between its
//! parts, the column list and its search, the conditions and their operator picker, the sort
//! line, the footer's chips, keys and `LIMIT` field. Every change of the draft is one undo
//! step; typing in one field is one step until the field is left.

use super::draft::{self, Arity, Op, Row};
use super::*;
use crate::widgets::text_input::InputResult;
use datarig_core::query::{Dir, Value};

/// The parts `Tab` moves between, in order: the column list, the right pane, the footer's
/// chips, its keys, the `LIMIT` field and the `SQL` toggle; while the SQL section is open, its
/// `WHERE` and `ORDER BY` fields and its `Open in editor` button.
const AREAS: usize = 9;

/// The part of the dialog focus `f` is in ([`AREAS`]).
fn area(f: DialogFocus) -> usize {
    match f {
        DialogFocus::Columns => 0,
        DialogFocus::Op(_) | DialogFocus::Value(..) | DialogFocus::Add | DialogFocus::Values(_) | DialogFocus::Sort => {
            1
        }
        DialogFocus::Chips(_) => 2,
        DialogFocus::Order(_) => 3,
        DialogFocus::Limit => 4,
        DialogFocus::Sql => 5,
        DialogFocus::SqlWhere => 6,
        DialogFocus::SqlOrder => 7,
        DialogFocus::SqlOpen => 8,
    }
}

/// The values of a condition whose operator becomes `op`: kept as they are where the new
/// operator takes values of the same kind and number, else read again from their texts as
/// they would be typed for it.
fn values_for(row: &Row, op: Op, c: &ColumnInfo, d: Dialect) -> Vec<Value> {
    let pattern = |o: Op| matches!(o, Op::Contains | Op::StartsWith);
    if op.arity() == row.op.arity() && pattern(op) == pattern(row.op) {
        return row.values.clone();
    }
    let texts = row.value_texts(d);
    let fields: Vec<String> = match op.arity() {
        Arity::Zero => Vec::new(),
        Arity::One => texts.first().cloned().into_iter().collect(),
        Arity::Two => texts.iter().take(2).cloned().collect(),
        // Each value typed again on its own: a comma inside one does not split it.
        Arity::Many => {
            return texts.iter().filter(|t| !t.is_empty()).flat_map(|t| draft::parse_input(Op::Eq, &[t], c)).collect();
        }
    };
    let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
    draft::parse_input(op, &refs, c)
}

impl App {
    /// A change of the open dialog's draft by `f` (with the table as the app knows it now):
    /// one undo step when the draft changed (typing in `field` coalesces), and what the last
    /// apply was told goes.
    pub(super) fn filter_dialog_edit(&mut self, field: Option<FieldKey>, f: impl FnOnce(&mut FilterDialog, &Target)) {
        let Some(tab) = self.filter_dialog().map(|d| d.tab) else { return };
        let Some(target) = self.table_target(tab) else { return };
        let Some(d) = self.filter_dialog_mut() else { return };
        let before = d.draft.clone();
        f(d, &target);
        if d.draft != before {
            d.undo.record(&before, field);
            d.message = None;
            d.mark = None;
        }
        d.settle();
    }

    /// A key of the open filter dialog that no action took.
    pub(crate) fn filter_dialog_key(&mut self, key: KeyEvent) {
        // The list as it is now: how many values the keys move through.
        self.filter_dialog_list();
        let Some(d) = self.filter_dialog() else { return };
        if d.picker.is_some() {
            return self.filter_dialog_picker_key(key);
        }
        match key.code {
            KeyCode::Tab => return self.filter_dialog_area(true),
            KeyCode::BackTab => return self.filter_dialog_area(false),
            _ => {}
        }
        if d.typing() {
            return self.filter_dialog_text_key(key);
        }
        match d.focus {
            DialogFocus::Columns => self.filter_dialog_columns_key(key),
            DialogFocus::Op(t) => self.filter_dialog_op_key(t, key),
            DialogFocus::Add => match key.code {
                KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => self.filter_dialog_add(),
                _ => self.filter_dialog_pane_key(key),
            },
            DialogFocus::Sort => self.filter_dialog_sort_key(key),
            DialogFocus::Values(i) => self.filter_dialog_values_key(i, key),
            DialogFocus::Chips(i) => self.filter_dialog_chips_key(i, key),
            DialogFocus::Order(i) => self.filter_dialog_order_key(i, key),
            DialogFocus::Sql => {
                if key.code == KeyCode::Char(' ') {
                    self.filter_dialog_toggle_sql();
                }
            }
            DialogFocus::SqlOpen => match key.code {
                KeyCode::Char(' ') => self.filter_dialog_open_sql(),
                KeyCode::Up | KeyCode::Char('k') => self.filter_dialog_sql_move(DialogFocus::SqlOrder),
                _ => {}
            },
            DialogFocus::Value(..) | DialogFocus::Limit | DialogFocus::SqlWhere | DialogFocus::SqlOrder => {}
        }
    }

    /// Text pasted while the dialog is open: into the field that has the keys, on one line, the
    /// field holding at most [`table_query::FIELD_CAP`] bytes.
    pub(crate) fn filter_dialog_paste(&mut self, text: &str) {
        let Some(d) = self.filter_dialog_mut() else { return };
        if !d.typing() {
            return;
        }
        let one_line: String = text.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
        let Some(input) = d.input_mut() else { return };
        let room = table_query::FIELD_CAP.saturating_sub(input.text().len());
        let mut end = one_line.len().min(room);
        while !one_line.is_char_boundary(end) {
            end -= 1;
        }
        input.insert_str(&one_line[..end]);
        let cut = end < one_line.len();
        self.filter_dialog_typed();
        if cut && let Some(d) = self.filter_dialog_mut() {
            d.message = Some(Notice::new(Label::TableFilterPasteCut, Level::Warning));
        }
    }

    /// `Tab` (`forward`) or `Shift+Tab`: the next part of the dialog that has something to
    /// focus.
    fn filter_dialog_area(&mut self, forward: bool) {
        self.filter_dialog_popup_close();
        // A field of the SQL section is read back as it is left; an order refused keeps it.
        if self.filter_dialog().is_some_and(|d| d.sql_field().is_some()) && !self.filter_dialog_read_back() {
            return;
        }
        let Some(d) = self.filter_dialog_mut() else { return };
        let first = |d: &FilterDialog, a: usize| -> Option<DialogFocus> {
            match a {
                0 => Some(DialogFocus::Columns),
                1 => d.pane_lines().first().copied(),
                2 => (!d.draft.where_.chips().is_empty()).then_some(DialogFocus::Chips(0)),
                3 => (!d.draft.order.is_empty()).then_some(DialogFocus::Order(0)),
                4 => Some(DialogFocus::Limit),
                5 => Some(DialogFocus::Sql),
                6 => d.sql.is_some().then_some(DialogFocus::SqlWhere),
                7 => d.sql.is_some().then_some(DialogFocus::SqlOrder),
                _ => d.sql.is_some().then_some(DialogFocus::SqlOpen),
            }
        };
        let mut a = area(d.focus);
        for _ in 0..AREAS {
            a = if forward { (a + 1) % AREAS } else { (a + AREAS - 1) % AREAS };
            if let Some(f) = first(d, a) {
                return d.focus_on(f);
            }
        }
    }

    /// `↑`/`↓` between the SQL section's fields and its button: the field left read back
    /// first (an order refused keeps the keys in it).
    fn filter_dialog_sql_move(&mut self, to: DialogFocus) {
        self.filter_dialog_popup_close();
        if self.filter_dialog().is_some_and(|d| d.sql_field().is_some()) && !self.filter_dialog_read_back() {
            return;
        }
        if let Some(d) = self.filter_dialog_mut() {
            d.focus_on(to);
        }
    }

    /// A key while a text field has the keys: the column search, a value field, the `LIMIT`
    /// field.
    fn filter_dialog_text_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let focus = d.focus;
        let at_start = d.input_mut().is_some_and(|i| i.cursor() == 0);
        let at_end = d.input_mut().is_some_and(|i| i.cursor_byte() == i.text().len());
        match (focus, key.code) {
            (DialogFocus::Columns, KeyCode::Up | KeyCode::Down) => return self.filter_dialog_columns_key(key),
            (DialogFocus::Value(t, k), KeyCode::Left) if at_start => {
                return d.focus_on(if k > 0 { DialogFocus::Value(t, k - 1) } else { DialogFocus::Op(t) });
            }
            (DialogFocus::Value(t, 0), KeyCode::Right) if at_end && d.inputs.len() > 1 => {
                return d.focus_on(DialogFocus::Value(t, 1));
            }
            // `↓` into the values (narrowed by what is typed), when there are some.
            (DialogFocus::Value(..), KeyCode::Down) if d.listed > 0 => {
                d.values_scroll = 0;
                return d.focus_on(DialogFocus::Values(0));
            }
            (DialogFocus::Value(..), KeyCode::Up | KeyCode::Down) => return self.filter_dialog_pane_key(key),
            (DialogFocus::SqlWhere, KeyCode::Down) => return self.filter_dialog_sql_move(DialogFocus::SqlOrder),
            (DialogFocus::SqlOrder, KeyCode::Up) => return self.filter_dialog_sql_move(DialogFocus::SqlWhere),
            (DialogFocus::SqlOrder, KeyCode::Down) => return self.filter_dialog_sql_move(DialogFocus::SqlOpen),
            (DialogFocus::SqlWhere, KeyCode::Up) => return,
            _ => {}
        }
        let Some(input) = d.input_mut() else { return };
        let r = match key.code {
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => input.delete_word_back(),
            _ => input.handle_key(&key),
        };
        if matches!(focus, DialogFocus::SqlWhere | DialogFocus::SqlOrder) {
            // What is typed in the SQL section waits there until it is read back.
            let typed = if let KeyCode::Char(c) = key.code { Some(c) } else { None };
            match r {
                InputResult::Changed => self.filter_dialog_sql_typed(true, typed),
                InputResult::Moved => self.filter_dialog_sql_typed(false, None),
                InputResult::Ignored => {}
            }
            if r == InputResult::Changed
                && let Some(d) = self.filter_dialog_mut()
            {
                d.message = None;
                d.mark = None;
            }
            return;
        }
        if r == InputResult::Changed {
            self.filter_dialog_typed();
        }
    }

    /// The field with the keys changed: the column search narrows the list, a value field
    /// gives its condition its values, the `LIMIT` field the draft its cap.
    fn filter_dialog_typed(&mut self) {
        let Some(d) = self.filter_dialog_mut() else { return };
        match d.focus {
            DialogFocus::Columns => {
                d.cursor = 0;
                d.scroll = 0;
            }
            DialogFocus::Limit => {
                let text = d.limit.text().to_string();
                self.filter_dialog_edit(Some(FieldKey::Limit), |d, _| d.draft.limit = text);
            }
            DialogFocus::Value(t, k) => {
                let texts: Vec<String> = d.inputs.iter().map(|i| i.text().to_string()).collect();
                let Some(row) = d.draft.where_.terms().get(t).and_then(|t| t.row.clone()) else { return };
                let c = d.column_info(&row.col);
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                let values = match row.op.arity() {
                    // Values written as the field showed them stay what they were.
                    Arity::Many => {
                        let kept = draft::list_kept(&row.values, d.dialect, &c);
                        draft::parse_list(refs.first().copied().unwrap_or(""), &c, &kept)
                    }
                    _ => draft::parse_input(row.op, &refs, &c),
                };
                // What is typed narrows the column's value list.
                d.narrow = list::fragment(row.op, refs.first().copied().unwrap_or("")).map(|f| (t, f));
                self.filter_dialog_edit(Some(FieldKey::Value(t, k)), |d, target| {
                    d.draft.where_.set(t, Row { values, ..row }, target);
                });
            }
            // Pasted into the SQL section: it waits there until it is read back.
            DialogFocus::SqlWhere | DialogFocus::SqlOrder => {
                d.message = None;
                d.mark = None;
                self.filter_dialog_popup_close();
            }
            _ => {}
        }
    }

    /// Column `i` of the list selected, whatever selects it (a key, a click, the wheel): a
    /// field of the SQL section with the keys is read back first (an order refused keeps it
    /// and nothing else happens), the column list takes the keys, the value fields and the
    /// narrowing of the column left go, and the focus is kept where the draft has it.
    pub(super) fn filter_dialog_select_column(&mut self, i: usize) {
        if self.filter_dialog().is_some_and(|d| d.sql_field().is_some()) {
            self.filter_dialog_popup_close();
            if !self.filter_dialog_read_back() {
                return;
            }
        }
        let Some(d) = self.filter_dialog_mut() else { return };
        d.focus_on(DialogFocus::Columns);
        let last = d.shown_columns().len().saturating_sub(1);
        if d.cursor != i.min(last) {
            d.cursor = i.min(last);
            d.inputs.clear();
            d.narrow = None;
            d.values_scroll = 0;
        }
        d.settle();
    }

    /// The column list's keys.
    fn filter_dialog_columns_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let n = d.shown_columns().len();
        let last = n.saturating_sub(1);
        let at = d.cursor;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.filter_dialog_select_column(at.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.filter_dialog_select_column((at + 1).min(last)),
            KeyCode::Home | KeyCode::Char('g') => self.filter_dialog_select_column(0),
            KeyCode::End | KeyCode::Char('G') => self.filter_dialog_select_column(last),
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(f) = d.pane_lines().first().copied() {
                    d.focus_on(f);
                }
            }
            KeyCode::Char('/') => d.search = Some(TextInput::new("")),
            KeyCode::Char('s') => {
                let Some(col) = d.column().map(|c| c.name.clone()) else { return };
                let next = match d.draft.sort_of(&col) {
                    None => Some(Dir::Asc),
                    Some((Dir::Asc, _)) => Some(Dir::Desc),
                    Some((Dir::Desc, _)) => None,
                };
                self.filter_dialog_edit(None, |d, _| d.draft.set_sort(&col, next));
            }
            _ => {}
        }
    }

    /// `↑`/`↓` in the right pane: the line above or below (a condition's operator, `+ add
    /// condition`, the sort line); past the top, the column list.
    fn filter_dialog_pane_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let lines = d.pane_lines();
        let here = match d.focus {
            DialogFocus::Value(t, _) => DialogFocus::Op(t),
            DialogFocus::Values(_) => DialogFocus::Values(0),
            f => f,
        };
        let Some(at) = lines.iter().position(|l| *l == here) else { return };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if at == 0 => d.focus_on(DialogFocus::Columns),
            KeyCode::Up | KeyCode::Char('k') => d.focus_on(lines[at - 1]),
            KeyCode::Down | KeyCode::Char('j') if at + 1 < lines.len() => d.focus_on(lines[at + 1]),
            KeyCode::Left | KeyCode::Char('h') => d.focus_on(DialogFocus::Columns),
            _ => {}
        }
    }

    /// The keys on a condition's operator.
    fn filter_dialog_op_key(&mut self, t: usize, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        match key.code {
            KeyCode::Char(' ') => {
                let tab = d.tab;
                let Some(row) = d.draft.where_.terms().get(t).and_then(|t| t.row.clone()) else { return };
                let c = d.column_info(&row.col);
                let Some(target) = self.table_target(tab) else { return };
                let at = draft::ops_for(&c, &target).iter().position(|(op, _)| *op == row.op).unwrap_or(0);
                if let Some(d) = self.filter_dialog_mut() {
                    d.picker = Some(at);
                }
            }
            KeyCode::Right | KeyCode::Char('l') if !d.inputs.is_empty() => d.focus_on(DialogFocus::Value(t, 0)),
            KeyCode::Char('x') | KeyCode::Delete => {
                self.filter_dialog_edit(None, |d, _| {
                    d.draft.where_.remove(t);
                });
            }
            _ => self.filter_dialog_pane_key(key),
        }
    }

    /// The keys of the open operator picker (`Enter` picks through the apply action, `Esc`
    /// closes through the cancel action).
    fn filter_dialog_picker_key(&mut self, key: KeyEvent) {
        let Some(n) = self.filter_dialog_ops().map(|o| o.len()) else { return };
        let Some(d) = self.filter_dialog_mut() else { return };
        let Some(at) = d.picker else { return };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => d.picker = Some(at.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => d.picker = Some((at + 1).min(n.saturating_sub(1))),
            KeyCode::Home | KeyCode::Char('g') => d.picker = Some(0),
            KeyCode::End | KeyCode::Char('G') => d.picker = Some(n.saturating_sub(1)),
            KeyCode::Char(' ') => self.filter_dialog_pick(),
            _ => {}
        }
    }

    /// The operators the picker offers for the focused condition's column, each with why it is
    /// dimmed.
    pub(crate) fn filter_dialog_ops(&self) -> Option<Vec<(Op, Result<(), draft::OpOff>)>> {
        let d = self.filter_dialog()?;
        let (DialogFocus::Op(t) | DialogFocus::Value(t, _)) = d.focus else { return None };
        let row = d.draft.where_.terms().get(t)?.row.as_ref()?;
        let target = self.table_target(d.tab)?;
        Some(draft::ops_for(&d.column_info(&row.col), &target))
    }

    /// The picker's selection becomes the focused condition's operator (a dimmed one is not
    /// taken: why is said); its values kept where they still fit.
    pub(super) fn filter_dialog_pick(&mut self) {
        let Some(ops) = self.filter_dialog_ops() else { return };
        let Some(d) = self.filter_dialog_mut() else { return };
        let (Some(at), DialogFocus::Op(t) | DialogFocus::Value(t, _)) = (d.picker, d.focus) else { return };
        let Some(&(op, allowed)) = ops.get(at) else { return };
        if let Err(off) = allowed {
            d.message = Some(Notice::new(op_off_label(off), Level::Warning));
            return;
        }
        d.picker = None;
        let Some(row) = d.draft.where_.terms().get(t).and_then(|t| t.row.clone()) else { return };
        let c = d.column_info(&row.col);
        let values = values_for(&row, op, &c, d.dialect);
        self.filter_dialog_edit(None, |d, target| {
            d.draft.where_.set(t, Row { op, values, keep_nulls: false, ..row }, target);
        });
        if let Some(d) = self.filter_dialog_mut() {
            d.fill_inputs(t);
            d.focus = if d.inputs.is_empty() { DialogFocus::Op(t) } else { DialogFocus::Value(t, 0) };
        }
    }

    /// `+ add condition`: a condition of the selected column with the first operator it takes,
    /// waiting for its value (never sent before it has one).
    fn filter_dialog_add(&mut self) {
        let Some(d) = self.filter_dialog() else { return };
        let Some(c) = d.column().cloned() else { return };
        let Some(target) = self.table_target(d.tab) else { return };
        let ops = draft::ops_for(&c, &target);
        let op = ops
            .iter()
            .find(|(op, ok)| ok.is_ok() && op.arity() != Arity::Zero)
            .or_else(|| ops.iter().find(|(_, ok)| ok.is_ok()))
            .map_or(Op::IsNull, |(op, _)| *op);
        let mut added = None;
        self.filter_dialog_edit(None, |d, target| {
            added = Some(d.draft.where_.add(Row::new(c.name.clone(), op), target))
        });
        let (Some(t), Some(d)) = (added, self.filter_dialog_mut()) else { return };
        d.focus_on(DialogFocus::Op(t));
        if !d.inputs.is_empty() {
            d.focus_on(DialogFocus::Value(t, 0));
        }
    }

    /// The keys on value `i` of the list: `↑`/`↓` through the values (past the first, back to
    /// the value field that narrows the list, else the line above), `Space` ticks.
    fn filter_dialog_values_key(&mut self, i: usize, key: KeyEvent) {
        let Some(d) = self.filter_dialog() else { return };
        let (last, narrow) = (d.listed.saturating_sub(1), d.narrow.as_ref().map(|(t, _)| *t));
        let up = matches!(key.code, KeyCode::Up | KeyCode::Char('k'));
        let down = matches!(key.code, KeyCode::Down | KeyCode::Char('j'));
        let to = match key.code {
            _ if up && i == 0 => match narrow {
                Some(t) => DialogFocus::Value(t, 0),
                None => return self.filter_dialog_pane_key(key),
            },
            _ if up => DialogFocus::Values(i - 1),
            _ if down && i >= last => DialogFocus::Sort,
            _ if down => DialogFocus::Values(i + 1),
            KeyCode::Home | KeyCode::Char('g') => DialogFocus::Values(0),
            KeyCode::End | KeyCode::Char('G') => DialogFocus::Values(last),
            KeyCode::Char(' ') => return self.filter_dialog_tick(i),
            _ => return self.filter_dialog_pane_key(key),
        };
        if let Some(d) = self.filter_dialog_mut() {
            d.focus_on(to);
        }
    }

    /// The sort line's keys: `←`/`→` none, up, down; `<`/`>` the key's place in the order.
    fn filter_dialog_sort_key(&mut self, key: KeyEvent) {
        let Some(d) = self.filter_dialog() else { return };
        let Some(col) = d.column().map(|c| c.name.clone()) else { return };
        let now = d.draft.sort_of(&col).map(|(dir, _)| dir);
        let step = |right: bool| match (now, right) {
            (None, true) => Some(Dir::Asc),
            (Some(Dir::Asc), true) | (Some(Dir::Desc), true) => Some(Dir::Desc),
            (Some(Dir::Desc), false) => Some(Dir::Asc),
            (Some(Dir::Asc), false) | (None, false) => None,
        };
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                self.filter_dialog_edit(None, |d, _| d.draft.set_sort(&col, step(false)))
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.filter_dialog_edit(None, |d, _| d.draft.set_sort(&col, step(true)))
            }
            KeyCode::Char('<') => self.filter_dialog_edit(None, |d, _| {
                d.draft.move_key(&col, -1);
            }),
            KeyCode::Char('>') => self.filter_dialog_edit(None, |d, _| {
                d.draft.move_key(&col, 1);
            }),
            _ => self.filter_dialog_pane_key(key),
        }
    }

    /// The `Where` row's keys: `←`/`→` between the chips, `x` takes one out, `Space` goes to
    /// its column's condition.
    fn filter_dialog_chips_key(&mut self, i: usize, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let chips: Vec<usize> = d.draft.where_.chips().iter().map(|c| c.index).collect();
        let Some(&t) = chips.get(i) else { return };
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => d.focus_on(DialogFocus::Chips(i.saturating_sub(1))),
            KeyCode::Right | KeyCode::Char('l') => d.focus_on(DialogFocus::Chips((i + 1).min(chips.len() - 1))),
            KeyCode::Char('x') | KeyCode::Delete => self.filter_dialog_edit(None, |d, _| {
                d.draft.where_.remove(t);
            }),
            KeyCode::Char(' ') => {
                let Some(col) = d.draft.where_.terms()[t].row.as_ref().map(|r| r.col.clone()) else {
                    d.message = Some(Notice::new(Label::TableDialogExprNoColumn, Level::Info));
                    return;
                };
                d.select_column(&col);
                d.focus_on(DialogFocus::Op(t));
            }
            _ => {}
        }
    }

    /// The `Order` row's keys: `←`/`→` between the keys, `<`/`>` move one, `x` takes it out,
    /// `Space` goes to its column's sort line.
    fn filter_dialog_order_key(&mut self, i: usize, key: KeyEvent) {
        let Some(d) = self.filter_dialog_mut() else { return };
        let Some(col) = d.draft.order.get(i).map(|k| k.col.clone()) else { return };
        let n = d.draft.order.len();
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => d.focus_on(DialogFocus::Order(i.saturating_sub(1))),
            KeyCode::Right | KeyCode::Char('l') => d.focus_on(DialogFocus::Order((i + 1).min(n - 1))),
            KeyCode::Char('<') | KeyCode::Char('>') => {
                let delta = if key.code == KeyCode::Char('<') { -1 } else { 1 };
                self.filter_dialog_edit(None, |d, _| {
                    if d.draft.move_key(&col, delta) {
                        d.focus = DialogFocus::Order(i.saturating_add_signed(delta));
                    }
                });
            }
            KeyCode::Char('x') | KeyCode::Delete => self.filter_dialog_edit(None, |d, _| d.draft.set_sort(&col, None)),
            KeyCode::Char(' ') => {
                if d.columns.iter().any(|c| c.name == col) {
                    d.select_column(&col);
                    d.focus_on(DialogFocus::Sort);
                }
            }
            _ => {}
        }
    }
}

/// Why an operator is dimmed in the picker.
pub(crate) fn op_off_label(off: draft::OpOff) -> Label {
    match off {
        draft::OpOff::NoEquality => Label::TableDialogOpOffNoEquality,
        draft::OpOff::NoOrder => Label::TableDialogOpOffNoOrder,
        draft::OpOff::NotText => Label::TableDialogOpOffNotText,
        draft::OpOff::TypeUnknown => Label::TableDialogOpOffTypeUnknown,
        draft::OpOff::CaseInsensitiveRefused => Label::TableDialogOpOffCaseInsensitive,
        draft::OpOff::CollationUnknown => Label::TableDialogOpOffCollationUnknown,
        draft::OpOff::LikeRefused => Label::TableDialogOpOffLikeRefused,
        draft::OpOff::LikeUnknown => Label::TableDialogOpOffLikeUnknown,
    }
}
