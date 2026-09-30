//! The result inspector: a panel on the right of the results that
//! follows the grid's selection. The **Cell** tab shows the selected cell's whole value
//! (wrapped, JSON pretty-printed) with its type and length; the **Row** tab shows every column
//! of the selected row as `name: value`, with the column key marks. `Enter` keeps
//! opening the cell viewer for very long values.

use crate::app::action::{Action, GridAction};
use crate::app::{App, DetailTab, Keys, Results};
use crate::icons;
use crate::keymap::Ctx;
use crate::text::{clip, sanitize_cell, width, wrap};
use crate::theme;
use crate::widgets::grid::{CellRef, ResultSet, viewer_text};
use crate::widgets::{panel, put};
use datarig_core::driver::KeyMarks;
use datarig_core::i18n::{I18n, Label, Localized, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

/// The panel's width for results `total` columns wide: 40% (24 to 60 columns), leaving the
/// grid at least 30; `None` when that leaves no room for it.
pub(crate) fn width_for(total: u16) -> Option<u16> {
    let w = (total * 4 / 10).clamp(24, 60).min(total.saturating_sub(30));
    (w >= 20).then_some(w)
}

/// The tab names as the panel's title shows them (`[Cell] Row`: the active one in brackets),
/// with the columns each one takes from the panel's left edge (a click there switches tabs).
pub(crate) fn tab_labels(i18n: &I18n, active: DetailTab) -> [(String, u16, u16); 2] {
    let mark = |label: Localized, on: bool| if on { format!("[{label}]") } else { label.to_string() };
    let cell = mark(i18n.label(Label::DetailTabCell), active == DetailTab::Cell);
    let row = mark(i18n.label(Label::DetailTabRow), active == DetailTab::Row);
    // The title starts after the corner and a blank; one blank between the two names.
    let cw = width(&cell) as u16;
    let rw = width(&row) as u16;
    [(cell, 2, 2 + cw), (row, 2 + cw + 1, 2 + cw + 1 + rw)]
}

/// The key marks of every result column, from the profile's key cache.
pub(crate) fn column_marks(app: &App, rs: &ResultSet) -> Vec<KeyMarks> {
    let keys = Some(app.tab_keys(app.tab()));
    rs.columns.iter().map(|c| keys.map(|k| k.marks(c.meta.origin)).unwrap_or_default()).collect()
}

/// `text · 12 characters · 14 bytes` (or `text · NULL`).
fn describe(i18n: &I18n, type_name: &str, value: Option<&str>) -> String {
    match value {
        None => format!("{type_name} · {}", i18n.label(Label::ResultsNull)),
        Some(v) => {
            let chars = i18n.msg(&Msg::DetailChars { count: v.chars().count() as u64 });
            if v.len() == v.chars().count() {
                format!("{type_name} · {chars}")
            } else {
                let bytes = i18n.msg(&Msg::DetailBytes { count: v.len() as u64 });
                format!("{type_name} · {chars} · {bytes}")
            }
        }
    }
}

pub(crate) fn draw_inspector(app: &mut App, area: Rect, buf: &mut Buffer) {
    // The title names the tabs, the active one in brackets, and the key that switches them:
    // `[Cell] Row · I`.
    let active = app.detail.tab;
    let [(cell, ..), (row, ..)] = tab_labels(&app.i18n, active);
    let key = app
        .keymap
        .hint_keys(crate::app::action::Action::DetailTab, crate::keymap::Ctx::Grid, app.enhanced_keys)
        .map(|k| crate::keymap::keys::label(&k));
    let title = match key {
        Some(k) => format!("{cell} {row} · {k}"),
        None => format!("{cell} {row}"),
    };
    let focused = app.focus == crate::app::Focus::Inspector;
    let block = panel(&Localized::verbatim(title), focused, area.width);
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height < 2 || inner.width < 8 {
        return;
    }
    let w = inner.width as usize;
    let body = inner;
    let t = app.tab();
    let Results::Rows(rs) = &t.results else { return };
    if rs.rows.is_empty() {
        put(buf, body.x + 1, body.y, &app.i18n.label(Label::DetailNoRows), w - 1, dim());
        return;
    }
    let marks = column_marks(app, rs);
    match active {
        DetailTab::Cell => draw_cell(app, rs, &marks, body, buf),
        DetailTab::Row => draw_row(app, rs, &marks, body, buf),
    }
}

fn dim() -> Style {
    Style::new().fg(theme::FG_DIM).bg(theme::BG)
}

/// The key marks in their colors from `x`; returns the columns used.
fn put_marks(m: KeyMarks, on: bool, x: u16, y: u16, room: usize, bg: ratatui::style::Color, buf: &mut Buffer) -> u16 {
    let mut used = 0u16;
    for k in icons::key_marks(m) {
        let text = format!("{} ", k.text(on));
        let tw = width(&text);
        if used as usize + tw >= room {
            break;
        }
        buf.set_stringn(x + used, y, &text, tw, Style::new().fg(theme::key_color(k)).bg(bg));
        used += tw as u16;
    }
    used
}

/// The Cell tab: the column's marks and name, its type and length, then the value wrapped.
fn draw_cell(app: &App, rs: &ResultSet, marks: &[KeyMarks], body: Rect, buf: &mut Buffer) {
    let t = app.tab();
    let (row, col) = (t.grid.row.min(rs.rows.len() - 1), t.grid.col.min(rs.columns.len().saturating_sub(1)));
    let Some(column) = rs.columns.get(col) else { return };
    let w = body.width as usize - 1;
    let x = body.x + 1;
    let mut y = body.y;
    let bold = Style::new().fg(theme::FG).bg(theme::BG).add_modifier(Modifier::BOLD);
    let used = put_marks(marks.get(col).copied().unwrap_or_default(), app.icons_on(), x, y, w, theme::BG, buf);
    put(buf, x + used, y, &column.meta.name, w.saturating_sub(used as usize), bold);
    y += 1;
    let not_read = app.i18n.label(Label::ResultsNotRead);
    let cell = rs.cell(row, col);
    let value = match cell {
        CellRef::Here(c) => c.as_deref(),
        _ => None,
    };
    if cell == CellRef::NotRead {
        put(buf, x, y, &not_read, w, dim());
        return;
    }
    put(buf, x, y, &describe(&app.i18n, &column.meta.type_name, value), w, dim());
    y += 2;
    let bottom = body.y + body.height;
    if y >= bottom {
        return;
    }
    let null = app.i18n.label(Label::ResultsNull);
    let text = viewer_text(rs, &t.grid, &null, &not_read).map(|(_, t)| t).unwrap_or_default();
    let style = if value.is_none() {
        Style::new().fg(theme::NULL_FG).bg(theme::BG).add_modifier(Modifier::ITALIC)
    } else {
        Style::new().fg(theme::FG).bg(theme::BG)
    };
    let lines = wrap(&text, w);
    let room = (bottom - y) as usize;
    let cut = lines.len() > room;
    let shown = if cut { room.saturating_sub(1) } else { lines.len() };
    for l in lines.iter().take(shown) {
        buf.set_stringn(x, y, l, w, style);
        y += 1;
    }
    if cut {
        put(
            buf,
            x,
            y,
            &app.i18n.msg(&Msg::DetailMore { key: app.key_for(Action::Grid(GridAction::ViewCell), Ctx::Grid) }),
            w,
            Style::new().fg(theme::ACCENT).bg(theme::BG),
        );
    }
}

/// The Row tab: every column of the selected row, `name: value` on one line each (key marks
/// first), the selected column highlighted and kept in view.
fn draw_row(app: &App, rs: &ResultSet, marks: &[KeyMarks], body: Rect, buf: &mut Buffer) {
    let t = app.tab();
    let row = t.grid.row.min(rs.rows.len() - 1);
    let w = body.width as usize - 1;
    let x = body.x + 1;
    let mut y = body.y;
    let bottom = body.y + body.height;
    let keys_failed = matches!(app.tab_keys(t), Keys::Failed(_));
    if keys_failed && y < bottom {
        put(buf, x, y, &app.i18n.label(Label::DetailKeysUnknown), w, dim());
        y += 1;
    }
    let room = (bottom.saturating_sub(y)) as usize;
    let first = t.grid.col.saturating_sub(room.saturating_sub(1));
    let null = app.i18n.label(Label::ResultsNull);
    let on = app.icons_on();
    for (ci, column) in rs.columns.iter().enumerate().skip(first).take(room) {
        let selected = ci == t.grid.col;
        let bg = if selected { theme::SELECTION_BG } else { theme::BG };
        buf.set_style(Rect::new(body.x, y, body.width, 1), Style::new().bg(bg));
        let mut cx = x + put_marks(marks.get(ci).copied().unwrap_or_default(), on, x, y, w, bg, buf);
        let name = format!("{}: ", column.meta.name);
        let left = w.saturating_sub((cx - x) as usize);
        cx += put(buf, cx, y, &name, left, Style::new().fg(theme::FG_MUTED).bg(bg).add_modifier(Modifier::BOLD));
        let left = w.saturating_sub((cx - x) as usize);
        match rs.cell(row, ci) {
            CellRef::Here(Some(v)) => {
                put(buf, cx, y, &clip(&sanitize_cell(v), left), left, Style::new().fg(theme::FG).bg(bg))
            }
            CellRef::Here(None) | CellRef::Missing => {
                put(buf, cx, y, &null, left, Style::new().fg(theme::NULL_FG).bg(bg).add_modifier(Modifier::ITALIC))
            }
            CellRef::NotRead => put(buf, cx, y, &app.i18n.label(Label::ResultsNotRead), left, dim().bg(bg)),
        };
        y += 1;
    }
}

/// The one-line preview of the selected cell for `detail_view = statusbar`.
pub(crate) fn preview(app: &App) -> Option<Localized> {
    let t = app.tab();
    let Results::Rows(rs) = &t.results else { return None };
    let column = rs.columns.get(t.grid.col)?;
    let value = match rs.cell(t.grid.row, t.grid.col) {
        CellRef::Here(Some(v)) => sanitize_cell(v),
        CellRef::Here(None) => app.i18n.label(Label::ResultsNull).to_string(),
        CellRef::NotRead => app.i18n.label(Label::ResultsNotRead).to_string(),
        CellRef::Missing => return None,
    };
    Some(app.i18n.msg(&Msg::DetailPreview { column: column.meta.name.clone(), value }))
}
