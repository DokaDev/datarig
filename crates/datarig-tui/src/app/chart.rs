//! The shown row result as a chart, in the results pane's Chart tab.
//!
//! `results.chart` (`c` in the grid) draws the rows the result holds: nothing is asked of the
//! server, nothing runs again. Which columns go where is picked at first sight
//! ([`chart::infer`]) and can be changed: the kind (`v`, the digits), the X column (`x`), the
//! value columns (`s`), a column that splits the values into series (`b`), a logarithmic scale
//! (`S`). The numbers are read again only when the result, its rows or the choice changes;
//! a frame draws from them.
//!
//! The chart follows the shown result: another result tab, or a later run's rows, get their
//! own chart, with the columns chosen before while the new result has them by name.

use super::*;
use crate::widgets::grid::ResultSet;
use datarig_core::chart::{self, Kind, Model, Role, Spec, Unsuitable};
use std::sync::Arc;

/// What the Chart tab acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartAction {
    /// Show the shown result as a chart, or its rows again (from the chart).
    Toggle,
    /// Move the cursor: along the X axis, or between the series (the bars' direction decides
    /// which keys do which).
    Left,
    Right,
    Up,
    Down,
    First,
    Last,
    /// The next (`true`) or previous kind of chart.
    NextKind(bool),
    Kind(Kind),
    PickX,
    PickY,
    PickBy,
    /// A logarithmic value axis, or a linear one again.
    Log,
    /// The chart's numbers as TSV, or the drawing as text, to the clipboard.
    CopyData,
    CopyText,
    /// Show the row the cursor's point comes from in the grid.
    GotoRow,
}

/// A part of the Chart tab's first lines that takes a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartHit {
    Kind(Kind),
    X,
    Y,
    By,
    Log,
}

/// What a chart's numbers were read from: the result, its rows then, and the choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub result: u64,
    pub rows: usize,
    pub spec: Spec,
}

/// What the Chart tab draws.
#[derive(Clone, Debug)]
pub enum Drawn {
    Model(Arc<Model>),
    Unsuitable(Unsuitable),
    /// The rows could not be read (the spill file).
    Unreadable(datarig_core::fault::Fault),
}

/// A line chart drawn in braille dots, kept until its numbers or its size change.
#[derive(Clone, Debug, Default)]
pub struct Raster {
    pub key: (u64, usize, u16, u16, bool),
    /// Per cell: its dots, and the series drawn last in it (+1; 0: none).
    pub cells: Vec<(u8, u8)>,
    /// The column of each point.
    pub cols: Vec<u16>,
}

/// A tab's chart: what it draws and its cursor.
pub struct ChartTab {
    pub spec: Spec,
    /// The result the choice is for (its id), and that result's column names and roles.
    pub result: u64,
    pub names: Vec<String>,
    pub roles: Vec<Role>,
    /// The choice could not be made: why (no rows, no numbers).
    pub unsuitable: Option<Unsuitable>,
    pub built: Option<(Source, Drawn)>,
    /// The point and the series under the cursor.
    pub cursor: usize,
    pub series: usize,
    /// The first bar drawn (bars that do not fit scroll to the cursor).
    pub offset: usize,
    /// The row of the cursor's point as read for the readout: (result, row, cells).
    pub row: Option<(u64, usize, Vec<datarig_core::driver::Cell>)>,
    /// Kept by the renderer for the mouse: where each point's bar or column is, the plot, the
    /// first lines' parts, and the last area drawn (a copy of the drawing has its size).
    pub hits: Vec<(Rect, usize)>,
    pub plot: Rect,
    pub field_hits: Vec<(Rect, ChartHit)>,
    pub area: Rect,
    pub raster: Option<Raster>,
}

impl ChartTab {
    /// A chart of `rs`.
    fn of(rs: &ResultSet) -> ChartTab {
        let names: Vec<String> = rs.columns.iter().map(|c| c.meta.name.clone()).collect();
        let (roles, sample) = sample(rs);
        let (spec, unsuitable) = match chart::infer(&roles, &sample, rs.rows.len()) {
            Ok(s) => (s, None),
            Err(u) => (Spec { kind: Kind::Bar, x: None, ys: Vec::new(), by: None, log: false }, Some(u)),
        };
        ChartTab {
            spec,
            result: rs.id,
            names,
            roles,
            unsuitable,
            built: None,
            cursor: 0,
            series: 0,
            offset: 0,
            row: None,
            hits: Vec::new(),
            plot: Rect::default(),
            field_hits: Vec::new(),
            area: Rect::default(),
            raster: None,
        }
    }

