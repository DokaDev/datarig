//! The summary: cards for what to look at first — the slowest node and its share, the worst
//! row estimate, disk reads and the cache hit ratio, planning and execution time (by cost: the
//! costliest node and the total estimated cost) — above a compact tree. A card that names a
//! node selects it when clicked.

use super::{Look, fmt_ms, fmt_num, fmt_share, fmt_weight, tree, work};
use crate::app::plan::PlanTab;
use crate::text::wrap_words;
use crate::widgets::put;
use datarig_core::i18n::{Label, Msg};
use datarig_core::sql::plan::Measure;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// A card: its title, its lines, and the node it names.
pub(super) struct Card {
    pub(super) title: String,
    /// The node it names, on one line (cut to fit): its numbers below it stay in the card.
    pub(super) name: Option<(String, Style)>,
    pub(super) lines: Vec<(String, Style)>,
    /// The numbers in one short line, shown instead of `lines` when they do not fit the card.
    pub(super) short: Option<(String, Style)>,
    pub(super) node: Option<usize>,
}

fn cards(cx: &Look, p: &PlanTab) -> Vec<Card> {
    let th = cx.th;
    let plan = &p.plan;
    let i18n = cx.i18n;
    let base = th.base();
    let muted = Style::new().fg(th.fg_muted).bg(th.bg);
    let time = plan.measure() == Measure::Time;
    let mut out = Vec::new();
    // The node that takes the most.
    let slowest = plan.slowest();
    let title = i18n.label(if time { Label::PlanCardSlowest } else { Label::PlanCardCostliest }).to_string();
    let mut lines = Vec::new();
    let mut name = None;
    let mut short = None;
    if let Some(i) = slowest {
        let style = if plan.is_hot(i) { base.patch(th.plan_hot) } else { base.add_modifier(Modifier::BOLD) };
        name = Some((plan.label(i), style));
        let (share, own) = (fmt_share(plan.share(i)), fmt_weight(plan, i));
        lines.push((i18n.msg(&Msg::PlanCardShare { share: share.clone(), own: own.clone() }).to_string(), muted));
        short = Some((i18n.msg(&Msg::PlanCardShareShort { share, own }).to_string(), muted));
    }
    out.push(Card { title, name, lines, short, node: slowest });
    // The worst row estimate.
    let title = i18n.label(Label::PlanCardMisestimate).to_string();
    let worst = plan.worst_misestimate();
    let (name, lines, short) = match worst {
        _ if !plan.analyzed => (None, vec![(i18n.label(Label::PlanCardNeedsAnalyze).to_string(), muted)], None),
        None => (None, vec![(i18n.label(Label::PlanCardNoMisestimate).to_string(), muted)], None),
        Some((i, off)) => {
            let n = &plan.nodes[i];
            let (ratio, est) = (fmt_num(off.ratio), n.plan_rows.map(fmt_num).unwrap_or_default());
            let actual = n.actual.map(|a| fmt_num(a.rows)).unwrap_or_default();
            let (msg, brief) = if off.under {
                (
                    Msg::PlanCardUnder { ratio: ratio.clone(), est: est.clone(), actual: actual.clone() },
                    Msg::PlanCardUnderShort { ratio, est, actual },
                )
            } else {
                (
                    Msg::PlanCardOver { ratio: ratio.clone(), est: est.clone(), actual: actual.clone() },
                    Msg::PlanCardOverShort { ratio, est, actual },
                )
            };
            let style = base.patch(th.plan_misestimate);
            (
                Some((plan.label(i), base.add_modifier(Modifier::BOLD))),
                vec![(i18n.msg(&msg).to_string(), style)],
                Some((i18n.msg(&brief).to_string(), style)),
            )
        }
    };
    out.push(Card { title, name, lines, short, node: worst.map(|w| w.0) });
    // Disk reads (a format that never counts them has no card for them).
    let title = i18n.label(Label::PlanCardReads).to_string();
    let lines = match plan.buffers() {
        None if !p.format.has_buffers() => Vec::new(),
        Some(b) => {
            let read = fmt_num(b.reads() as f64);
            let hit = b.hit_ratio().map_or_else(|| "-".into(), fmt_share);
            let mut v = vec![(i18n.msg(&Msg::PlanCardReadsLine { read, hit }).to_string(), base)];
            if b.temp_written > 0 || b.temp_read > 0 {
                let (written, read) = (fmt_num(b.temp_written as f64), fmt_num(b.temp_read as f64));
                v.push((
                    i18n.msg(&Msg::PlanCardTemp { written, read }).to_string(),
                    Style::new().fg(th.warning).bg(th.bg),
                ));
            }
            v
        }
        None => vec![(i18n.label(Label::PlanCardNoBuffers).to_string(), muted)],
    };
    if !lines.is_empty() {
        out.push(Card { title, name: None, lines, short: None, node: None });
    }
    // The whole.
    let measured_only = time && !p.format.reports_statement_times();
    let lines = if measured_only {
        // No statement times from the server: the roots' measured total.
        vec![(i18n.msg(&Msg::PlanMeasured { total: fmt_ms(plan.total()) }).to_string(), base)]
    } else if time {
        let planning = plan.planning_ms.map_or_else(|| "?".into(), fmt_ms);
        let execution = plan.execution_ms.map_or_else(|| "?".into(), fmt_ms);
        let mut v = vec![(i18n.msg(&Msg::PlanCardTimes { planning, execution }).to_string(), base)];
        let triggers: f64 = plan.triggers.iter().map(|t| t.ms).sum();
        if !plan.triggers.is_empty() {
            let time = fmt_ms(triggers);
            v.push((i18n.msg(&Msg::PlanCardTriggers { time }).to_string(), muted));
        }
        v
    } else {
        // The roots' cost, marked when one of them is not the server's.
        let total = match plan.roots().any(|r| plan.nodes[r].cost_derived) {
            true => format!("≈{}", fmt_num(plan.total())),
            false => fmt_num(plan.total()),
        };
        vec![(i18n.msg(&Msg::PlanCardCost { total }).to_string(), Style::new().fg(th.warning).bg(th.bg))]
    };
    let title = match (time, measured_only) {
        (true, true) => Label::PlanCardMeasured,
        (true, false) => Label::PlanCardTime,
        (false, _) => Label::PlanCardCostTotal,
    };
    let title = i18n.label(title).to_string();
    out.push(Card { title, name: None, lines, short: None, node: None });
    out
}

