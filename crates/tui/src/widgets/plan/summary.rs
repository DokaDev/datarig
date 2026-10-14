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
struct Card {
    title: String,
    lines: Vec<(String, Style)>,
    node: Option<usize>,
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
    if let Some(i) = slowest {
        let style = if plan.is_hot(i) { base.patch(th.plan_hot) } else { base.add_modifier(Modifier::BOLD) };
        lines.push((plan.label(i), style));
        let (share, own) = (fmt_share(plan.share(i)), fmt_weight(plan, plan.weight(i)));
        lines.push((i18n.msg(&Msg::PlanCardShare { share, own }).to_string(), muted));
    }
    out.push(Card { title, lines, node: slowest });
    // The worst row estimate.
    let title = i18n.label(Label::PlanCardMisestimate).to_string();
    let worst = plan.worst_misestimate();
    let lines = match worst {
        _ if !plan.analyzed => vec![(i18n.label(Label::PlanCardNeedsAnalyze).to_string(), muted)],
        None => vec![(i18n.label(Label::PlanCardNoMisestimate).to_string(), muted)],
        Some((i, off)) => {
            let n = &plan.nodes[i];
            let (ratio, est) = (fmt_num(off.ratio), n.plan_rows.map(fmt_num).unwrap_or_default());
            let actual = n.actual.map(|a| fmt_num(a.rows)).unwrap_or_default();
            let msg = if off.under {
                Msg::PlanCardUnder { ratio, est, actual }
            } else {
                Msg::PlanCardOver { ratio, est, actual }
            };
            vec![
                (plan.label(i), base.add_modifier(Modifier::BOLD)),
                (i18n.msg(&msg).to_string(), base.patch(th.plan_misestimate)),
            ]
        }
    };
    out.push(Card { title, lines, node: worst.map(|w| w.0) });
    // Disk reads.
    let title = i18n.label(Label::PlanCardReads).to_string();
    let lines = match plan.buffers() {
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
    out.push(Card { title, lines, node: None });
    // The whole.
    let lines = if time {
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
        let total = fmt_num(plan.total());
        vec![(i18n.msg(&Msg::PlanCardCost { total }).to_string(), Style::new().fg(th.warning).bg(th.bg))]
    };
    let title = i18n.label(if time { Label::PlanCardTime } else { Label::PlanCardCostTotal }).to_string();
    out.push(Card { title, lines, node: None });
    out
}

pub(super) fn draw(cx: &Look, p: &mut PlanTab, area: Rect, buf: &mut Buffer) {
    let cards = cards(cx, p);
    work(cards.len() as u64);
    // Four in a row when they fit, else two by two; none in a pane too short for them.
    let per_row = if area.width >= 4 * 30 { 4 } else { 2 };
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
fn card(cx: &Look, c: &Card, r: Rect, buf: &mut Buffer) {
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
    for (t, style) in &c.lines {
        for l in wrap_words(t, inner_w.saturating_sub(1)) {
            lines.push((l, *style));
        }
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
