//! The column menu of a table tab's grid (`F`, a right click on a column's header): the
//! cursor's column's sort (ascending, descending, none), its filter (the dialog on that column,
//! only the cursor's value, everything but it, none of its own conditions) and hiding it. Each
//! entry that cannot apply is shown dimmed with why; an entry that changes the query runs it as
//! the tab runs any query ([`App::table_run`]: checks, questions, pending edits), written when
//! the menu opens (what runs is what the menu showed).

use super::filter_dialog::draft::Where;
use super::filter_value::Pick;
use super::menu::{MenuItem, MenuTarget, Off};
use super::table_filter::TableAction;
use super::table_query::{FIELD_CAP, filter_text};
use super::*;
use datarig_core::query::{Dir, KeyOrigin, OrderKey};

/// An entry of the column menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnItem {
    /// The column alone, ascending.
    SortAsc,
    /// The column alone, descending.
    SortDesc,
    /// The column out of the order (the other keys stay).
    SortClear,
    /// The filter and sort dialog on the column.
    Dialog,
    /// Only the cursor's value (`=`).
    Only,
    /// Everything but the cursor's value (`!`).
    Exclude,
    /// None of the column's own conditions (the other terms stay as written).
    ClearFilter,
    /// The column hidden from the grid (`C`).
    Hide,
}

impl ColumnItem {
    /// The group of entries it is shown in: a rule between groups.
    pub fn group(self) -> u8 {
        match self {
            ColumnItem::SortAsc | ColumnItem::SortDesc | ColumnItem::SortClear => 0,
            ColumnItem::Dialog | ColumnItem::Only | ColumnItem::Exclude | ColumnItem::ClearFilter => 1,
            ColumnItem::Hide => 2,
        }
    }
}

impl App {
    /// `F`: the menu of the active table tab's cursor column, next to the cursor's cell; why
    /// not, said, when the tab shows no column.
    pub(super) fn open_column_menu(&mut self) {
        let at = self.menu_at_cell();
        self.open_column_menu_at(at);
    }

    /// The column menu of the active table tab's cursor column, opened at `at`.
    pub(super) fn open_column_menu_at(&mut self, at: (u16, u16)) {
        let id = self.tab().id;
        self.close_search_prompt();
        // The rows first: the menu is about the grid's column.
        if self.chart_shown() || self.plan_shown() || self.changes_shown() {
            self.tab_mut().exec.view = super::tabs::ResultView::Rows;
        }
        if self.focus != Focus::Inspector {
            self.focus = Focus::Results;
        }
        let column = match &self.tab().results {
            Results::Rows(rs) => rs.columns.get(self.tab().grid.col).map(|c| c.meta.name.clone()),
            _ => None,
        };
        let Some(column) = column else {
            return self.flash(Notice::new(Label::TableSortNoColumns, Level::Info));
        };
        let (Some(grid), Some(result)) = (self.menu_grid_target(), self.grid_result_id()) else { return };
        let label = |l: Label| self.i18n.label(l);
        let mut items = Vec::new();
        let mut off: Vec<(MenuItem, Off)> = Vec::new();
        let mut names = Vec::new();
        let mut filters = Vec::new();
        let info = |short: Label, l: Label| Off::new(short, Notice::new(l, Level::Info));
        // The order of the rows shown, by the user's keys.
        let shown = self.table_query(id).and_then(|q| q.shown.clone());
        let user: Vec<OrderKey> = shown
            .as_ref()
            .map(|q| q.order.iter().filter(|k| k.origin == KeyOrigin::User).cloned().collect())
            .unwrap_or_default();
        let alone = |dir: Dir| user.len() == 1 && user[0].col == column && user[0].dir == dir;
        let sorted = user.iter().any(|k| k.col == column);
        for (c, glyph, l) in [
            (ColumnItem::SortAsc, "↑", Label::TableColumnSortAsc),
            (ColumnItem::SortDesc, "↓", Label::TableColumnSortDesc),
            (ColumnItem::SortClear, "", Label::TableColumnSortClear),
        ] {
            let item = MenuItem::Column(c);
            items.push(item);
            let name = if glyph.is_empty() { label(l).to_string() } else { format!("{glyph} {}", label(l)) };
            names.push((item, Localized::verbatim(name)));
            let why = match c {
                ColumnItem::SortAsc if alone(Dir::Asc) => {
                    Some(info(Label::TableColumnOffSorted, Label::TableColumnSorted))
                }
                ColumnItem::SortDesc if alone(Dir::Desc) => {
                    Some(info(Label::TableColumnOffSorted, Label::TableColumnSorted))
                }
                ColumnItem::SortClear if !sorted => {
                    Some(info(Label::TableColumnOffNotSorted, Label::TableColumnNotSorted))
                }
                _ => None,
            };
            if let Some(o) = why {
                off.push((item, o));
            }
        }
        let item = MenuItem::Column(ColumnItem::Dialog);
        items.push(item);
        names.push((item, label(Label::TableColumnDialog)));
        for (pick, c, msg, plain) in [
            (Pick::Only, ColumnItem::Only, true, Label::TableColumnOnlyCell),
            (Pick::Exclude, ColumnItem::Exclude, false, Label::TableColumnExcludeCell),
        ] {
            let item = MenuItem::Column(c);
            items.push(item);
            let name = match self.grid_pick(id, pick) {
                Ok(p) => {
                    filters.push((item, p.text));
                    match msg {
                        true => self.i18n.msg(&Msg::TableColumnOnly { value: p.shown }),
                        false => self.i18n.msg(&Msg::TableColumnExclude { value: p.shown }),
                    }
                }
                Err(o) => {
                    off.push((item, *o));
                    label(plain)
                }
            };
            names.push((item, name));
        }
        let item = MenuItem::Column(ColumnItem::ClearFilter);
        items.push(item);
        names.push((item, label(Label::TableColumnClearFilter)));
        match self.column_unfiltered(id, &column) {
            Ok(text) => filters.push((item, text)),
            Err(o) => off.push((item, *o)),
        }
        let item = MenuItem::Column(ColumnItem::Hide);
        items.push(item);
        names.push((item, label(Label::TableColumnHide)));
        let title = crate::text::escape_controls(&column);
        let target = MenuTarget::Filter { grid: Box::new(grid), result };
        self.push_filter_menu(target, title, (items, off, names, Vec::new(), filters), at);
    }

