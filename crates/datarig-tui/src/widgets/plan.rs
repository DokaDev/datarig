//! The Plan tab of the results pane: a line naming the views (the shown one in brackets) with
//! what the plan's numbers are (measured times, or estimated costs), the view, and the selected
//! node's detail next to it or below it.
//!
//! Every view draws only what is on screen and walks each node a bounded number of times per
//! frame ([`work`] counts it), so a plan of hundreds of nodes stays fast. Names too long for
//! their place are cut with `…`; markers (hot, misestimate) are kept.

mod detail;
mod raw;
mod tree;

use crate::app::plan::{PlanTab, PlanView};
use crate::app::{App, Focus};
use crate::text::{clip, width};
use crate::theme::{self, Theme};
use crate::widgets::put;
use datarig_core::i18n::{I18n, Label, Msg};
use datarig_core::sql::plan::{HOT, Measure, Plan};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use std::cell::Cell;

thread_local! {
    static WORK: Cell<u64> = const { Cell::new(0) };
}

/// Nodes the plan views walked on this thread since the last [`take_work`] (the benchmark's
/// work count per frame).
pub fn take_work() -> u64 {
    WORK.with(|w| w.replace(0))
}

/// One node walked.
fn work(n: u64) {
    WORK.with(|w| w.set(w.get() + n));
}

/// What a view draws with.
pub(crate) struct Look<'a> {
    pub i18n: &'a I18n,
    pub th: &'a Theme,
    pub icons: bool,
    pub focused: bool,
}

/// The active tab's plan, in `area` (inside the results pane, below its strip).
pub(crate) fn draw_plan(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    buf.set_style(area, th.base());
    let focused = matches!(app.focus, Focus::Results | Focus::Inspector);
    let icons = app.icons_on();
    // Taken out while it is drawn (it keeps what the frame drew: hits, scroll).
    let Some(mut p) = app.tabs.active_mut().exec.plan.take() else { return };
    let cx = Look { i18n: &app.i18n, th: &th, icons, focused };
    p.hits.clear();
    p.view_hits.clear();
    if area.height > 0 {
        view_bar(&cx, &mut p, Rect { height: 1, ..area }, buf);
    }
    let body = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
    let (view, side) = split_detail(&p, body);
    if view.height > 0 && view.width > 0 {
        match p.view {
            PlanView::Tree => tree::draw(&cx, &mut p, view, buf),
            PlanView::Raw => raw::draw(&cx, &mut p, view, buf),
        }
    }
    if let Some((side, beside)) = side {
        detail::draw(&cx, &p, side, beside, buf);
    }
    app.tabs.active_mut().exec.plan = Some(p);
}

/// Where the view and the detail go: the detail on the right when the pane is wide (`true`),
/// else below; not at all when the pane is too small for both.
fn split_detail(p: &PlanTab, body: Rect) -> (Rect, Option<(Rect, bool)>) {
    if !p.detail || body.height < 3 {
        return (body, None);
    }
    if body.width >= 100 {
        let w = (body.width * 2 / 5).clamp(36, 64);
        let view = Rect { width: body.width - w, ..body };
        (view, Some((Rect { x: body.x + body.width - w, width: w, ..body }, true)))
    } else if body.height >= 10 {
        let h = body.height / 2;
        let view = Rect { height: body.height - h, ..body };
        (view, Some((Rect { y: body.y + body.height - h, height: h, ..body }, false)))
    } else {
        (body, None)
    }
}

/// The views' names (the shown one in brackets, each where the pointer picks it), and on the
/// right what the plan's numbers are.
fn view_bar(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    buf.set_style(area, Style::new().bg(th.surface));
    let plan = p.plan.clone();
    let (note, note_style) = match plan.measure() {
        Measure::Cost => {
            (cx.i18n.label(Label::PlanEstimatesOnly).to_string(), Style::new().fg(th.warning).bg(th.surface))
        }
        Measure::Time => {
            let execution = plan.execution_ms.map_or_else(|| "?".into(), fmt_ms);
            let planning = plan.planning_ms.map_or_else(|| "?".into(), fmt_ms);
            (
                cx.i18n.msg(&Msg::PlanTimes { execution, planning }).to_string(),
                Style::new().fg(th.fg_muted).bg(th.surface),
            )
        }
    };
    let more =
        (plan.more_plans > 0).then(|| cx.i18n.msg(&Msg::PlanMorePlans { count: plan.more_plans as u64 }).to_string());
    let note = match more {
        Some(m) => format!("{note} · {m}"),
        None => note,
    };
    let end = area.x + area.width;
    let mut x = area.x + 1;
    let names: Vec<(PlanView, String)> =
        PlanView::ALL.iter().map(|v| (*v, cx.i18n.label(v.label()).to_string())).collect();
    for (v, name) in names {
        let on = v == p.view;
        let text = if on { format!("[{name}]") } else { format!(" {name} ") };
        let w = width(&text) as u16;
        if x + w > end {
            break;
        }
        let style = if on {
            Style::new().fg(th.accent).bg(th.surface).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(th.fg_muted).bg(th.surface)
        };
        put(buf, x, area.y, &text, (end - x) as usize, style);
        p.view_hits.push((Rect { x, y: area.y, width: w, height: 1 }, v));
        x += w + 1;
    }
    // The note on the right, when there is room after the names.
    let room = end.saturating_sub(x + 2) as usize;
    if room >= 8 {
        let t = clip(&note, room);
        let at = end - 1 - width(&t) as u16;
        put(buf, at, area.y, &t, room, note_style);
    }
}

