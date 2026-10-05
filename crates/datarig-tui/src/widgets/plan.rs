//! The Plan tab of the results pane: a line naming the views (the shown one in brackets) with
//! what the plan's numbers are (measured times, or estimated costs), the view, and the selected
//! node's detail next to it or below it.
//!
//! Every view draws only what is on screen and walks each node a bounded number of times per
//! frame ([`work`] counts it), so a plan of hundreds of nodes stays fast. Names too long for
//! their place are cut with `…`; markers (hot, misestimate) are kept.

mod boxes;
mod detail;
mod funnel;
mod icicle;
mod raw;
mod summary;
mod timeline;
mod tree;
mod treemap;

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
    draw_into(&cx, &mut p, area, buf);
    app.tabs.active_mut().exec.plan = Some(p);
}

/// The plan `p` in `area`: the views' line, the view and the detail.
pub(crate) fn draw_into(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    p.hits.clear();
    p.view_hits.clear();
    if area.height > 0 {
        view_bar(cx, p, Rect { height: 1, ..area }, buf);
    }
    let body = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
    let (view, side) = split_detail(p, body);
    if view.height > 0 && view.width > 0 {
        match p.view {
            PlanView::Tree => tree::draw(cx, p, view, false, buf),
            PlanView::Summary => summary::draw(cx, p, view, buf),
            PlanView::Icicle => icicle::draw(cx, p, view, false, buf),
            PlanView::Flame => icicle::draw(cx, p, view, true, buf),
            PlanView::Timeline => timeline::draw(cx, p, view, buf),
            PlanView::Rows => funnel::draw(cx, p, view, buf),
            PlanView::Treemap => treemap::draw(cx, p, view, buf),
            PlanView::Boxes => boxes::draw(cx, p, view, buf),
            PlanView::Raw => raw::draw(cx, p, view, buf),
        }
    }
    if let Some((side, beside)) = side {
        detail::draw(cx, p, side, beside, buf);
    }
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
    // The note on the right comes first (it says whether the numbers are times or costs); the
    // views' names take the rest: all of them, or the shown one and as many around it as fit,
    // `…` where some are left out (`v`, the digits, the menu and `:` reach them).
    let note_w = (width(&note) as u16).min(area.width.saturating_sub(2));
    let names: Vec<(PlanView, String)> = PlanView::ALL
        .iter()
        .map(|v| {
            let name = cx.i18n.label(v.label()).to_string();
            (*v, if *v == p.view { format!("[{name}]") } else { format!(" {name} ") })
        })
        .collect();
    let room = area.width.saturating_sub(note_w + 4) as usize;
    let cost = |shown: &[usize]| -> usize {
        let gaps = shown.windows(2).filter(|w| w[1] > w[0] + 1).count()
            + usize::from(shown.first().is_some_and(|f| *f > 0))
            + usize::from(shown.last().is_some_and(|l| *l + 1 < names.len()));
        shown.iter().map(|&k| width(&names[k].1) + 1).sum::<usize>() + 2 * gaps
    };
    let at = PlanView::ALL.iter().position(|v| *v == p.view).unwrap_or(0);
    let all: Vec<usize> = (0..names.len()).collect();
    let shown = if cost(&all) <= room {
        all
    } else {
        let mut shown = vec![at];
        let (mut left, mut right) = (at.checked_sub(1), (at + 1 < names.len()).then_some(at + 1));
        while left.is_some() || right.is_some() {
            for side in [&mut right, &mut left] {
                let Some(k) = *side else { continue };
                let mut more = shown.clone();
                more.push(k);
                more.sort_unstable();
                if cost(&more) > room {
                    *side = None;
                    continue;
                }
                shown = more;
                *side = if k > at { Some(k + 1).filter(|k| *k < names.len()) } else { k.checked_sub(1) };
            }
        }
        shown
    };
    let mut x = area.x + 1;
    let dim = Style::new().fg(th.fg_dim).bg(th.surface);
    let mut next = 0;
    for k in shown {
        if k > next && x + 2 <= end {
            put(buf, x, area.y, "…", 1, dim);
            x += 2;
        }
        next = k + 1;
        let (v, text) = &names[k];
        let w = width(text) as u16;
        if x + w > end {
            break;
        }
        let style = if *v == p.view {
            Style::new().fg(th.accent).bg(th.surface).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(th.fg_muted).bg(th.surface)
        };
        put(buf, x, area.y, text, (end - x) as usize, style);
        p.view_hits.push((Rect { x, y: area.y, width: w, height: 1 }, *v));
        x += w + 1;
    }
    if next < names.len() && x + 2 <= end {
        put(buf, x, area.y, "…", 1, dim);
        x += 2;
    }
    let room = end.saturating_sub(x + 1) as usize;
    if room >= 4 {
        let t = clip(&note, room);
        let at = end - 1 - width(&t) as u16;
        put(buf, at, area.y, &t, room, note_style);
    }
}

/// The selected node in one line, for the views whose boxes may be too small to name it: its
/// name, its own and total time (by cost: costs, said to be estimated) and its share.
pub(crate) fn info_line(cx: &Look, plan: &Plan, i: usize, area: Rect, buf: &mut Buffer) {
    let th = cx.th;
    buf.set_style(area, Style::new().bg(th.surface));
    let Some(n) = plan.nodes.get(i) else { return };
    let label = plan.label(i);
    let share = fmt_share(plan.share(i));
    let msg = match plan.measure() {
        Measure::Time => {
            let own = n.self_ms.map_or_else(String::new, fmt_ms);
            let total = n.total_ms.map_or_else(String::new, fmt_ms);
            Msg::PlanInfoTime { label, own, share, total }
        }
        Measure::Cost => {
            let own = n.self_cost.map_or_else(String::new, fmt_num);
            let total = n.cost.map_or_else(String::new, |c| fmt_num(c.1));
            Msg::PlanInfoCost { label, own, share, total }
        }
    };
    let mut x = area.x + 1;
    let end = area.x + area.width.saturating_sub(1);
    let style = Style::new().fg(th.fg).bg(th.surface);
    x += put(buf, x, area.y, &cx.i18n.msg(&msg), end.saturating_sub(x) as usize, style);
    for (t, s) in markers(cx, plan, i) {
        if x + 1 < end {
            x += 1 + put(buf, x + 1, area.y, &t, (end - x - 1) as usize, Style::new().bg(th.surface).patch(s));
        }
    }
}

/// Node `i` named on one line, after its subplan's name when it has one (`SubPlan 1: Aggregate`),
/// indented by its depth (at most a third of `w`), in `w` columns.
pub(crate) fn indented_name(plan: &Plan, i: usize, w: usize) -> String {
    let n = &plan.nodes[i];
    let indent = " ".repeat((n.depth * 2).min(w / 3));
    let name = match &n.subplan {
        Some(sub) => format!("{indent}{sub}: {}", plan.label(i)),
        None => format!("{indent}{}", plan.label(i)),
    };
    crate::text::fit(&name, w, crate::text::Align::Left)
}

/// The width of the names in the views that list nodes beside a band (timeline, row flow):
/// two fifths of the view, within limits.
pub(crate) fn label_column(w: u16) -> usize {
    (w as usize * 2 / 5).clamp(16, 48).min((w as usize).saturating_sub(12))
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