pub(super) fn draw(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    let cards = cards(cx, p);
    work(cards.len() as u64);
    // Four in a row when they fit, else two by two; none in a pane too short for them.
    let per_row = if area.width >= 4 * 30 { cards.len().clamp(1, 4) } else { 2 };
    let rows = cards.len().div_ceil(per_row) as u16;
    let card_h = 4;
    let cards_h = rows * card_h;
    let show = area.height >= cards_h + 4;
    let tree_area = if show { Rect { y: area.y + cards_h, height: area.height - cards_h, ..area } } else { area };
    if show {
        let w = area.width / per_row as u16;
        for (k, c) in cards.iter().enumerate() {
            let (col, row) = ((k % per_row) as u16, (k / per_row) as u16);
            let r = Rect { x: area.x + col * w, y: area.y + row * card_h, width: w, height: card_h };
            card(cx, c, r, buf);
            if let Some(i) = c.node {
                p.hits.push((r, i));
            }
        }
    }
    tree::draw(cx, p, tree_area, true, buf);
}

/// One card in `r`: a frame with its title, and its lines inside.
pub(super) fn card(cx: &Look, c: &Card, r: Rect, buf: &mut Buffer) {
    let th = cx.th;
    if r.width < 6 {
        return;
    }
    let frame = Style::new().fg(th.border).bg(th.bg);
    let inner_w = (r.width - 3) as usize;
    let top = format!("╭ {} {}╮", c.title, "─".repeat(inner_w.saturating_sub(crate::text::width(&c.title) + 2)));
    put(buf, r.x, r.y, &top, r.width as usize - 1, frame);
    let title_style = Style::new().fg(th.fg_muted).bg(th.bg).add_modifier(Modifier::BOLD);
    put(buf, r.x + 2, r.y, &c.title, inner_w.saturating_sub(1), title_style);
    let mut y = r.y + 1;
    let mut lines: Vec<(String, Style)> = Vec::new();
    if let Some((name, style)) = &c.name {
        lines.push((crate::text::clip(name, inner_w.saturating_sub(1)), *style));
    }
    for (t, style) in &c.lines {
        for l in wrap_words(t, inner_w.saturating_sub(1)) {
            lines.push((l, *style));
        }
    }
    // The numbers are what the card is for: in their short form when the full ones would be cut.
    let room = r.height.saturating_sub(2) as usize;
    if lines.len() > room
        && let Some((t, style)) = &c.short
    {
        // With room for one line only, that is the numbers' (the name is the node selected).
        lines.truncate(usize::from(c.name.is_some() && room > 1));
        lines.push((crate::text::clip(t, inner_w.saturating_sub(1)), *style));
    }
    for k in 0..(r.height.saturating_sub(2)) {
        buf.set_stringn(r.x, y, "│", 1, frame);
        buf.set_stringn(r.x + r.width - 2, y, "│", 1, frame);
        if let Some((t, style)) = lines.get(k as usize) {
            put(buf, r.x + 2, y, t, inner_w.saturating_sub(1), *style);
        }
        y += 1;
    }
    let bottom = format!("╰{}╯", "─".repeat((r.width as usize).saturating_sub(3)));
    put(buf, r.x, y, &bottom, r.width as usize - 1, frame);
}
