//! A column's popup from a table tab's grid: the filter dialog's condition popup on the
//! column, under its header. A click on a header's name or type opens it (a drag from there
//! selects whole columns, and Shift extends the selection, as on any grid); `F`, a right click
//! on a header and the grid menu's entry open it with lines above it ([`ColumnLines`]): hiding
//! the column, and on a cell only its value or everything but it (`=` and `!`), each dimmed
//! with why when it cannot apply. The popup's own lines hold the rest: the column's
//! conditions (each with its `×`), its values and its sort, `⋯ Advanced` the whole dialog.
//!
//! A value line runs the filter text written when the popup opened, as `=` and `!` run it
//! ([`App::grid_filter_run`]: the checks and questions of every run): only while the grid
//! (its cell and selection, the result its cells were read from, the columns it shows) and the
//! text those cells give now are what they were. Anything else writes the lines again from the
//! grid as it is, says so, and sends nothing.

use super::filter_dialog::view::{Anchor, PopupKind};
use super::filter_dialog::{ColumnLine, ColumnLines, DialogFocus, ValueLines};
use super::filter_value::Pick;
use super::menu::MenuTarget;
use super::tabs::ResultView;
use super::*;
use crate::widgets::grid::Shape;

/// A press on a table tab's header (its name or type row, not its sort zone), with what the
/// grid showed then: its release over the same header of the same grid opens the column's
/// popup, a drag from it selects whole columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeaderPress {
    tab: TabId,
    binding: u64,
    query: u64,
    shown: Option<usize>,
    view: ResultView,
    result: u64,
    /// The column, by its place in the result.
    pub(crate) col: usize,
    vis_rev: u64,
    x_off: usize,
}

impl App {
    /// What a press at (x, y) on the active table tab's header would arm: the column there
    /// and the grid it is shown in. None off a header, on its sort zone, or on a tab that is
    /// not a table's.
    pub(super) fn header_press_at(&self, x: u16, y: u16) -> Option<HeaderPress> {
        if self.tabs.is_empty() || self.tab().grid.sort_zone_col(x, y).is_some() {
            return None;
        }
        self.header_press_on(self.tab().grid.header_col(x, y)?)
    }