    /// The chart follows `rs`: a result it was not made for gets the columns chosen before by
    /// name (while they fit), else a choice of its own; a choice that could not be made is
    /// made again once rows come.
    fn follow(&mut self, rs: &ResultSet) {
        if self.result == rs.id {
            // No choice could be made (no rows yet, say): made again when rows come.
            let more_rows = self.built.as_ref().is_some_and(|(s, _)| s.rows != rs.rows.len());
            if self.unsuitable.is_some() && more_rows {
                *self = ChartTab::of(rs);
            }
            return;
        }
        let mut next = ChartTab::of(rs);
        if self.unsuitable.is_none()
            && let Some(spec) = chart::carry(&self.spec, &self.names, &next.names, &next.roles)
        {
            next.spec = spec;
            next.unsuitable = None;
        }
        *self = next;
    }

    /// The numbers for `rs` as the choice is now: read again when the result, its rows or the
    /// choice changed.
    fn build(&mut self, rs: &ResultSet) {
        let source = Source { result: rs.id, rows: rs.rows.len(), spec: self.spec.clone() };
        if self.built.as_ref().is_some_and(|(s, _)| *s == source) {
            return;
        }
        let drawn = match self.unsuitable {
            Some(u) => Drawn::Unsuitable(u),
            None => {
                let columns: Vec<datarig_core::driver::ColumnMeta> =
                    rs.columns.iter().map(|c| c.meta.clone()).collect();
                let mut b = chart::Builder::new(&self.spec, &columns, &self.roles);
                let mut at = 0;
                let read = rs.rows.for_each_chunk(0..rs.rows.len(), 4096, |chunk| {
                    for r in chunk {
                        b.push(at, r);
                        at += 1;
                    }
                    crate::widgets::chart::work(chunk.len() as u64);
                });
                match read {
                    Err(e) => Drawn::Unreadable(e),
                    Ok(()) => match b.finish() {
                        Ok(m) => Drawn::Model(Arc::new(m)),
                        Err(u) => Drawn::Unsuitable(u),
                    },
                }
            }
        };
        if let Drawn::Model(m) = &drawn {
            self.cursor = self.cursor.min(m.points.len().saturating_sub(1));
            self.series = self.series.min(m.series.len().saturating_sub(1));
        }
        self.raster = None;
        self.built = Some((source, drawn));
    }

    /// The numbers drawn, when there are some.
    pub fn model(&self) -> Option<&Arc<Model>> {
        match &self.built {
            Some((_, Drawn::Model(m))) => Some(m),
            _ => None,
        }
    }

    /// The row the value of `series` at `point` comes from (its first, when it sums several),
    /// else the point's first row; `None` for "others".
    pub fn row_of(m: &Model, point: usize, series: usize) -> Option<usize> {
        let p = m.points.get(point).filter(|p| !p.others)?;
        m.series.get(series).and_then(|s| s.first.get(point).copied().flatten()).or(p.first_row)
    }

    /// Move the cursor `d` points along the X axis.
    fn step(&mut self, d: isize) {
        let n = self.model().map_or(0, |m| m.points.len());
        if n > 0 {
            self.cursor = (self.cursor as isize + d).clamp(0, n as isize - 1) as usize;
        }
    }

    /// Move the cursor `d` series.
    fn step_series(&mut self, d: isize) {
        let n = self.model().map_or(0, |m| m.series.len());
        if n > 0 {
            self.series = (self.series as isize + d).clamp(0, n as isize - 1) as usize;
        }
    }

    fn act(&mut self, a: ChartAction) {
        // Horizontal bars run down the screen: the vertical keys move along them.
        let along = self.spec.kind != Kind::HBar;
        match a {
            ChartAction::Left if along => self.step(-1),
            ChartAction::Right if along => self.step(1),
            ChartAction::Up if !along => self.step(-1),
            ChartAction::Down if !along => self.step(1),
            ChartAction::Left | ChartAction::Up => self.step_series(-1),
            ChartAction::Right | ChartAction::Down => self.step_series(1),
            ChartAction::First => self.cursor = 0,
            ChartAction::Last => self.step(isize::MAX / 2),
            ChartAction::NextKind(next) => self.spec.kind = self.spec.kind.step(if next { 1 } else { -1 }),
            ChartAction::Kind(k) => self.spec.kind = k,
            ChartAction::Log => self.spec.log = !self.spec.log,
            _ => {}
        }
    }
}

