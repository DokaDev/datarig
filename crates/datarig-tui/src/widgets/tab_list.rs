//! The tab list (`Space t t`): a filter line, then one row per tab with its number, name, kind,
//! where it works (`profile · database.schema`) and the tab bar's marks, the closed tabs under a
//! heading of their own. The number and the marks are the tab bar's own (`tabbar::number`,
//! `tabbar::marks`): the profile's color while the tab's session is connected, the spinner
//! while a statement runs, `*`, `RO`, `◆` and `!`.

use crate::app::App;
use crate::app::overlay::OverlayKind;
use crate::app::tab_list::{TabEntry, kind_label};
use crate::icons;
use crate::text::{Align, fit, width};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::put;
use crate::widgets::tabbar;
use datarig_core::i18n::Label;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// A line of the list as drawn: a heading, or an entry's parts (number, name, kind, place,
/// marks) and whether it is the active tab.
enum Line {
    Heading(String),
    Entry { index: usize, parts: Parts, active: bool },
}

struct Parts {
    number: (String, Color),
    name: String,
    kind: String,
    /// The profile's icon cell and its color, and where the tab works.
    icon: Option<(String, Color)>,
    /// Where it works, or what it says without a connection.
    place: Result<String, Label>,
    marks: Vec<(String, Style)>,
}

