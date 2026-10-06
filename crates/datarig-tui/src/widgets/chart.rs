//! The Chart tab of the results pane: a line with the kinds of chart (the shown one in
//! brackets) and how many fetched rows it draws, a line with the columns chosen, the chart,
//! and under it the series' colors, the values at the cursor and what was left out.
//!
//! Bars are drawn with block characters in eighths of a cell, lines with braille dots (2×4 a
//! cell). A frame draws from the numbers read before ([`ChartTab::built`]): bars walk the bars
//! on screen; a line chart is rasterized once per size and kept ([`ChartTab::raster`]), walking
//! every point once and each dot column a bounded number of times. [`work`] counts both for
//! the benchmark.

use crate::app::chart::{ChartHit, ChartTab, Drawn, Raster};
use crate::app::hover::PointerOn;
use crate::app::{App, Focus, Results};
use crate::text::{Align, clip, fit, sanitize_cell, width};
use crate::theme::{self, Theme};
use crate::widgets::put;
use datarig_core::chart::{Axis, Kind, Model, TimeKind, Unsuitable, scale};
use datarig_core::i18n::{I18n, Label, Localized, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use std::cell::Cell;

thread_local! {
    static WORK: Cell<u64> = const { Cell::new(0) };
}

/// Rows, points, bars and dot columns the chart walked on this thread since the last
/// [`take_work`] (the benchmark's work count).
pub fn take_work() -> u64 {
    WORK.with(|w| w.replace(0))
}

pub(crate) fn work(n: u64) {
    WORK.with(|w| w.set(w.get() + n));
}

/// Bars growing up, by eighths of a cell.
const UP: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
/// Bars growing right, by eighths of a cell.
const RIGHT: [&str; 9] = [" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];

/// What the chart is drawn with.
pub(crate) struct Look<'a> {
    pub i18n: &'a I18n,
    pub th: &'a Theme,
    pub focused: bool,
    /// The part of the first lines under the pointer.
    pub hover: Option<ChartHit>,
    /// The keys that choose the X column, the values, the series column and the kind.
    pub keys: [String; 4],
}

/// The result the chart is of: its rows and whether the server has more.
#[derive(Clone, Copy)]
pub(crate) struct Info {
    pub rows: usize,
    pub more: bool,
}

/// The active tab's chart, in `area` (inside the results pane, below its strip).
pub(crate) fn draw_chart(app: &mut App, area: Rect, buf: &mut Buffer) {
    app.chart_sync();
    let th = theme::cur();
    buf.set_style(area, th.base());
    let focused = matches!(app.focus, Focus::Results | Focus::Inspector);
    let hover = match app.pointer_hover() {
        Some(PointerOn::Chart(h)) => Some(h),
        _ => None,
    };
    let keys = chart_keys(app);
    let t = app.tabs.active_mut();
    let info = match &t.results {
        Results::Rows(rs) => Info { rows: rs.rows.len(), more: rs.more },
        _ => Info { rows: 0, more: false },
    };
    // Taken out while it is drawn (it keeps what the frame drew: hits, the raster).
    let Some(mut c) = t.exec.chart.take() else { return };
    let cx = Look { i18n: &app.i18n, th: &th, focused, hover, keys };
    draw_into(&cx, &mut c, info, area, buf);
    // Laid out otherwise: the part under the pointer is not there any more.
    if let Some(h) = hover
        && !c.field_hits.iter().any(|f| f.1 == h)
    {
        app.pointer_on = None;
    }
    app.tabs.active_mut().exec.chart = Some(c);
}

/// The keys bound to the chart's choices (shown where there is nothing to draw).
fn chart_keys(app: &App) -> [String; 4] {
    use crate::app::action::Action;
    use crate::app::chart::ChartAction as C;
    let key = |a| {
        app.keymap
            .hint_keys(Action::Chart(a), crate::keymap::Ctx::Chart, app.enhanced_keys)
            .map(|k| crate::keymap::keys::label(&k))
            .unwrap_or_default()
    };
    [key(C::PickX), key(C::PickY), key(C::PickBy), key(C::NextKind(true))]
}

/// The active tab's chart as text, as large as it was drawn last (`y`/`Y` copies it).
pub(crate) fn as_text(app: &mut App) -> String {
    let th = theme::cur();
    let keys = chart_keys(app);
    let t = app.tabs.active_mut();
    let info = match &t.results {
        Results::Rows(rs) => Info { rows: rs.rows.len(), more: rs.more },
        _ => Info { rows: 0, more: false },
    };
    let Some(mut c) = t.exec.chart.take() else { return String::new() };
    let saved = (c.hits.clone(), c.field_hits.clone(), c.plot, c.area, c.offset);
    let area =
        if c.area.width > 0 && c.area.height > 0 { Rect { x: 0, y: 0, ..c.area } } else { Rect::new(0, 0, 100, 30) };
    let mut buf = Buffer::empty(area);
    let cx = Look { i18n: &app.i18n, th: &th, focused: false, hover: None, keys };
    draw_into(&cx, &mut c, info, area, &mut buf);
    // What the screen shows stays: the copy drew elsewhere.
    (c.hits, c.field_hits, c.plot, c.area, c.offset) = saved;
    app.tabs.active_mut().exec.chart = Some(c);
    let mut lines = Vec::new();
    for y in 0..area.height {
        let mut line = String::new();
        let mut x = 0;
        while x < area.width {
            let cell = &buf[(x, y)];
            line.push_str(cell.symbol());
            x += width(cell.symbol()).max(1) as u16;
        }
        lines.push(line.trim_end().to_string());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

/// The chart `c` in `area`: its first lines, the chart and what is under it.
pub(crate) fn draw_into(cx: &Look, c: &mut ChartTab, info: Info, area: Rect, buf: &mut Buffer) {
    c.hits.clear();
    c.field_hits.clear();
    c.plot = Rect::default();
    c.area = area;
    if area.height == 0 || area.width == 0 {
        return;
    }
    kind_bar(cx, c, info, Rect { height: 1, ..area }, buf);
    let mut y = area.y + 1;
    if area.height >= 6 {
        fields(cx, c, Rect { y, height: 1, ..area }, buf);
        y += 1;
    }
    let body = Rect { y, height: area.y + area.height - y, ..area };
    if body.height == 0 {
        return;
    }
    match c.built.as_ref().map(|b| b.1.clone()) {
        Some(Drawn::Model(m)) => draw_model(cx, c, &m, body, buf),
        Some(Drawn::Unsuitable(u)) => message(cx, &unsuitable(cx, u), body, buf),
        Some(Drawn::Unreadable(_)) => message(cx, &[cx.i18n.label(Label::ChartUnreadable).to_string()], body, buf),
        None => {}
    }
}

/// The kinds (the shown one in brackets, each where the pointer picks it), and on the right
/// how many fetched rows the chart draws and whether the server has more.
fn kind_bar(cx: &Look, c: &mut ChartTab, info: Info, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    buf.set_style(area, Style::new().bg(th.surface));
    let count = info.rows as u64;
    let (long, short, style) = if info.more {
        (
            cx.i18n.msg(&Msg::ChartRowsMore { count }),
            cx.i18n.msg(&Msg::ChartRowsMoreShort { count }),
            Style::new().fg(th.warning).bg(th.surface),
        )
    } else {
        (
            cx.i18n.msg(&Msg::ChartRows { count }),
            cx.i18n.msg(&Msg::ChartRowsShort { count }),
            Style::new().fg(th.fg_muted).bg(th.surface),
        )
    };
    // The kinds take what they need; the note its long form when it fits after them.
    let kinds: u16 = Kind::ALL.iter().map(|k| width(&cx.i18n.label(kind_label(*k))) as u16 + 3).sum();
    let note = if kinds + width(&long) as u16 + 3 <= area.width { long } else { short };
    let note_w = width(&note) as u16;
    let mut x = area.x + 1;
    let end = area.x + area.width;
    for k in Kind::ALL {
        let on = k == c.spec.kind;
        let label = cx.i18n.label(kind_label(k));
        let text = if on { format!("[{label}]") } else { format!(" {label} ") };
        let w = width(&text) as u16;
        if x + w > end {
            break;
        }
        let mut style = if on {
            Style::new().fg(th.accent).bg(th.surface).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(th.fg_muted).bg(th.surface)
        };
        if cx.hover == Some(ChartHit::Kind(k)) {
            style = style.patch(crate::widgets::pointer_style());
        }
        put(buf, x, area.y, &text, w as usize, style);
        c.field_hits.push((Rect::new(x, area.y, w, 1), ChartHit::Kind(k)));
        x += w + 1;
    }
    if x + note_w + 2 <= end {
        put(buf, end - note_w - 1, area.y, &note, note_w as usize, style);
    }
}

pub(crate) fn kind_label(k: Kind) -> Label {
    match k {
        Kind::Bar => Label::ChartKindBar,
        Kind::HBar => Label::ChartKindHbar,
        Kind::Line => Label::ChartKindLine,
    }
}

/// The columns chosen: `X day · Y orders, revenue · by none · linear`, each part where a click
/// changes it.
fn fields(cx: &Look, c: &mut ChartTab, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let name = |i: Option<usize>, none: Label| match i.and_then(|i| c.names.get(i)) {
        Some(n) => sanitize_cell(n),
        None => cx.i18n.label(none).to_string(),
    };
    let ys = if c.spec.ys.is_empty() {
        cx.i18n.label(Label::ChartNone).to_string()
    } else {
        c.spec.ys.iter().filter_map(|&i| c.names.get(i)).map(|n| sanitize_cell(n)).collect::<Vec<_>>().join(", ")
    };
    let scale = cx.i18n.label(if c.spec.log { Label::ChartScaleLog } else { Label::ChartScaleLinear });
    let parts = [
        (ChartHit::X, cx.i18n.label(Label::ChartFieldX).to_string(), name(c.spec.x, Label::ChartRowNumber)),
        (ChartHit::Y, cx.i18n.label(Label::ChartFieldY).to_string(), ys),
        (ChartHit::By, cx.i18n.label(Label::ChartFieldBy).to_string(), name(c.spec.by, Label::ChartNone)),
        (ChartHit::Log, String::new(), scale.to_string()),
    ];
    let mut x = area.x + 1;
    let end = area.x + area.width;
    for (i, (hit, label, value)) in parts.into_iter().enumerate() {
        if i > 0 {
            if x + 3 >= end {
                break;
            }
            put(buf, x, area.y, " · ", 3, Style::new().fg(th.fg_dim).bg(th.bg));
            x += 3;
        }
        let hovered = cx.hover == Some(hit);
        let patch = |s: Style| if hovered { s.patch(crate::widgets::pointer_style()) } else { s };
        let start = x;
        if !label.is_empty() {
            x += put(
                buf,
                x,
                area.y,
                &format!("{label} "),
                (end - x) as usize,
                patch(Style::new().fg(th.fg_muted).bg(th.bg)),
            );
        }
        if x < end {
            x += put(buf, x, area.y, &value, (end - x) as usize, patch(Style::new().fg(th.fg).bg(th.bg)));
        }
        c.field_hits.push((Rect::new(start, area.y, x - start, 1), hit));
        if x >= end {
            break;
        }
    }
}

/// Why there is no chart, and what to do about it.
fn unsuitable(cx: &Look, u: Unsuitable) -> Vec<String> {
    let reason = match u {
        Unsuitable::NoRows => Label::ChartUnsuitableNoRows,
        Unsuitable::NoNumber => Label::ChartUnsuitableNoNumber,
        Unsuitable::NoSeries => Label::ChartUnsuitableNoSeries,
        Unsuitable::NoValues => Label::ChartUnsuitableNoValues,
        Unsuitable::OnePoint => Label::ChartUnsuitableOnePoint,
    };
    let [x, y, by, kind] = cx.keys.clone();
    let mut out = vec![cx.i18n.label(reason).to_string()];
    if !matches!(u, Unsuitable::NoRows | Unsuitable::NoNumber) {
        out.push(cx.i18n.msg(&Msg::ChartKeysHint { x, y, by, kind }).to_string());
    }
    out
}

/// Lines of text at the top of `area`, wrapped.
fn message(cx: &Look, lines: &[String], area: Rect, buf: &mut Buffer) {
    let w = area.width.saturating_sub(2) as usize;
    let mut y = area.y + area.height.min(1);
    for (i, l) in lines.iter().enumerate() {
        let style =
            if i == 0 { Style::new().fg(cx.th.fg).bg(cx.th.bg) } else { Style::new().fg(cx.th.fg_muted).bg(cx.th.bg) };
        for part in crate::text::wrap_words(l, w.max(1)) {
            if y >= area.y + area.height {
                return;
            }
            put(buf, area.x + 1, y, &part, w, style);
            y += 1;
        }
    }
}

/// The color of series `s`: the theme's chart colors in turn, "others" muted.
fn series_color(th: &Theme, m: &Model, s: usize) -> Color {
    if m.series.get(s).is_some_and(|s| s.others) { th.fg_muted } else { th.chart()[s % 6] }
}

/// The value axis of `m` with at most `max_ticks` ticks; bars start at zero.
fn value_ticks(m: &Model, log: bool, bars: bool, max_ticks: usize) -> scale::Ticks {
    let (lo, hi) = m.range(log).unwrap_or((0.0, 1.0));
    if log { scale::log(lo, hi, max_ticks) } else { scale::linear(lo, hi, max_ticks, bars) }
}

/// The chart of `m` in `body`, with the legend, the readout and the notes under it.
fn draw_model(cx: &Look, c: &mut ChartTab, m: &Model, body: Rect, buf: &mut Buffer) {
    let notes = notes(cx, c, m);
    let legend = m.series.len() > 1;
    let mut below = 1 + u16::from(legend) + u16::from(!notes.is_empty());
    // Small: the notes go first, then the legend.
    let need = |below: u16| body.height.saturating_sub(below) >= 5;
    let notes = if need(below) { notes } else { Vec::new() };
    below = 1 + u16::from(legend) + u16::from(!notes.is_empty());
    let legend = legend && need(below);
    below = 1 + u16::from(legend) + u16::from(!notes.is_empty());
    if !need(below) || body.width < 16 {
        return message(cx, &[cx.i18n.label(Label::ChartTooSmall).to_string()], body, buf);
    }
    let chart = Rect { height: body.height - below, ..body };
    match c.spec.kind {
        Kind::Bar => bars(cx, c, m, chart, buf),
        Kind::HBar => hbars(cx, c, m, chart, buf),
        Kind::Line => line(cx, c, m, chart, buf),
    }
    let mut y = chart.y + chart.height;
    if legend {
        draw_legend(cx, c, m, Rect { y, height: 1, ..body }, buf);
        y += 1;
    }
    readout(cx, c, m, Rect { y, height: 1, ..body }, buf);
    y += 1;
    if !notes.is_empty() {
        let text = notes.join(" · ");
        put(
            buf,
            body.x + 1,
            y,
            &text,
            body.width.saturating_sub(2) as usize,
            Style::new().fg(cx.th.fg_dim).bg(cx.th.bg),
        );
    }
}

/// What was left out or summed, said under the chart.
fn notes(cx: &Look, c: &ChartTab, m: &Model) -> Vec<String> {
    let mut v = Vec::new();
    let i = cx.i18n;
    if m.skipped.nulls() > 0 {
        v.push(i.msg(&Msg::ChartNoteNulls { count: m.skipped.nulls() }).to_string());
    }
    if m.skipped.not_numbers() > 0 {
        v.push(i.msg(&Msg::ChartNoteNotNumbers { count: m.skipped.not_numbers() }).to_string());
    }
    if m.merged {
        v.push(i.label(Label::ChartNoteSummed).to_string());
    }
    if m.other_points > 0 {
        v.push(i.msg(&Msg::ChartNoteOthers { count: m.other_points as u64 }).to_string());
    }
    if m.other_series > 0 {
        v.push(i.msg(&Msg::ChartNoteOthersSeries { count: m.other_series as u64 }).to_string());
    }
    if c.spec.log && m.nonpositive > 0 {
        v.push(i.msg(&Msg::ChartNoteLog { count: m.nonpositive as u64 }).to_string());
    }
    v
}

/// The label of point `i` along the X axis.
fn point_label(cx: &Look, m: &Model, i: usize) -> String {
    match m.points.get(i) {
        Some(p) if p.others => cx.i18n.label(Label::ChartOthers).to_string(),
        Some(p) => sanitize_cell(&p.label),
        None => String::new(),
    }
}

/// The value axis' labels in a gutter left of `plot` (right-aligned), a tick on the axis line
/// at each: for bars on the cell a bar of that value reaches, for lines on the dot's cell.
fn y_axis(cx: &Look, t: &scale::Ticks, labels: &[String], gutter: u16, plot: Rect, bars: bool, buf: &mut Buffer) {
    let th = cx.th;
    let axis = plot.x - 1;
    let muted = Style::new().fg(th.fg_muted).bg(th.bg);
    let line = Style::new().fg(th.border).bg(th.bg);
    for y in plot.y..plot.y + plot.height {
        put(buf, axis, y, "│", 1, line);
    }
    let mut used = None;
    for (v, label) in t.values.iter().zip(labels) {
        let Some(f) = t.at(*v) else { continue };
        let h = f64::from(plot.height);
        let r = if bars { (f * h).round().min(h - 1.0) } else { (f * (h - 1.0)).round() };
        let y = plot.y + plot.height - 1 - r.max(0.0) as u16;
        if used == Some(y) {
            continue;
        }
        used = Some(y);
        put(buf, axis - gutter, y, &fit(label, gutter as usize, Align::Right), gutter as usize, muted);
        put(buf, axis, y, "┤", 1, line);
    }
}

/// The cells of a bar along a scale `cells` long (from 0 at its start): from the baseline
/// to `v`, in eighths; each cell's eighths filled, and whether it grows away from the start.
/// The baseline is a cell boundary. A value off the baseline fills at least an eighth.
fn bar_cells(t: &scale::Ticks, v: f64, cells: u16) -> Option<(i64, i64)> {
    let n = i64::from(cells) * 8;
    let base = if t.log { 0 } else { (t.at(0.0).unwrap_or(0.0) * f64::from(cells)).round() as i64 * 8 };
    let mut at = (t.at(v)? * n as f64).round() as i64;
    if at == base && v != 0.0 && !t.log {
        at = if v > 0.0 { base + 1 } else { base - 1 };
    }
    if t.log && at == 0 {
        at = 1;
    }
    Some((base.clamp(0, n), at.clamp(0, n)))
}

/// Vertical bars: a group per point (a bar per series), scrolled to the cursor.
fn bars(cx: &Look, c: &mut ChartTab, m: &Model, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let plot_h = area.height.saturating_sub(2);
    let ticks = value_ticks(m, c.spec.log, true, (plot_h as usize / 2).clamp(2, 8));
    let labels = ticks.labels();
    let gutter = labels.iter().map(|l| width(l)).max().unwrap_or(1).min(12) as u16;
    let plot = Rect { x: area.x + gutter + 2, y: area.y, width: area.width.saturating_sub(gutter + 3), height: plot_h };
    if plot.width < 4 || plot.height < 2 {
        return message(cx, &[cx.i18n.label(Label::ChartTooSmall).to_string()], area, buf);
    }
    c.plot = plot;
    y_axis(cx, &ticks, &labels, gutter, plot, true, buf);
    let (n, k) = (m.points.len(), m.series.len().max(1));
    // Groups as wide as the plot allows (bars of at least a column with a gap between groups,
    // at most 16 columns a bar); when they all fit they spread over the whole width.
    let natural = plot.width as usize / n.max(1);
    let gw = natural.max(k + 1).min(k * 16 + 4);
    // Not even one group of a column a bar: too narrow.
    if gw > plot.width as usize {
        return message(cx, &[cx.i18n.label(Label::ChartTooSmall).to_string()], area, buf);
    }
    let fits = (plot.width as usize / gw).max(1).min(n);
    let span = if fits == n && natural <= k * 16 + 4 { plot.width as usize } else { fits * gw };
    let start = |j: usize| plot.x + (j * span / fits) as u16;
    let bw = ((gw - 1) / k).max(1);
    scroll_to(c, fits, n);
    let cursor_bg = th.cursor_line.bg.filter(|_| cx.focused);
    let label_y = plot.y + plot.height + 1;
    for i in c.offset..(c.offset + fits).min(n) {
        work(1);
        let (gx, gwi) = (start(i - c.offset), start(i - c.offset + 1) - start(i - c.offset));
        let pad = (usize::from(gwi) - bw * k) / 2;
        if i == c.cursor
            && let Some(bg) = cursor_bg
        {
            buf.set_style(Rect::new(gx, plot.y, gwi.min(plot.x + plot.width - gx), plot.height), Style::new().bg(bg));
        }
        for (s, series) in m.series.iter().enumerate() {
            let Some(v) = series.values[i] else { continue };
            let Some((base, at)) = bar_cells(&ticks, v, plot.height) else { continue };
            let x = gx + (pad + s * bw) as u16;
            let style = Style::new().fg(series_color(th, m, s));
            for r in 0..plot.height {
                let (lo, hi) = (i64::from(r) * 8, i64::from(r) * 8 + 8);
                let y = plot.y + plot.height - 1 - r;
                let glyph = if at >= base {
                    UP[(hi.min(at) - lo.max(base)).clamp(0, 8) as usize]
                } else {
                    match hi.min(base) - lo.max(at) {
                        8.. => "█",
                        4..=7 => "▀",
                        1..=3 => "▔",
                        _ => " ",
                    }
                };
                if glyph != " " {
                    for dx in (0..bw as u16).filter(|dx| x + dx < plot.x + plot.width) {
                        buf[(x + dx, y)].set_symbol(glyph).set_style(style);
                    }
                }
            }
        }
        c.hits.push((Rect::new(gx, plot.y, gwi, plot.height + 2), i));
    }
    // The axis line, and the labels under the groups (every few when they are narrow).
    let line = Style::new().fg(th.border).bg(th.bg);
    put(buf, plot.x - 1, plot.y + plot.height, "└", 1, line);
    put(buf, plot.x, plot.y + plot.height, &"─".repeat(plot.width as usize), plot.width as usize, line);
    // Labels at least this wide (or whole), every few groups when the groups are narrower.
    let widest = (c.offset..(c.offset + fits).min(n)).map(|i| width(&point_label(cx, m, i))).max().unwrap_or(1);
    let want = widest.min(10);
    let every = if gw > want { 1 } else { (want + 1).div_ceil(gw).max(1) };
    let room = (every * gw).saturating_sub(1).max(1);
    let muted = Style::new().fg(th.fg_muted).bg(th.bg);
    let mut drawn = Vec::new();
    for i in (c.offset..(c.offset + fits).min(n)).step_by(every) {
        drawn.push(i);
    }
    if !drawn.contains(&c.cursor) && (c.offset..c.offset + fits).contains(&c.cursor) {
        drawn.push(c.cursor);
    }
    for i in drawn {
        let (gx, gwi) = (start(i - c.offset), usize::from(start(i - c.offset + 1) - start(i - c.offset)));
        let label = clip(&point_label(cx, m, i), room);
        let lw = width(&label);
        let x = if every == 1 { gx + ((gwi.saturating_sub(lw)) / 2) as u16 } else { gx };
        let style = if i == c.cursor { label_style(cx) } else { muted };
        let x = x.min((plot.x + plot.width).saturating_sub(lw as u16));
        put(buf, x, label_y, &label, (plot.x + plot.width - x) as usize, style);
    }
    scroll_marks(cx, c, fits, n, Rect::new(plot.x, label_y, plot.width, 1), buf);
}

/// The style of the label at the cursor.
fn label_style(cx: &Look) -> Style {
    Style::new().fg(cx.th.fg).bg(cx.th.bg).patch(cx.th.selection).add_modifier(Modifier::BOLD)
}

/// Keep the cursor's point among the `fits` drawn of `n`.
fn scroll_to(c: &mut ChartTab, fits: usize, n: usize) {
    if c.cursor < c.offset {
        c.offset = c.cursor;
    }
    if c.cursor >= c.offset + fits {
        c.offset = c.cursor + 1 - fits;
    }
    c.offset = c.offset.min(n.saturating_sub(fits));
}

/// `‹` and `›` at the ends of `at` when points are left out on that side.
fn scroll_marks(cx: &Look, c: &ChartTab, fits: usize, n: usize, at: Rect, buf: &mut Buffer) {
    let style = Style::new().fg(cx.th.accent).bg(cx.th.bg).add_modifier(Modifier::BOLD);
    if c.offset > 0 {
        put(buf, at.x.saturating_sub(1), at.y, "‹", 1, style);
    }
    if c.offset + fits < n {
        put(buf, at.x + at.width - 1, at.y, "›", 1, style);
    }
}

/// Horizontal bars: a line per point and series, labels on the left, scrolled to the cursor.
fn hbars(cx: &Look, c: &mut ChartTab, m: &Model, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let (n, k) = (m.points.len(), m.series.len().max(1));
    let per = k + usize::from(k > 1);
    let widest = m.points.iter().take(400).enumerate().map(|(i, _)| width(&point_label(cx, m, i))).max().unwrap_or(1);
    let lw = widest.clamp(3, (area.width as usize / 3).clamp(3, 28)) as u16;
    // Room on the right for the values after the bars.
    let vw = m.series.iter().flat_map(|s| s.values.iter().flatten()).take(2000).map(|v| width(&scale::short(*v))).max();
    let vw = vw.unwrap_or(0).min(8) as u16 + 1;
    let plot = Rect {
        x: area.x + lw + 2,
        y: area.y,
        width: area.width.saturating_sub(lw + 3 + vw),
        height: area.height.saturating_sub(2),
    };
    if plot.width < 6 || plot.height < 1 {
        return message(cx, &[cx.i18n.label(Label::ChartTooSmall).to_string()], area, buf);
    }
    c.plot = plot;
    let max_ticks = (plot.width as usize / 8).clamp(2, 10);
    let ticks = value_ticks(m, c.spec.log, true, max_ticks);
    let fits = ((plot.height as usize + usize::from(k > 1)) / per).max(1).min(n);
    scroll_to(c, fits, n);
    let line = Style::new().fg(th.border).bg(th.bg);
    for y in plot.y..plot.y + plot.height {
        put(buf, plot.x - 1, y, "│", 1, line);
    }
    let cursor_bg = th.cursor_line.bg.filter(|_| cx.focused);
    let muted = Style::new().fg(th.fg_muted).bg(th.bg);
    for i in c.offset..(c.offset + fits).min(n) {
        work(1);
        let gy = plot.y + ((i - c.offset) * per) as u16;
        let rows = (k as u16).min(plot.y + plot.height - gy);
        if i == c.cursor
            && let Some(bg) = cursor_bg
        {
            buf.set_style(Rect::new(plot.x, gy, plot.width, rows), Style::new().bg(bg));
        }
        let label = fit(&point_label(cx, m, i), lw as usize, Align::Right);
        let style = if i == c.cursor { label_style(cx) } else { muted };
        put(buf, area.x + 1, gy, &label, lw as usize, style);
        for (s, series) in m.series.iter().enumerate().take(rows as usize) {
            let y = gy + s as u16;
            let Some(v) = series.values[i] else { continue };
            let Some((base, at)) = bar_cells(&ticks, v, plot.width) else { continue };
            let style = Style::new().fg(series_color(th, m, s));
            for col in 0..plot.width {
                let (lo, hi) = (i64::from(col) * 8, i64::from(col) * 8 + 8);
                let glyph = if at >= base {
                    RIGHT[(hi.min(at) - lo.max(base)).clamp(0, 8) as usize]
                } else {
                    match hi.min(base) - lo.max(at) {
                        8.. => "█",
                        4..=7 => "▐",
                        1..=3 => "▕",
                        _ => " ",
                    }
                };
                if glyph != " " {
                    buf[(plot.x + col, y)].set_symbol(glyph).set_style(style);
                }
            }
            // The value after the bar's end, when it fits.
            let end = plot.x + (at.max(base) as u16).div_ceil(8);
            let text = scale::short(v);
            let tw = width(&text) as u16;
            if v >= 0.0 && end + 1 + tw <= plot.x + plot.width + vw {
                put(buf, end + 1, y, &text, tw as usize, Style::new().fg(th.fg_muted));
            }
        }
        c.hits.push((Rect::new(area.x, gy, area.width, rows), i));
    }
    // The value axis under the bars.
    let ay = plot.y + plot.height;
    put(buf, plot.x - 1, ay, "└", 1, line);
    put(buf, plot.x, ay, &"─".repeat(plot.width as usize), plot.width as usize, line);
    let labels = ticks.labels();
    let mut free = plot.x;
    for (v, label) in ticks.values.iter().zip(&labels) {
        let Some(f) = ticks.at(*v) else { continue };
        let col = plot.x + (f * f64::from(plot.width - 1)).round() as u16;
        put(buf, col, ay, "┬", 1, line);
        let w = width(label) as u16;
        let x = col.saturating_sub(w / 2).max(free).min((plot.x + plot.width).saturating_sub(w));
        if x >= free && x + w <= area.x + area.width {
            put(buf, x, ay + 1, label, w as usize, muted);
            free = x + w + 1;
        }
    }
    // Bars left out above or below.
    let style = Style::new().fg(th.accent).bg(th.bg).add_modifier(Modifier::BOLD);
    if c.offset > 0 {
        put(buf, area.x, plot.y, "▲", 1, style);
    }
    if c.offset + fits < n {
        put(buf, area.x, plot.y + plot.height - 1, "▼", 1, style);
    }
}

/// The braille bit of dot (`dx`, `dy`) of a cell (2 wide, 4 high).
fn dot_bit(dx: usize, dy: usize) -> u8 {
    const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    BITS[dx][dy]
}

/// Where point `i` of `m` is along the X axis, from 0 to 1.
fn x_frac(m: &Model, i: usize, span: (f64, f64)) -> f64 {
    match m.axis {
        Axis::Number | Axis::Time(_) if span.1 > span.0 => (m.points[i].x - span.0) / (span.1 - span.0),
        Axis::Number | Axis::Time(_) => 0.5,
        _ if m.points.len() > 1 => i as f64 / (m.points.len() - 1) as f64,
        _ => 0.5,
    }
}

/// A dot column of a series: its first, last, lowest and highest dot, and whether the line is
/// broken before it.
type Column = (usize, usize, usize, usize, bool);

/// The lines of `m` in braille dots in a `w`×`h` plot.
fn rasterize(m: &Model, t: &scale::Ticks, w: u16, h: u16) -> Raster {
    let (dw, dh) = (usize::from(w) * 2, usize::from(h) * 4);
    let mut cells = vec![(0u8, 0u8); usize::from(w) * usize::from(h)];
    let span = m.x_range();
    let px: Vec<usize> = (0..m.points.len()).map(|i| (x_frac(m, i, span) * (dw - 1) as f64).round() as usize).collect();
    work(m.points.len() as u64);
    let mut set = |x: usize, y: usize, s: usize| {
        let cell = &mut cells[(y / 4) * usize::from(w) + x / 2];
        cell.0 |= dot_bit(x % 2, y % 4);
        cell.1 = s as u8 + 1;
    };
    for (s, series) in m.series.iter().enumerate() {
        // Per dot column (a point without a value breaks the line).
        let mut cols: Vec<Option<Column>> = vec![None; dw];
        let mut broken = true;
        for (i, v) in series.values.iter().enumerate() {
            let Some(f) = v.and_then(|v| t.at(v)) else {
                // Split by a column, a series has no row at the other series' points: its line
                // goes on past them.
                broken |= !m.split || series.rows[i] > 0;
                continue;
            };
            let y = ((1.0 - f.clamp(0.0, 1.0)) * (dh - 1) as f64).round() as usize;
            let c = &mut cols[px[i]];
            *c = Some(match *c {
                None => (y, y, y, y, broken),
                Some((first, _, lo, hi, b)) => (first, y, lo.min(y), hi.max(y), b),
            });
            broken = false;
        }
        work(series.values.len() as u64 + dw as u64);
        let mut prev: Option<(usize, usize)> = None;
        for (x, c) in cols.iter().enumerate() {
            let Some((first, last, lo, hi, broken)) = *c else { continue };
            if let (Some((px, py)), false) = (prev, broken) {
                // Bresenham from the last dot of the column before.
                let (mut x0, mut y0) = (px as isize, py as isize);
                let (x1, y1) = (x as isize, first as isize);
                let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
                let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
                let mut err = dx + dy;
                loop {
                    set(x0 as usize, y0 as usize, s);
                    if x0 == x1 && y0 == y1 {
                        break;
                    }
                    let e2 = 2 * err;
                    if e2 >= dy {
                        err += dy;
                        x0 += sx;
                    }
                    if e2 <= dx {
                        err += dx;
                        y0 += sy;
                    }
                }
            }
            for y in lo..=hi {
                set(x, y, s);
            }
            prev = Some((x, last));
        }
    }
    Raster { key: (0, 0, w, h, false), cells, cols: px.iter().map(|&x| (x / 2) as u16).collect() }
}

/// Lines in braille dots, the X axis' ticks under them, a guide at the cursor.
fn line(cx: &Look, c: &mut ChartTab, m: &Model, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let plot_h = area.height.saturating_sub(2);
    let ticks = value_ticks(m, c.spec.log, false, (plot_h as usize / 2).clamp(2, 8));
    let labels = ticks.labels();
    let gutter = labels.iter().map(|l| width(l)).max().unwrap_or(1).min(12) as u16;
    let plot = Rect { x: area.x + gutter + 2, y: area.y, width: area.width.saturating_sub(gutter + 3), height: plot_h };
    if plot.width < 4 || plot.height < 2 {
        return message(cx, &[cx.i18n.label(Label::ChartTooSmall).to_string()], area, buf);
    }
    c.plot = plot;
    y_axis(cx, &ticks, &labels, gutter, plot, false, buf);
    let key = (m.points.len() as u64, m.series.len(), plot.width, plot.height, c.spec.log);
    if c.raster.as_ref().is_none_or(|r| r.key != key) {
        let mut r = rasterize(m, &ticks, plot.width, plot.height);
        r.key = key;
        c.raster = Some(r);
    }
    let Some(r) = c.raster.as_ref() else { return };
    work(r.cells.len() as u64);
    let cursor_col = r.cols.get(c.cursor).copied();
    for (i, &(bits, s)) in r.cells.iter().enumerate() {
        let (x, y) = (plot.x + (i % usize::from(plot.width)) as u16, plot.y + (i / usize::from(plot.width)) as u16);
        if bits != 0 {
            let ch = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
            let color = series_color(th, m, usize::from(s).saturating_sub(1));
            buf[(x, y)].set_char(ch).set_style(Style::new().fg(color));
        } else if Some(x - plot.x) == cursor_col && cx.focused {
            buf[(x, y)].set_symbol("┊").set_style(Style::new().fg(th.fg_dim));
        }
    }
    // The cursor's point of the selected series.
    if let (Some(col), Some(v)) =
        (cursor_col, m.series.get(c.series).and_then(|s| s.values.get(c.cursor)).copied().flatten())
        && let Some(f) = ticks.at(v)
    {
        let row = ((1.0 - f.clamp(0.0, 1.0)) * f64::from(plot.height * 4 - 1)).round() as u16 / 4;
        let cell = &mut buf[(plot.x + col, plot.y + row)];
        if cell.symbol() == " " || cell.symbol() == "┊" {
            cell.set_symbol("●");
        }
        cell.set_style(Style::new().fg(series_color(th, m, c.series)).add_modifier(Modifier::BOLD).patch(th.selection));
    }
    // The X axis.
    let line = Style::new().fg(th.border).bg(th.bg);
    let ay = plot.y + plot.height;
    put(buf, plot.x - 1, ay, "└", 1, line);
    put(buf, plot.x, ay, &"─".repeat(plot.width as usize), plot.width as usize, line);
    let muted = Style::new().fg(th.fg_muted).bg(th.bg);
    let mut free = area.x;
    for (col, label) in x_ticks(cx, m, plot.width) {
        let at = plot.x + col;
        put(buf, at, ay, "┬", 1, line);
        let w = width(&label) as u16;
        let x =
            at.saturating_sub(w / 2).max(plot.x.saturating_sub(gutter)).min((plot.x + plot.width).saturating_sub(w));
        if x >= free && x + w <= area.x + area.width {
            put(buf, x, ay + 1, &label, w as usize, muted);
            free = x + w + 2;
        }
    }
    for (i, &col) in r.cols.iter().enumerate().filter(|(i, _)| *i == c.cursor) {
        c.hits.push((Rect::new(plot.x + col, plot.y, 1, plot.height), i));
    }
}

/// The ticks of a line chart's X axis: their column in a plot `w` wide, and their labels.
fn x_ticks(cx: &Look, m: &Model, w: u16) -> Vec<(u16, String)> {
    let span = m.x_range();
    let col = |f: f64| (f.clamp(0.0, 1.0) * f64::from(w - 1)).round() as u16;
    match m.axis {
        Axis::Time(kind) => {
            let est = match kind {
                TimeKind::Date => 10,
                TimeKind::DateTime => 11,
                TimeKind::Time => 8,
            };
            let max = (usize::from(w) / (est + 2)).clamp(2, 10);
            scale::times(span.0, span.1, max, kind)
                .into_iter()
                .map(|(v, l)| (col((v - span.0) / (span.1 - span.0).max(1e-9)), l))
                .collect()
        }
        Axis::Number => {
            let t = scale::linear(span.0, span.1, (usize::from(w) / 8).clamp(2, 10), false);
            let labels = t.labels();
            t.values
                .iter()
                .zip(labels)
                .filter(|(v, _)| **v >= span.0 && **v <= span.1)
                .map(|(v, l)| (col((v - span.0) / (span.1 - span.0).max(1e-9)), l))
                .collect()
        }
        Axis::Category | Axis::Row => {
            let n = m.points.len();
            let widest =
                m.points.iter().take(200).enumerate().map(|(i, _)| width(&point_label(cx, m, i))).max().unwrap_or(1);
            let lw = widest.min(14);
            let every = (n * (lw + 2)).div_ceil(usize::from(w).max(1)).max(1);
            (0..n).step_by(every).map(|i| (col(x_frac(m, i, span)), clip(&point_label(cx, m, i), lw))).collect()
        }
    }
}

/// The series' colors and names.
fn draw_legend(cx: &Look, c: &ChartTab, m: &Model, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let mut x = area.x + 1;
    let end = area.x + area.width;
    for (s, series) in m.series.iter().enumerate() {
        let name = if series.others {
            cx.i18n.msg(&Msg::ChartOthersSeries { count: m.other_series as u64 }).to_string()
        } else {
            sanitize_cell(&series.name)
        };
        if x + 3 >= end {
            break;
        }
        put(buf, x, area.y, "■", 1, Style::new().fg(series_color(th, m, s)).bg(th.bg));
        x += 2;
        let mut style = Style::new().fg(th.fg_muted).bg(th.bg);
        if s == c.series {
            style = Style::new().fg(th.fg).bg(th.bg).add_modifier(Modifier::BOLD);
        }
        x += put(buf, x, area.y, &name, (end - x) as usize, style) + 2;
    }
}

/// The values at the cursor: the point's X, each series' value (the selected one marked),
/// and the row it comes from (or how many rows it sums).
fn readout(cx: &Look, c: &ChartTab, m: &Model, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    let Some(p) = m.points.get(c.cursor) else { return };
    let end = area.x + area.width;
    let mut x = area.x + 1;
    let mut text = |x: &mut u16, s: &str, style: Style| {
        if *x < end {
            *x += put(buf, *x, area.y, s, (end - *x) as usize, style);
        }
    };
    let bold = Style::new().fg(th.accent).bg(th.bg).add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(th.fg_muted).bg(th.bg);
    text(&mut x, "▸ ", bold);
    text(&mut x, &point_label(cx, m, c.cursor), bold);
    // A value of one row as the row has it (no binary rounding), when that row was read.
    let cell = |s: usize| -> Option<String> {
        let series = m.series.get(s)?;
        if p.others || series.rows.get(c.cursor) != Some(&1) {
            return None;
        }
        let (_, at, cells) = c.row.as_ref()?;
        if series.first.get(c.cursor).copied().flatten() != Some(*at) {
            return None;
        }
        let col = if c.spec.by.is_some() { *c.spec.ys.first()? } else { *c.spec.ys.get(s)? };
        cells.get(col)?.clone()
    };
    for (s, series) in m.series.iter().enumerate() {
        text(&mut x, " · ", muted);
        let name =
            if series.others { cx.i18n.label(Label::ChartOthers).to_string() } else { sanitize_cell(&series.name) };
        let value = match series.values[c.cursor] {
            Some(v) => cell(s).unwrap_or_else(|| scale::plain(v)),
            None => "–".into(),
        };
        let (ns, vs) = if s == c.series && m.series.len() > 1 {
            (
                Style::new().fg(series_color(th, m, s)).bg(th.bg).add_modifier(Modifier::BOLD),
                Style::new().fg(th.fg).bg(th.bg).add_modifier(Modifier::BOLD),
            )
        } else {
            (Style::new().fg(series_color(th, m, s)).bg(th.bg), Style::new().fg(th.fg).bg(th.bg))
        };
        text(&mut x, &format!("{name} "), ns);
        text(&mut x, &value, vs);
    }
    // The rows of the selected series' value (of the point when it has none).
    let summed = m.series.get(c.series).and_then(|s| s.rows.get(c.cursor)).copied().filter(|n| *n > 0);
    let count = summed.map_or(p.rows as u64, u64::from);
    let row = crate::app::chart::ChartTab::row_of(m, c.cursor, c.series).map_or(0, |r| r + 1).to_string();
    let rows: Localized = if p.others {
        cx.i18n.msg(&Msg::ChartReadoutOthers { count: m.other_points as u64 })
    } else if count > 1 {
        cx.i18n.msg(&Msg::ChartReadoutRows { count, row })
    } else {
        cx.i18n.msg(&Msg::ChartReadoutRow { row })
    };
    text(&mut x, " · ", muted);
    text(&mut x, &rows, muted);
}

#[cfg(test)]
mod tests;