/// The roles of `rs`'s columns, and its first rows they were told from.
fn sample(rs: &ResultSet) -> (Vec<Role>, Vec<Vec<datarig_core::driver::Cell>>) {
    let mut sample = Vec::new();
    let n = rs.rows.len().min(chart::SAMPLE);
    // Rows that cannot be read leave the sample shorter (the chart says so when it reads them).
    let _ = rs.rows.for_each_chunk(0..n, n.max(1), |chunk| sample.extend_from_slice(chunk));
    let columns: Vec<datarig_core::driver::ColumnMeta> = rs.columns.iter().map(|c| c.meta.clone()).collect();
    (chart::roles(&columns, &sample), sample)
}

impl App {
    /// The active tab shows a chart.
    pub fn chart_shown(&self) -> bool {
        !self.tabs.is_empty()
            && self.tab().exec.view == super::tabs::ResultView::Chart
            && self.tab().exec.chart.is_some()
            && matches!(self.tab().results, Results::Rows(_))
            && self.results_shown()
    }

    /// The active tab's chart follows its shown result and has its numbers.
    pub(crate) fn chart_sync(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let t = self.tab_mut();
        let (Results::Rows(rs), Some(c)) = (&t.results, t.exec.chart.as_mut()) else { return };
        c.follow(rs);
        c.build(rs);
        let fault = match &c.built {
            Some((_, Drawn::Unreadable(f))) => Some(f.clone()),
            _ => None,
        };
        // The cursor's row, read once for the readout (it may be in the spill file).
        let want = c.model().and_then(|m| ChartTab::row_of(m, c.cursor, c.series));
        let have = c.row.as_ref().map(|(r, i, _)| (*r, *i));
        match want {
            Some(i) if have != Some((rs.id, i)) => {
                let mut cells = None;
                if rs.rows.for_each_chunk(i..i + 1, 1, |chunk| cells = chunk.first().cloned()).is_ok() {
                    c.row = cells.map(|cells| (rs.id, i, cells));
                } else {
                    c.row = None;
                }
            }
            Some(_) => {}
            None => c.row = None,
        }
        if let Some(f) = fault {
            self.results_read_failed(&f);
        }
    }

    pub(super) fn chart_action(&mut self, a: ChartAction) {
        use super::tabs::ResultView;
        match a {
            ChartAction::Toggle => return self.toggle_chart(),
            ChartAction::CopyData => return self.copy_chart(false),
            ChartAction::CopyText => return self.copy_chart(true),
            ChartAction::PickX | ChartAction::PickY | ChartAction::PickBy => return self.open_chart_picker(a),
            ChartAction::GotoRow => return self.chart_goto_row(),
            _ => {}
        }
        self.chart_sync();
        let t = self.tab_mut();
        let Some(c) = t.exec.chart.as_mut() else { return };
        // Choosing a kind or the scale shows the chart.
        if matches!(a, ChartAction::NextKind(_) | ChartAction::Kind(_) | ChartAction::Log) {
            t.exec.view = ResultView::Chart;
        }
        c.act(a);
    }

    /// `results.chart`: the shown result as a chart, or (on the chart) its rows again.
    fn toggle_chart(&mut self) {
        use super::tabs::ResultView;
        if self.chart_shown() {
            self.tab_mut().exec.view = ResultView::Rows;
            return;
        }
        let t = self.tab_mut();
        let Results::Rows(rs) = &t.results else { return };
        if t.exec.chart.is_none() {
            t.exec.chart = Some(ChartTab::of(rs));
        }
        t.exec.view = ResultView::Chart;
        self.focus = Focus::Results;
        self.chart_sync();
    }

    /// The chart's numbers as TSV, or the drawing as text, to the clipboard.
    fn copy_chart(&mut self, drawing: bool) {
        self.chart_sync();
        let others = self.i18n.label(Label::ChartOthers).to_string();
        let row = self.i18n.label(Label::ChartRowNumber).to_string();
        let t = self.tab();
        let Some(c) = t.exec.chart.as_ref() else { return };
        let Some(m) = c.model().cloned() else {
            return self.flash(Notice::new(Label::ChartNothingToCopy, Level::Warning));
        };
        let text = if drawing {
            crate::widgets::chart::as_text(self)
        } else {
            let x = c.spec.x.and_then(|x| c.names.get(x)).cloned().unwrap_or(row);
            m.tsv(&x, &others)
        };
        let count = text.lines().count() as u64;
        let done = match self.deliver(&text) {
            Ok(method) => {
                let method = self.method_text(method);
                let msg =
                    if drawing { Msg::ChartCopiedText { count, method } } else { Msg::ChartCopied { count, method } };
                Notice::new(msg, Level::Success)
            }
            Err(msg) => Notice::new(msg, Level::Error),
        };
        self.show_status(done.clone());
        self.flash(done);
    }