    /// The filter of table tab `id`'s rows shown without column `column`'s own conditions (the
    /// terms the dialog shows as its rows; every other term as written), or why there is none
    /// to take out.
    fn column_unfiltered(&self, id: TabId, column: &str) -> Result<String, Box<Off>> {
        let none = || {
            Box::new(Off::new(
                Label::TableColumnOffNoCondition,
                Notice::new(Label::TableColumnNoCondition, Level::Info),
            ))
        };
        let tq = self.table_query(id).ok_or_else(none)?;
        let (Some(shown), Some(target)) = (tq.shown.as_ref(), self.table_target(id)) else { return Err(none()) };
        if !self.rows_shown_applied(id) {
            let n = Notice::new(Label::TableFilterByValueNotShown, Level::Warning);
            return Err(Box::new(Off::new(Label::TableFilterByValueOffNotShown, n)));
        }
        let mut terms = Where::from_text(filter_text(shown).unwrap_or_default(), &target);
        let own = terms.rows_of(column);
        if own.is_empty() {
            return Err(none());
        }
        for i in own.into_iter().rev() {
            terms.remove(i);
        }
        let text = terms.text();
        match text.len() > FIELD_CAP {
            true => Err(Box::new(Off::new(
                Label::TableFilterByValueOffTooLong,
                Notice::new(Label::TableFilterByValueTooLong, Level::Warning),
            ))),
            false => Ok(text),
        }
    }

    /// Entry `item` of the column menu runs (from [`App::menu_run`]), with the filter `text`
    /// the menu wrote for it when it opened (for the entries that change the filter).
    pub(super) fn column_item(&mut self, item: ColumnItem, text: Option<String>) {
        let id = self.tab().id;
        let col = self.tab().grid.col;
        match item {
            ColumnItem::SortAsc => self.table_sort_with(id, col, |_, c| vec![OrderKey::user(c, Dir::Asc)]),
            ColumnItem::SortDesc => self.table_sort_with(id, col, |_, c| vec![OrderKey::user(c, Dir::Desc)]),
            ColumnItem::SortClear => self.table_sort_with(id, col, |keys, c| {
                keys.iter().filter(|k| k.origin == KeyOrigin::User && k.col != c).cloned().collect()
            }),
            ColumnItem::Dialog => self.table_action(TableAction::EditFilter),
            ColumnItem::Hide => self.table_action(TableAction::Hide),
            ColumnItem::Only | ColumnItem::Exclude | ColumnItem::ClearFilter => {
                if let Some(text) = text {
                    self.grid_filter_run(text);
                }
            }
        }
    }

    /// The keys shown with entry `c` of the column menu (in the grid's context).
    pub(super) fn column_item_keys(&self, c: ColumnItem) -> String {
        let keys = |t: TableAction| {
            self.keymap
                .hint_keys(Action::Table(t), Ctx::Grid, self.enhanced_keys)
                .map(|k| crate::keymap::keys::label(&k))
                .unwrap_or_default()
        };
        match c {
            ColumnItem::SortAsc => keys(TableAction::SortCycle),
            ColumnItem::SortDesc => {
                let k = keys(TableAction::SortCycle);
                if k.is_empty() { k } else { format!("{k} {k}") }
            }
            ColumnItem::Dialog => keys(TableAction::EditFilter),
            ColumnItem::Only => keys(TableAction::Only),
            ColumnItem::Exclude => keys(TableAction::Exclude),
            ColumnItem::Hide => keys(TableAction::Hide),
            ColumnItem::SortClear | ColumnItem::ClearFilter => String::new(),
        }
    }
}