/// Draw the tab list; returns the hardware cursor (its filter).
pub(crate) fn draw_tab_list(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let th = theme::cur();
    let l = app.overlays.tab_list()?;
    let (entries, selected, hover, open) = (l.entries.clone(), l.selected, l.hover, l.open_count());
    let icons_on = app.icons_on();
    let active = (!app.tabs.is_empty()).then(|| app.tab().id);
    let mut lines: Vec<Line> = Vec::new();
    for (index, e) in entries.iter().enumerate() {
        if index == open {
            lines.push(Line::Heading(app.i18n.label(Label::TabListClosed).to_string()));
        }
        let parts = match *e {
            TabEntry::Open(id) => {
                let Some(t) = app.tabs.get(id) else { continue };
                let n = app.tabs.position(id).unwrap_or(0);
                let profile = t.profile.and_then(|p| app.profile(p));
                Parts {
                    number: tabbar::number(app, n, t),
                    name: tabbar::document_name(app, t),
                    kind: app.i18n.label(kind_label(t.kind)).to_string(),
                    icon: profile.map(|p| (icons::cell(p, icons_on), theme::profile_color(p.display_color()))),
                    place: app
                        .tab_place(t.profile, &t.context)
                        .ok_or_else(|| tabbar::unbound(app, t).unwrap_or(Label::TabUnbound)),
                    marks: tabbar::marks(app, t, th.surface),
                }
            }
            TabEntry::Closed(serial) => {
                let Some(c) = app.tabs.closed().find(|c| c.serial == serial) else { continue };
                let profile = c.profile.and_then(|p| app.profile(p));
                Parts {
                    number: (String::new(), th.fg_dim),
                    name: app.closed_name(c),
                    kind: app.i18n.label(kind_label(c.kind)).to_string(),
                    icon: profile.map(|p| (icons::cell(p, icons_on), theme::profile_color(p.display_color()))),
                    place: app.tab_place(c.profile, &c.context).ok_or(Label::TabUnbound),
                    marks: Vec::new(),
                }
            }
        };
        let is_active = matches!(e, TabEntry::Open(id) if Some(*id) == active);
        lines.push(Line::Entry { index, parts, active: is_active });
    }
    let title = app.i18n.label(Label::TabListTitle);
    let footer = app.i18n.label(Label::TabListKeys);
    let w = area.width.saturating_sub(8).min(100);
    let rows = lines.len().clamp(1, (area.height as usize).saturating_sub(6).max(1));
    let rect = centered(area, w, rows as u16 + 4);
    let inner = modal(rect, &title, &footer, buf);
    let iw = inner.width as usize;
    put(buf, inner.x + 1, inner.y, "›", 1, Style::new().fg(th.accent).bg(th.surface).add_modifier(Modifier::BOLD));
    let input = Rect::new(inner.x + 3, inner.y, inner.width.saturating_sub(4), 1);
    let placeholder = app.i18n.label(Label::TabListPlaceholder);
    let none = app.i18n.label(Label::TabListNone);
    // Column widths: the number, the kind and the marks as wide as their widest; the name and
    // the place share the rest, the name first.
    let entry_parts =
        || lines.iter().filter_map(|l| if let Line::Entry { parts, .. } = l { Some(parts) } else { None });
    let num_w = entry_parts().map(|p| width(&p.number.0)).max().unwrap_or(0).max(2);
    let kind_w = entry_parts().map(|p| width(&p.kind)).max().unwrap_or(0);
    // Each mark has a blank before it.
    let marks_w = entry_parts().map(|p| p.marks.iter().map(|m| width(&m.0)).sum::<usize>()).max().unwrap_or(0);
    let name_max = entry_parts().map(|p| width(&p.name)).max().unwrap_or(0);
    // One blank at each side, two between the columns (the marks bring one of their own).
    let rest = iw.saturating_sub(2 + num_w + marks_w + 2 + 2 + kind_w + 2);
    let name_w = name_max.min(rest * 11 / 20).max(rest.min(8));
    let place_w = rest.saturating_sub(name_w);
    // Keep the selection on screen; the lines stay put while it moves among them.
    let line_of = |i: usize| i + usize::from(i >= open && open < entries.len());
    let top = app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::TabList);
    let now = app.now();
    let l = app.overlays.tab_list_mut()?;
    l.press.drawn(top, now);
    let sel_line = line_of(selected);
    if sel_line < l.scroll {
        l.scroll = sel_line;
    } else if sel_line >= l.scroll + rows {
        l.scroll = sel_line + 1 - rows;
    }
    l.scroll = l.scroll.min(lines.len().saturating_sub(rows));
    l.rows.clear();
    let first = l.scroll;
    let dim = Style::new().fg(th.fg_dim);
    for (i, line) in lines.iter().enumerate().skip(first).take(rows) {
        let y = inner.y + 2 + (i - first) as u16;
        match line {
            Line::Heading(text) => {
                let style = Style::new().fg(th.fg_muted).bg(th.surface).add_modifier(Modifier::BOLD);
                buf.set_stringn(inner.x, y, fit("", iw, Align::Left), iw, Style::new().bg(th.surface));
                put(buf, inner.x + 1, y, text, iw.saturating_sub(2), style);
            }
            Line::Entry { index, parts, active } => {
                let bg = if *index == selected {
                    th.selection
                } else if hover == Some(*index) {
                    crate::widgets::hover_style()
                } else {
                    Style::new().bg(th.surface)
                };
                buf.set_stringn(inner.x, y, fit("", iw, Align::Left), iw, bg);
                l.rows.push((Rect::new(inner.x, y, inner.width, 1), *index));
                let end = inner.x + inner.width.saturating_sub(1);
                let mut x = inner.x + 1;
                let mut col = |text: &str, w: usize, style: Style, x: &mut u16| {
                    let room = (end.saturating_sub(*x) as usize).min(w);
                    put(buf, *x, y, &fit(text, room, Align::Left), room, style.patch(bg));
                    *x += room as u16;
                };
                let mut number = Style::new().fg(parts.number.1);
                let mut name = Style::new().fg(th.fg);
                if *active {
                    number = number.add_modifier(Modifier::BOLD);
                    name = name.add_modifier(Modifier::BOLD);
                }
                col(&format!("{:>num_w$}", parts.number.0), num_w, number, &mut x);
                let marks_end = x + marks_w as u16;
                for (text, style) in &parts.marks {
                    col(text, width(text), *style, &mut x);
                }
                col("", usize::from(marks_end - x) + 2, dim, &mut x);
                col(&parts.name, name_w, name, &mut x);
                col("", 2, dim, &mut x);
                col(&parts.kind, kind_w, Style::new().fg(th.fg_muted), &mut x);
                col("", 2, dim, &mut x);
                let mut left = place_w;
                if let Some((icon, color)) = &parts.icon {
                    let used = width(icon).min(left);
                    col(icon, used, Style::new().fg(*color), &mut x);
                    left -= used;
                }
                match &parts.place {
                    Ok(place) => col(place, left, Style::new().fg(th.fg_muted), &mut x),
                    Err(l) => col(&app.i18n.label(*l), left, dim, &mut x),
                }
            }
        }
    }
    if lines.is_empty() {
        put(buf, inner.x + 1, inner.y + 2, &none, iw.saturating_sub(2), Style::new().fg(th.fg_dim).bg(th.surface));
    }
    let l = app.overlays.tab_list_mut()?;
    let cx = l.filter.render(input, buf, Style::new().fg(th.fg).bg(th.surface), true, false, None);
    if l.filter.text().is_empty() {
        put(buf, input.x, input.y, &placeholder, input.width as usize, Style::new().fg(th.fg_dim).bg(th.surface));
    }
    Some((cx, inner.y))
}