    /// `Enter` on the chart: the grid, on the row the cursor's point comes from (its first,
    /// when it sums several).
    fn chart_goto_row(&mut self) {
        self.chart_sync();
        let t = self.tab_mut();
        let Some(c) = t.exec.chart.as_ref() else { return };
        let Some(m) = c.model() else { return };
        if c.cursor >= m.points.len() {
            return;
        }
        let Some(row) = ChartTab::row_of(m, c.cursor, c.series) else {
            return self.flash(Notice::new(Label::ChartOthersNoRow, Level::Info));
        };
        let col = c.spec.x.or(c.spec.ys.first().copied()).unwrap_or(0);
        let g = &mut t.grid;
        if g.page_size > 0 {
            g.page = row / g.page_size;
        }
        g.row = row;
        g.top = row;
        g.col = col;
        g.anchor = None;
        g.detached = false;
        t.exec.view = super::tabs::ResultView::Rows;
    }

    /// A list of the result's columns to choose the X axis, the values or the series from.
    fn open_chart_picker(&mut self, a: ChartAction) {
        // A row of the list: the column's index, and its text.
        type Item = (Option<String>, String);
        use super::chooser::{Chooser, ChooserPurpose};
        self.chart_sync();
        let row_number = self.i18n.label(Label::ChartRowNumber).to_string();
        let none = self.i18n.label(Label::ChartNone).to_string();
        let t = self.tab();
        let Some(c) = t.exec.chart.as_ref() else { return };
        let kind = |r: Role| match r {
            Role::Number => "#",
            Role::Time(_) => "\u{25F7}",
            _ => "a",
        };
        let col = |i: usize| (Some(i.to_string()), format!("{} {}", kind(c.roles[i]), c.names[i]));
        let (title, items, current, purpose): (Label, Vec<Item>, Option<String>, _) = match a {
            ChartAction::PickX => {
                let mut items = vec![(None, row_number)];
                items.extend((0..c.names.len()).filter(|&i| c.roles[i].x()).map(col));
                (Label::ChartPickX, items, c.spec.x.map(|i| i.to_string()), ChooserPurpose::ChartX(c.result))
            }
            ChartAction::PickY => {
                let items: Vec<_> = (0..c.names.len())
                    .filter(|&i| c.roles[i].y())
                    .map(|i| {
                        let mark = if c.spec.ys.contains(&i) { "[x]" } else { "[ ]" };
                        (Some(i.to_string()), format!("{mark} {}", c.names[i]))
                    })
                    .collect();
                if items.is_empty() {
                    return self.flash(Notice::new(Label::ChartUnsuitableNoNumber, Level::Info));
                }
                let title = if c.spec.by.is_some() { Label::ChartPickYOne } else { Label::ChartPickY };
                (title, items, c.spec.ys.first().map(|i| i.to_string()), ChooserPurpose::ChartY(c.result))
            }
            _ => {
                let mut items = vec![(None, none)];
                items.extend((0..c.names.len()).filter(|&i| c.roles[i].by() && Some(i) != c.spec.x).map(col));
                (Label::ChartPickBy, items, c.spec.by.map(|i| i.to_string()), ChooserPurpose::ChartBy(c.result))
            }
        };
        let selected = items.iter().position(|(v, _)| *v == current).unwrap_or(0);
        self.overlays.push(Overlay::Chooser(Chooser {
            title,
            items,
            selected,
            filter: TextInput::default(),
            filtering: false,
            scroll: 0,
            list: Default::default(),
            hover: None,
            press: Default::default(),
            purpose,
        }));
    }