/// The color of a share of the whole: hot, warm (a twentieth or more) or cool.
pub(crate) fn heat(th: &Theme, share: f64) -> Style {
    if share >= HOT {
        Style::new().patch(th.plan_hot)
    } else if share >= 0.05 {
        Style::new().fg(th.accent_warm)
    } else {
        Style::new().fg(th.accent)
    }
}

/// A bar of `w` columns filled to `share` (eighths of a column), at least a sliver when the
/// share is not zero.
pub(crate) fn bar(share: f64, w: usize) -> String {
    const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = ((share.clamp(0.0, 1.0) * (w * 8) as f64).round() as usize).max(usize::from(share > 0.0));
    let full = (eighths / 8).min(w);
    let mut s = "█".repeat(full);
    if full < w {
        s.push_str(PARTS[eighths % 8]);
    }
    let pad = w.saturating_sub(width(&s));
    s.push_str(&" ".repeat(pad));
    s
}

/// A time: `0.123 ms`, `12.3 ms`, `1.23 s`.
pub(crate) fn fmt_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 10.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{ms:.3} ms")
    }
}

/// A count in a few columns: `999`, `61.9k`, `1.2M`, `3.4G`; a fraction below ten keeps a
/// decimal (`0.5`).
pub(crate) fn fmt_num(n: f64) -> String {
    let a = n.abs();
    if a >= 1e9 {
        format!("{:.1}G", n / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", n / 1e6)
    } else if a >= 1e4 {
        format!("{:.1}k", n / 1e3)
    } else if a >= 10.0 || n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        // `0.04` is `0`, not `0.0`.
        let s = format!("{n:.1}");
        s.strip_suffix(".0").map_or(s.clone(), str::to_string)
    }
}

/// A share as a whole percentage, rounded down so that a node is never shown at the hot
/// threshold without being hot: `41%`, `19%` (for 19.6), `<1%`, `0%`.
pub(crate) fn fmt_share(share: f64) -> String {
    match share {
        s if s <= 0.0 => "0%".into(),
        s if s < 0.01 => "<1%".into(),
        s => format!("{}%", (s * 100.0 + 1e-9).floor() as u64),
    }
}

/// A node's own weight as its column says it: a time, or a cost.
pub(crate) fn fmt_weight(plan: &Plan, w: f64) -> String {
    match plan.measure() {
        Measure::Time => fmt_ms(w),
        Measure::Cost => fmt_num(w),
    }
}

/// The node's markers: hot (a glyph, else a word) and a misestimate with its factor after the
/// warning mark (`!×12↑` with icons off: more rows than estimated, `↓`: fewer).
pub(crate) fn markers(cx: &Look, plan: &Plan, i: usize) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    if plan.is_hot(i) {
        let text = if cx.icons { crate::icons::HOT.to_string() } else { cx.i18n.label(Label::PlanHotMark).to_string() };
        out.push((text, Style::new().patch(cx.th.plan_hot)));
    }
    if let Some(off) = plan.misestimate(i) {
        let mark = crate::icons::warning(cx.icons);
        let arrow = if off.under { "↑" } else { "↓" };
        out.push((format!("{mark}×{}{arrow}", fmt_num(off.ratio)), Style::new().patch(cx.th.plan_misestimate)));
    }
    out
}

/// `style` on a line that is `selected`: on the focused selection a color of its own may not
/// read (a light theme's selection), so only its modifier stays (hot stays bold, a misestimate
/// underlined); elsewhere `style` as it is.
pub(crate) fn on_line(cx: &Look, style: Style, selected: bool) -> Style {
    if selected && cx.focused { Style::new().add_modifier(style.add_modifier) } else { style }
}

/// The style of a selected line: the selection when the pane has the focus, else the cursor
/// line.
pub(crate) fn selected_style(cx: &Look) -> Style {
    if cx.focused { cx.th.selection } else { cx.th.cursor_line }
}

/// Keep line `at` (of `len`) on screen in `h` lines from `scroll`, unless the wheel moved the
/// view away; never past the end.
pub(crate) fn follow(p: &mut PlanTab, at: usize, len: usize, h: usize) -> usize {
    let h = h.max(1);
    p.page = h;
    let max = len.saturating_sub(h);
    if !p.detached {
        if at < p.scroll {
            p.scroll = at;
        } else if at >= p.scroll + h {
            p.scroll = at + 1 - h;
        }
    }
    p.scroll = p.scroll.min(max);
    p.scroll
}

#[cfg(test)]
mod tests;