    /// What a press on column `col`'s header of the active table tab's grid would arm now.
    fn header_press_on(&self, col: usize) -> Option<HeaderPress> {
        if self.tabs.is_empty() || !self.tab().is_table() {
            return None;
        }
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return None };
        if t.exec.view != ResultView::Rows || col >= rs.columns.len() || !t.grid.shown(col) {
            return None;
        }
        Some(HeaderPress {
            tab: t.id,
            binding: t.binding,
            query: t.exec.query_id,
            shown: t.exec.shown,
            view: t.exec.view,
            result: rs.id,
            col,
            vis_rev: t.grid.vis_rev,
            x_off: t.grid.x_off,
        })
    }

    /// The left button released at (x, y) after press `p` on a header with no drag between:
    /// over the same header of the same grid, the column's condition popup opens under it (the
    /// cursor goes to the column, the selection drops). Anything else does nothing.
    pub(super) fn header_release(&mut self, p: HeaderPress, x: u16, y: u16) {
        if self.modal_open() || self.header_press_at(x, y) != Some(p) {
            return;
        }
        let g = &mut self.tab_mut().grid;
        g.go_to(g.row, p.col);
        g.anchor = None;
        g.detached = false;
        self.open_column_popup(None);
    }

    /// The pointer moved with the left button held after press `p` on a header: the drag
    /// selects whole columns from that column, as one from a header always did, while the grid
    /// is the one pressed; else the press is over.
    pub(super) fn header_drag(&mut self, p: HeaderPress) -> bool {
        self.pointer.drag = None;
        if self.modal_open() || self.header_press_on(p.col) != Some(p) {
            return false;
        }
        let g = &mut self.tab_mut().grid;
        let to = (g.row, p.col);
        g.select_whole(Shape::Cols, to, false);
        self.pointer.drag = Some(Drag::Grid { anchor: to, shape: Shape::Cols });
        true
    }

    /// After every input and every frame: a press on a header whose grid is no longer the one
    /// pressed (another view, result, scroll or columns shown) is over, even if what changed
    /// comes back before the release.
    pub(crate) fn header_press_settle(&mut self) {
        if let Some(Drag::Header(p)) = self.pointer.drag
            && self.header_press_on(p.col) != Some(p)
        {
            self.pointer.drag = None;
        }
    }

    /// A press on a header that has not moved is over: its release was lost (the pointer moved
    /// with no button held), or a dialog came over it.
    pub(super) fn header_press_drop(&mut self) {
        if matches!(self.pointer.drag, Some(Drag::Header(_))) {
            self.pointer.drag = None;
        }
    }

    /// Where a column's popup opens: under the cursor's column's header as drawn last, else
    /// by the cursor's cell.
    fn header_anchor(&self) -> Anchor {
        let at = self.header_cell(self.tab().id, self.tab().grid.col).unwrap_or_else(|| {
            let (x, y) = self.menu_at_cell();
            Rect::new(x, y, 1, 1)
        });
        Anchor { seg: None, at }
    }

    /// Where column `col`'s header (both its rows) was drawn last in table tab `tab`'s grid,
    /// while that tab is the active one and the column is on screen.
    fn header_cell(&self, tab: TabId, col: usize) -> Option<Rect> {
        if self.tabs.is_empty() || self.tab().id != tab {
            return None;
        }
        let g = &self.tab().grid;
        let &(a, b, _) = g.hit_cols.iter().find(|c| c.2 == col)?;
        Some(Rect::new(a, g.header_y, b.saturating_sub(a).max(1), g.data_y.saturating_sub(g.header_y).max(1)))
    }

    /// Where the open filter dialog's popup on column `name` of table tab `tab` is drawn from
    /// when it opened by the column's header: that header where it is drawn now.
    pub(crate) fn popup_header(&self, tab: TabId, name: &str) -> Option<Rect> {
        let Results::Rows(rs) = &self.tabs.get(tab)?.results else { return None };
        self.header_cell(tab, rs.columns.iter().position(|c| c.meta.name == name)?)
    }

    /// `F`, the grid menu's entry: the popup of the cursor's column with its lines above it,
    /// the filters by the cursor's value among them.
    pub(super) fn open_column_menu(&mut self) {
        self.open_column_popup(Some(true));
    }

    /// The condition popup of the active table tab's cursor column, under its header: with
    /// `lines`, the lines above it (`Some(true)`: the filters by the cursor's value too). Why
    /// not, said, when the tab shows no column or the table's columns as read do not list it.
    pub(super) fn open_column_popup(&mut self, lines: Option<bool>) {
        self.close_search_prompt();
        // The rows first: the popup is about the grid's column.
        if self.chart_shown() || self.plan_shown() || self.changes_shown() {
            self.tab_mut().exec.view = ResultView::Rows;
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
        let anchor = self.header_anchor();
        self.open_filter_popup(PopupKind::Condition { col: column.clone() }, anchor, None);
        let id = self.tab().id;
        let Some(d) = self.filter_dialog().filter(|d| d.tab == id && d.is_popup()) else { return };
        // The dialog's columns are the table's as read (its catalogue): one of the rows shown
        // they do not list (added since) would leave the popup on another column.
        if d.column().is_none_or(|c| c.name != column) {
            self.filter_dialog_dismiss();
            let column = crate::text::escape_controls(&column);
            return self.flash(Notice::new(Msg::TableColumnNotListed { column }, Level::Warning));
        }
        let Some(cells) = lines else { return };
        let values = if cells { self.value_lines() } else { None };
        let Some(d) = self.filter_dialog_mut().filter(|d| d.tab == id && d.is_popup()) else { return };
        if !matches!(d.popup_kind(), Some(PopupKind::Condition { col }) if *col == column) {
            return;
        }
        d.column_lines = Some(ColumnLines { col: column, values });
    }

    /// The filters by the value of the grid's cursor cell (or the cells selected), for the grid
    /// as it is now.
    fn value_lines(&self) -> Option<ValueLines> {
        let target = MenuTarget::Filter { grid: Box::new(self.menu_grid_target()?), result: self.grid_result_id()? };
        let id = self.tab().id;
        Some(ValueLines {
            target,
            vis_rev: self.tab().grid.vis_rev,
            only: self.grid_pick(id, Pick::Only),
            exclude: self.grid_pick(id, Pick::Exclude),
        })
    }

    /// The label of line `l` of the open popup's lines: a value line names its value (or what
    /// it would do, when it cannot).
    fn column_line_label(&self, lines: &ColumnLines, l: ColumnLine) -> String {
        let label = |l: Label| self.i18n.label(l).to_string();
        let v = lines.values.as_ref();
        match l {
            ColumnLine::Only => match v.map(|v| &v.only) {
                Some(Ok(p)) => self.i18n.msg(&Msg::TableColumnOnly { value: p.shown.clone() }).to_string(),
                _ => label(Label::TableColumnOnlyCell),
            },
            ColumnLine::Exclude => match v.map(|v| &v.exclude) {
                Some(Ok(p)) => self.i18n.msg(&Msg::TableColumnExclude { value: p.shown.clone() }).to_string(),
                _ => label(Label::TableColumnExcludeCell),
            },
            ColumnLine::Hide => label(Label::TableColumnHide),
        }
    }

    /// The lines above the open column popup, top to bottom: each with its label, the key that
    /// runs it or (dimmed) why it cannot, and whether it is dimmed.
    pub(crate) fn column_lines_shown(&self) -> Vec<(ColumnLine, String, String, bool)> {
        let Some(c) = self.filter_dialog().and_then(|d| d.column_lines.as_ref()) else { return Vec::new() };
        c.lines()
            .into_iter()
            .map(|l| {
                let label = crate::text::escape_controls(&self.column_line_label(c, l));
                match c.off(l) {
                    Some(o) => (l, label, o.short.map(|s| self.i18n.label(s).to_string()).unwrap_or_default(), true),
                    None if l == ColumnLine::Hide => (l, label, "C".to_string(), false),
                    None => (l, label, String::new(), false),
                }
            })
            .collect()
    }

    /// [`App::column_lines_shown`] as text: label, key or why, dimmed.
    pub fn column_popup_lines(&self) -> Vec<(String, String, bool)> {
        self.column_lines_shown().into_iter().map(|(_, l, k, dim)| (l, k, dim)).collect()
    }

    /// The open column popup's line the keys are on, by its label.
    pub fn column_popup_focus(&self) -> Option<String> {
        let DialogFocus::Line(l) = self.filter_dialog()?.focus else { return None };
        self.column_lines_shown().into_iter().find(|x| x.0 == l).map(|x| x.1)
    }

    /// Why the open column popup's line `label` cannot run, in full, when it is dimmed.
    pub fn column_popup_line_notice(&self, label: &str) -> Option<Notice> {
        let c = self.filter_dialog()?.column_lines.as_ref()?;
        let l = self.column_lines_shown().into_iter().find(|x| x.1 == label)?.0;
        c.off(l).map(|o| o.notice.clone())
    }

    /// Line `l` of the open popup's lines pressed (`Enter`, `Space`, a click; `C` for hiding):
    /// one that cannot apply says why; a value line runs the text it showed while what it was
    /// written from is the same (else the lines are written again, nothing sent); hiding hides
    /// the popup's column while the rows shown still have it. Either closes the popup first,
    /// its draft dropped as `Esc` drops it.
    pub(super) fn column_line_run(&mut self, l: ColumnLine) {
        let Some(d) = self.filter_dialog() else { return };
        let Some(lines) = d.column_lines.clone() else { return };
        if d.tab != self.tab().id {
            return;
        }
        // Said on the status line, as the grid's keys say it: the hint line is too short.
        if let Some(o) = lines.off(l) {
            let n = o.notice.clone();
            return self.flash(n);
        }
        match l {
            ColumnLine::Only | ColumnLine::Exclude => {
                let Some(v) = lines.values.as_ref() else { return };
                let (pick, frozen) = match l {
                    ColumnLine::Only => (Pick::Only, &v.only),
                    _ => (Pick::Exclude, &v.exclude),
                };
                let id = self.tab().id;
                let same = self.menu_target_now(&v.target).as_ref() == Some(&v.target)
                    && self.tab().grid.vis_rev == v.vis_rev
                    && self.grid_pick(id, pick).ok() == frozen.as_ref().ok().cloned();
                if !same {
                    // Written again from the grid as it is; with no cell to write them from,
                    // dimmed.
                    let gone = || {
                        let n = Notice::new(Label::TableColumnLineStale, Level::Warning);
                        Err(Box::new(super::menu::Off::new(Label::TableColumnOffStale, n)))
                    };
                    let values = self.value_lines().unwrap_or_else(|| ValueLines {
                        target: v.target.clone(),
                        vis_rev: v.vis_rev,
                        only: gone(),
                        exclude: gone(),
                    });
                    if let Some(d) = self.filter_dialog_mut()
                        && let Some(c) = d.column_lines.as_mut()
                    {
                        c.values = Some(values);
                        d.settle();
                    }
                    return self.flash(Notice::new(Label::TableColumnLineStale, Level::Warning));
                }
                let Ok(p) = frozen.clone() else { return };
                self.filter_dialog_dismiss();
                self.grid_filter_run(p.text);
            }
            ColumnLine::Hide => {
                let shown = match &self.tab().results {
                    Results::Rows(rs) => rs.columns.iter().position(|c| c.meta.name == lines.col),
                    _ => None,
                };
                if !shown.is_some_and(|c| self.tab().grid.shown(c)) {
                    return self.flash(Notice::new(Label::TableColumnHideGone, Level::Warning));
                }
                self.filter_dialog_dismiss();
                self.table_hide(&lines.col);
            }
        }
    }

    /// `C` in a column's popup with its lines: the line that hides the column, focused, runs.
    pub(super) fn column_line_key(&mut self, l: ColumnLine) -> bool {
        let Some(d) = self.filter_dialog_mut() else { return false };
        if !d.column_lines.as_ref().is_some_and(|c| c.lines().contains(&l)) {
            return false;
        }
        d.focus_on(DialogFocus::Line(l));
        self.column_line_run(l);
        true
    }
}