    /// A column was picked in a chart's list (made for result `result`): the chart draws it.
    /// Values are added or removed (the list stays open); with a column splitting them into
    /// series, the one value is replaced. Nothing changes when the result is not the one the
    /// list was made for any more.
    pub(super) fn chart_picked(&mut self, purpose: &super::chooser::ChooserPurpose, value: Option<String>) -> bool {
        use super::chooser::ChooserPurpose as P;
        let result = match purpose {
            P::ChartX(r) | P::ChartY(r) | P::ChartBy(r) => *r,
            _ => return false,
        };
        let col = value.and_then(|v| v.parse::<usize>().ok());
        let t = self.tab_mut();
        let current = matches!(&t.results, Results::Rows(rs) if rs.id == result);
        let Some(c) = t.exec.chart.as_mut().filter(|c| current && c.result == result) else {
            self.overlays.close(OverlayKind::Chooser);
            self.flash(Notice::new(Label::ChartPickStale, Level::Warning));
            return true;
        };
        t.exec.view = super::tabs::ResultView::Chart;
        // Values: added or removed with the list kept open, unless one value is split by a
        // column (it is replaced).
        let mut close = true;
        let mut notice = None;
        match purpose {
            P::ChartX(_) => {
                c.spec.x = col;
                if c.spec.by.is_some() && c.spec.by == col {
                    c.spec.by = None;
                }
                c.cursor = 0;
            }
            P::ChartBy(_) => {
                c.spec.by = col;
                c.spec.ys.truncate(1);
                c.series = 0;
            }
            _ => {
                if let Some(col) = col {
                    if c.spec.by.is_some() {
                        c.spec.ys = vec![col];
                    } else if let Some(at) = c.spec.ys.iter().position(|&y| y == col) {
                        c.spec.ys.remove(at);
                        close = false;
                    } else if c.spec.ys.len() >= chart::MAX_SERIES {
                        let count = chart::MAX_SERIES as u64;
                        notice = Some(Notice::new(Msg::ChartTooManySeries { count }, Level::Warning));
                        close = false;
                    } else {
                        // In the result's order.
                        c.spec.ys.push(col);
                        c.spec.ys.sort_unstable();
                        close = false;
                    }
                    c.series = 0;
                }
            }
        }
        let (ys, names) = (c.spec.ys.clone(), c.names.clone());
        if close {
            self.overlays.close(OverlayKind::Chooser);
        } else if let Some(ch) = self.overlays.chooser_mut() {
            for (v, text) in &mut ch.items {
                if let Some(i) = v.as_deref().and_then(|v| v.parse::<usize>().ok()) {
                    let mark = if ys.contains(&i) { "[x]" } else { "[ ]" };
                    *text = format!("{mark} {}", names[i]);
                }
            }
        }
        if let Some(n) = notice {
            self.flash(n);
        }
        true
    }

    /// A click on the Chart tab: a kind or a column choice on its first lines, else the point
    /// there. Whether it was on a point.
    pub(super) fn chart_click(&mut self, x: u16, y: u16) -> bool {
        let at = |r: &Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let t = self.tab_mut();
        let Some(c) = t.exec.chart.as_mut() else { return false };
        let hit = c.field_hits.iter().find(|(r, _)| at(r)).map(|h| h.1);
        match hit {
            Some(ChartHit::Kind(k)) => c.spec.kind = k,
            Some(ChartHit::Log) => c.spec.log = !c.spec.log,
            Some(ChartHit::X) => self.open_chart_picker(ChartAction::PickX),
            Some(ChartHit::Y) => self.open_chart_picker(ChartAction::PickY),
            Some(ChartHit::By) => self.open_chart_picker(ChartAction::PickBy),
            None => {
                if let Some(i) = c.hits.iter().find(|(r, _)| at(r)).map(|h| h.1) {
                    c.cursor = i;
                    return true;
                } else if c.spec.kind == Kind::Line
                    && at(&c.plot)
                    && let Some(r) = &c.raster
                    && !r.cols.is_empty()
                {
                    // Lines: the point nearest the column clicked.
                    let col = x - c.plot.x;
                    let i = r.cols.partition_point(|&p| p < col);
                    let near = [i.saturating_sub(1), i.min(r.cols.len() - 1)]
                        .into_iter()
                        .min_by_key(|&j| r.cols[j].abs_diff(col))
                        .unwrap_or(0);
                    c.cursor = near;
                    return true;
                }
            }
        }
        false
    }

    /// The wheel over the Chart tab: the cursor moves along the X axis.
    pub(super) fn chart_scroll(&mut self, d: isize) {
        if let Some(c) = self.tab_mut().exec.chart.as_mut() {
            c.step(d.signum());
        }
    }
}

#[cfg(test)]
mod tests;
