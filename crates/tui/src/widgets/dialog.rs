//! Centered modal boxes and the connect-time password prompt.

use crate::app::action::Action;
use crate::app::overlay::{Buttons, Overlay, OverlayKind};
use crate::app::{App, PromptPurpose};
use crate::keymap::Ctx;
use crate::text::{clip, wrap_words};
use crate::theme;
use crate::widgets::popup::clear_overlay;
use crate::widgets::put;
use crate::widgets::statusbar::level_color;
use datarig_core::i18n::{Label, Localized, Msg};
use datarig_core::secret::SourceKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};
use std::time::Instant;

pub(crate) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// Clear `rect` and draw a rounded modal box; returns the inner area.
pub(crate) fn modal(rect: Rect, title: &Localized, footer: &Localized, buf: &mut Buffer) -> Rect {
    let th = theme::cur();
    clear_overlay(rect, buf);
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(th.accent).bg(th.surface))
        .style(Style::new().bg(th.surface).fg(th.fg))
        .title(Line::from(Span::styled(
            format!(" {} ", clip(title, (rect.width as usize).saturating_sub(6))),
            Style::new().fg(th.fg).add_modifier(Modifier::BOLD),
        )));
    if !footer.is_empty() {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" {} ", clip(footer, (rect.width as usize).saturating_sub(4))),
                Style::new().fg(th.fg_muted),
            ))
            .right_aligned(),
        );
    }
    let inner = block.inner(rect);
    block.render(rect, buf);
    inner
}

/// A row of buttons centered in `inner` at `y`, three columns apart, kept in `buttons` for the
/// mouse (where each was drawn; `top`: the dialog is on top, its arming clock runs from `now`). The one `Enter` presses
/// (`focus`) is in brackets (so it shows without color) and reversed, the others are raised;
/// the one under the pointer is underlined, the focus does not move to it. Nothing is drawn
/// below the box.
pub(crate) fn button_row(
    buf: &mut Buffer,
    inner: Rect,
    y: u16,
    labels: &[Localized],
    focus: Option<usize>,
    buttons: &mut Buttons,
    (top, now): (bool, Instant),
) {
    buttons.press.drawn(top, now);
    buttons.rects = button_rects(buf, inner, y, labels, focus, buttons.hover);
}

fn button_rects(
    buf: &mut Buffer,
    inner: Rect,
    y: u16,
    labels: &[Localized],
    focus: Option<usize>,
    hover: Option<usize>,
) -> Vec<Rect> {
    let th = theme::cur();
    if y >= inner.y + inner.height {
        return Vec::new();
    }
    let texts: Vec<String> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| if focus == Some(i) { format!("[ {l} ]") } else { format!("  {l}  ") })
        .collect();
    let total = texts.iter().map(|t| crate::text::width(t)).sum::<usize>() + 3 * texts.len().saturating_sub(1);
    let mut x = inner.x + inner.width.saturating_sub(total as u16) / 2;
    let right = inner.x + inner.width;
    let mut rects = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        let style = if focus == Some(i) {
            Style::new().fg(th.fg).patch(th.selection).add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else if hover == Some(i) {
            Style::new().fg(th.accent).bg(th.surface_alt).patch(crate::widgets::hover_style())
        } else {
            Style::new().fg(th.fg).bg(th.surface_alt)
        };
        let used = put(buf, x, y, text, right.saturating_sub(x) as usize, style);
        rects.push(Rect::new(x, y, used, 1));
        x = (x + used + 3).min(right);
    }
    rects
}

/// A yes/no confirmation: its question in the warning color, its buttons (the safe one first,
/// the one `Enter` presses focused) and its keys in the footer.
pub(crate) fn draw_confirm(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let when = (app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::Confirm), app.now());
    let Some(c) = app.overlays.confirm() else { return };
    let keys = match &c.tx {
        Some(tx) => app.tx_choice_keys(c.action, tx.commit),
        None => app.i18n.label(c.keys),
    };
    let (title, text) = (app.i18n.label(c.title), app.i18n.msg(&c.text));
    // Four buttons (commit or roll back first) need more room.
    let w = if c.tx.is_some() { area.width.saturating_sub(4).min(76) } else { area.width.saturating_sub(8).min(60) };
    let width = (w as usize).saturating_sub(4);
    let mut lines = wrap_words(&text, width);
    for d in &c.details {
        lines.extend(wrap_words(&app.i18n.msg(d), width));
    }
    let (buttons, focus) = c.buttons();
    let labels: Vec<Localized> = buttons.iter().map(|(l, _)| app.i18n.label(*l)).collect();
    // As many lines as fit (the box has a border, a blank line above and below them, and the
    // buttons).
    let max = (area.height as usize).saturating_sub(5).max(1);
    let n = lines.len().min(max);
    let rect = centered(area, w, n as u16 + 5);
    let inner = modal(rect, &title, &keys, buf);
    for (i, l) in lines.iter().take(n).enumerate() {
        put(
            buf,
            inner.x + 1,
            inner.y + 1 + i as u16,
            l,
            (inner.width as usize).saturating_sub(2),
            Style::new().fg(th.warning).bg(th.surface),
        );
    }
    let Some(c) = app.overlays.confirm_mut() else { return };
    button_row(buf, inner, inner.y + 2 + n as u16, &labels, focus, &mut c.buttons, when);
}

/// The run confirmation: the connection (its name in its color) and its
/// environment, each statement that asks with why, its target and its first line, then the buttons,
/// Cancel first. As many statements as fit are listed; the rest are counted.
pub(crate) fn draw_run_confirm(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let when = (app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::RunConfirm), app.now());
    let Some(c) = app.overlays.run_confirm() else { return };
    let title = match c.as_is {
        true => app.i18n.label(Label::TxOutsideTitle),
        false => app.i18n.msg(&Msg::SafetyConfirmTitle { count: c.items.len() as u64 }),
    };
    let keys = app.i18n.label(Label::SafetyConfirmKeys);
    let w = area.width.saturating_sub(4).min(120);
    let iw = (w as usize).saturating_sub(4);
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    let total = c.statements.len();
    // Each statement: why (and its target), then its first line.
    let mut items: Vec<[Vec<(String, Style)>; 2]> = Vec::new();
    for d in &c.items {
        let mut head = vec![
            (format!("{}/{total}  ", d.index + 1), surface(th.fg_dim)),
            (crate::app::safety::why_text(&app.i18n, d), surface(th.warning).add_modifier(Modifier::BOLD)),
        ];
        if let Some(t) = &d.target {
            let target = app.i18n.msg(&Msg::SafetyConfirmTarget { target: t.clone() });
            head.push((format!("  {target}"), surface(th.fg)));
        }
        let sql = crate::app::runlog::excerpt(&d.sql, iw.saturating_sub(6));
        items.push([head, vec![(format!("     {sql}"), surface(th.fg_muted))]]);
    }
    // Border (2), the connection, a blank line, the statements, a blank line, the buttons.
    let fixed = 6usize;
    let room = (area.height as usize).saturating_sub(fixed + 1) / 2;
    let shown = if items.len() <= room { items.len() } else { room.saturating_sub(1).max(1) };
    let more = items.len() - shown.min(items.len());
    let body = shown * 2 + usize::from(more > 0);
    let rect = centered(area, w, (fixed + body) as u16);
    let inner = modal(rect, &title, &keys, buf);
    let x = inner.x + 1;
    let mut y = inner.y;
    let line = |buf: &mut Buffer, y: u16, parts: &[(String, Style)]| {
        let mut cx = x;
        let end = inner.x + inner.width.saturating_sub(1);
        for (text, style) in parts {
            if cx >= end {
                break;
            }
            cx += put(buf, cx, y, text, (end - cx) as usize, *style) as u16;
        }
    };
    // The connection: its name in its color and its environment, and whether it is
    // read-only.
    if let Some(p) = app.profile(c.profile) {
        let color = theme::profile_color(p.display_color());
        let mut parts = vec![(
            format!("{} {}", crate::widgets::explorer::CONNECTED, p.name),
            surface(color).add_modifier(Modifier::BOLD),
        )];
        parts.extend(crate::widgets::statusbar::environment_parts(app, c.profile, th.surface));
        if app.read_only(c.profile) {
            let ro = crate::app::environments::read_only_badge(app);
            let color = crate::app::environments::read_only_color(&th);
            parts.push((format!("  {ro}"), surface(color).add_modifier(Modifier::BOLD)));
        }
        line(buf, y, &parts);
    }
    y += 2;
    for [head, sql] in items.iter().take(shown) {
        line(buf, y, head);
        line(buf, y + 1, sql);
        y += 2;
    }
    if more > 0 {
        let text = app.i18n.msg(&Msg::SafetyConfirmMore { count: more as u64 });
        line(buf, y, &[(text.to_string(), surface(th.fg_dim))]);
        y += 1;
    }
    // The buttons, Cancel first.
    let labels = [app.i18n.label(Label::SafetyConfirmCancel), app.i18n.label(Label::SafetyConfirmRun)];
    let focus = Some(usize::from(c.run_focused));
    let Some(c) = app.overlays.run_confirm_mut() else { return };
    button_row(buf, inner, y + 1, &labels, focus, &mut c.buttons, when);
}

/// The icons question: a live preview of a few Nerd Font glyphs (drawn as the
/// terminal draws them), what to answer when they look wrong, and the buttons, Yes then No; No
/// has the focus at first.
pub(crate) fn draw_icons_ask(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let when = (app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::IconsAsk), app.now());
    let Some(q) = app.overlays.icons_ask() else { return };
    let (title, keys) = (app.i18n.label(Label::IconsAskTitle), app.i18n.label(Label::IconsAskKeys));
    let w = area.width.saturating_sub(8).min(64);
    let iw = (w as usize).saturating_sub(4);
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    let hint = wrap_words(&app.i18n.msg(&Msg::IconsAskHint { key: app.key_for(Action::OpenSettings, Ctx::Nav) }), iw);
    // Border (2), the question, a blank line, the glyphs, a blank line, the hint, a blank line,
    // the buttons.
    let rect = centered(area, w, (hint.len() + 8) as u16);
    let inner = modal(rect, &title, &keys, buf);
    let x = inner.x + 1;
    put(buf, x, inner.y, &app.i18n.label(Label::IconsAskQuestion), iw, surface(th.fg).add_modifier(Modifier::BOLD));
    let glyphs = format!(
        "{}   {} {} {}   {}",
        crate::icons::preview(),
        crate::icons::KEY_PK,
        crate::icons::KEY_FK,
        crate::icons::KEY_UQ,
        crate::icons::TABLE
    );
    put(buf, x + 2, inner.y + 2, &glyphs, iw.saturating_sub(2), surface(th.accent));
    for (i, l) in hint.iter().enumerate() {
        put(buf, x, inner.y + 4 + i as u16, l, iw, surface(th.fg_muted));
    }
    let labels = [app.i18n.label(Label::IconsAskYes), app.i18n.label(Label::IconsAskNo)];
    let focus = Some(usize::from(!q.yes_focused));
    let by = inner.y + 5 + hint.len() as u16;
    let Some(q) = app.overlays.icons_ask_mut() else { return };
    button_row(buf, inner, by, &labels, focus, &mut q.buttons, when);
}

/// The busy notice: a small box with its text, waiting for background work.
pub(crate) fn draw_busy(app: &App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let Some(b) = app.overlays.iter().find_map(|o| if let Overlay::Busy(b) = o { Some(b) } else { None }) else {
        return;
    };
    let title = app.i18n.label(b.title);
    let text = app.i18n.label(b.text);
    let w = area.width.saturating_sub(4).min(60);
    let lines = wrap_words(&text, (w as usize).saturating_sub(4));
    let rect = centered(area, w, lines.len() as u16 + 4);
    let inner = modal(rect, &title, &app.i18n.msg(&Msg::BusyKeys { keys: app.busy_keys() }), buf);
    for (i, l) in lines.iter().enumerate() {
        put(
            buf,
            inner.x + 1,
            inner.y + 1 + i as u16,
            l,
            (inner.width as usize).saturating_sub(2),
            Style::new().fg(th.fg).bg(th.surface),
        );
    }
}

pub(crate) fn draw_prompt(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let th = theme::cur();
    let when = (app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::Password), app.now());
    let p = app.overlays.prompt()?;
    let title = match &p.title {
        Some(t) => app.i18n.msg(t),
        None => app.i18n.msg(&Msg::PromptPasswordTitle { name: p.profile.clone() }),
    };
    // The checkbox exists when the password can be saved to the profile's store.
    let keychain = p.save_to.is_some();
    let footer = app.i18n.label(match (&p.purpose, p.save_to) {
        (PromptPurpose::Test(_), _) => Label::PromptPasswordKeysTest,
        (PromptPurpose::Tunnel, None) => Label::SshPromptKeys,
        (_, Some(SourceKind::Keychain)) => Label::PromptPasswordKeys,
        (_, Some(_)) => Label::PromptPasswordKeysFile,
        (_, None) => Label::PromptPasswordKeysSession,
    });
    let label = match &p.field {
        crate::app::PromptField::Label(l) => app.i18n.label(*l).to_string(),
        crate::app::PromptField::Text(t) => t.clone(),
    };
    let mask = !p.echo;
    let save_label = app.i18n.label(p.note);
    let (save, save_focus) = (p.save, p.save_focus);
    let w = area.width.saturating_sub(4).min(64);
    let err_lines = wrap_words(&p.error.render(&app.i18n), (w as usize).saturating_sub(4));
    let err_color = level_color(p.error.level);
    let err_n = err_lines.len().min(3);
    // errors, blank, password, blank, checkbox, blank, buttons
    let rect = centered(area, w, err_n as u16 + 8);
    let inner = modal(rect, &title, &footer, buf);
    let iw = inner.width as usize;
    for (i, l) in err_lines.iter().take(err_n).enumerate() {
        put(buf, inner.x + 1, inner.y + i as u16, l, iw.saturating_sub(2), Style::new().fg(err_color).bg(th.surface));
    }
    let y = inner.y + err_n as u16 + 1;
    let lw = put(buf, inner.x + 1, y, &label, iw / 3, Style::new().fg(th.accent).bg(th.surface)) + 2;
    let input = Rect::new(inner.x + 1 + lw, y, inner.width.saturating_sub(lw + 2), 1);
    let cy = y + 2;
    let mut checkbox = Rect::default();
    let cursor = if keychain {
        let bg = if save_focus { th.selection } else { Style::new().bg(th.surface) };
        let text = format!("[{}] {save_label}", if save { "x" } else { " " });
        let used = put(buf, inner.x + 1, cy, &text, iw.saturating_sub(2), Style::new().fg(th.fg).patch(bg));
        checkbox = Rect::new(inner.x + 1, cy, used, 1);
        save_focus.then_some((inner.x + 2, cy))
    } else {
        put(buf, inner.x + 1, cy, &save_label, iw.saturating_sub(2), Style::new().fg(th.fg_dim).bg(th.surface));
        None
    };
    // `Enter` sends from the field and from the checkbox alike: OK is the focused button.
    let labels = [app.i18n.label(Label::DialogButtonOk), app.i18n.label(Label::DialogButtonCancel)];
    let p = app.overlays.prompt_mut()?;
    p.checkbox = checkbox;
    button_row(buf, inner, cy + 2, &labels, Some(0), &mut p.buttons, when);
    let bg = if save_focus { Style::new().bg(th.surface_alt) } else { th.selection };
    let cx = p.input.render(input, buf, Style::new().fg(th.fg).patch(bg), !save_focus, mask, None);
    cursor.or(Some((cx, y)))
}

/// The transaction summary: the connection and how the transaction began, its statements
/// that may have written (number, first line, rows, time; as many as fit, scrolled with
/// `j`/`k`), what it holds and for how long it has been open and idle, then the buttons
/// (Close first).
pub(crate) fn draw_tx_summary(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let when = (app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::TxSummary), app.now());
    let Some(s) = app.overlays.tx_summary() else { return };
    // Its transaction ended (or its tab went): it closes, never left on top unseen.
    let Some(tx) = app.tabs.get(s.tab).filter(|t| t.exec.user_tx()).and_then(|t| t.exec.tx.clone()) else {
        app.overlays.close(OverlayKind::TxSummary);
        return;
    };
    let Some(t) = app.tabs.get(s.tab) else { return };
    let (tab, profile, aborted) = (s.tab, t.profile, t.exec.tx_aborted);
    let busy = app.tab_busy(tab);
    let how = app.i18n.label(if tx.manual { Label::TxHowManual } else { Label::TxHowUser }).to_string();
    let title = app.i18n.msg(&Msg::TxSummaryTitle { how });
    let keys = app.i18n.label(if aborted { Label::TxSummaryKeysRollback } else { Label::TxSummaryKeys });
    let w = area.width.saturating_sub(4).min(110);
    let iw = (w as usize).saturating_sub(4);
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    // Border (2), the connection, a blank line, the list, a blank line, the footer, a blank
    // line, the buttons.
    let fixed = 8usize;
    let room = (area.height as usize).saturating_sub(fixed).max(1);
    let lines = tx.log.len().max(1);
    let shown = lines.min(room);
    let rect = centered(area, w, (fixed + shown) as u16);
    let inner = modal(rect, &title, &keys, buf);
    let x = inner.x + 1;
    let end = inner.x + inner.width.saturating_sub(1);
    let line = |buf: &mut Buffer, y: u16, parts: &[(String, Style)]| {
        let mut cx = x;
        for (text, style) in parts {
            if cx >= end {
                break;
            }
            cx += put(buf, cx, y, text, (end - cx) as usize, *style) as u16;
        }
    };
    let mut y = inner.y;
    if let Some(p) = profile.and_then(|p| app.profile(p)) {
        let color = theme::profile_color(p.display_color());
        let mut parts = vec![(
            format!("{} {}", crate::widgets::explorer::CONNECTED, p.name),
            surface(color).add_modifier(Modifier::BOLD),
        )];
        parts.extend(crate::widgets::statusbar::environment_parts(app, p.id, th.surface));
        line(buf, y, &parts);
    }
    y += 2;
    // The list: the scroll keeps the last line on screen.
    let max_scroll = tx.log.len().saturating_sub(shown);
    let scroll = app.overlays.tx_summary().map_or(0, |s| s.scroll).min(max_scroll);
    if let Some(s) = app.overlays.tx_summary_mut() {
        s.scroll = scroll;
    }
    let first = (tx.changes as usize).saturating_sub(tx.log.len());
    if tx.log.is_empty() {
        line(buf, y, &[(app.i18n.label(Label::TxSummaryEmpty).to_string(), surface(th.fg_muted))]);
    }
    let num_w = (first + tx.log.len()).to_string().len();
    for (k, e) in tx.log.iter().enumerate().skip(scroll).take(shown) {
        let rows = app.i18n.msg(&Msg::TxRows { count: e.rows }).to_string();
        let time = e.elapsed.map(datarig_core::i18n::fmt_elapsed).unwrap_or_default();
        let tail = format!("  {rows}  {time}");
        let num = format!("{:>num_w$}  ", first + k + 1);
        let room = iw.saturating_sub(crate::text::width(&num) + crate::text::width(&tail));
        let sql = crate::app::runlog::excerpt(&e.sql, room.max(8));
        let pad = room.saturating_sub(crate::text::width(&sql));
        line(
            buf,
            y + (k - scroll) as u16,
            &[
                (num, surface(th.fg_dim)),
                (format!("{sql}{}", " ".repeat(pad)), surface(th.fg)),
                (tail, surface(th.fg_muted)),
            ],
        );
    }
    y += shown as u16 + 1;
    // What it holds, for how long, and since when it is idle.
    let rows = app.i18n.msg(&Msg::TxRows { count: tx.rows }).to_string();
    let time = crate::app::tx::fmt_clock(app.tx_elapsed(&tx));
    let idle = crate::app::tx::fmt_clock(tx.idle(app.now(), busy));
    let mut footer = app.i18n.msg(&Msg::TxSummaryFooter { count: tx.changes, rows, time, idle }).to_string();
    if tx.warned {
        footer = format!("{footer} · {}", app.i18n.label(Label::StatusTxHoldingLocks));
    }
    let tone = if aborted || tx.warned { th.error } else { th.warning };
    line(buf, y, &[(footer, surface(tone).add_modifier(Modifier::BOLD))]);
    y += 2;
    let labels: Vec<Localized> = app.tx_summary_buttons().iter().map(|(l, _)| app.i18n.label(*l)).collect();
    let Some(s) = app.overlays.tx_summary_mut() else { return };
    button_row(buf, inner, y, &labels, Some(0), &mut s.buttons, when);
}

/// The session view: the profile, its database and environment, then the items of its plan in
/// order (the app's limits with their values, each statement under where it comes from and,
/// on PostgreSQL, how it goes behind a pooler; as many as fit, scrolled with `j`/`k`), the
/// footer, then Close.
pub(crate) fn draw_session_view(app: &mut App, area: Rect, buf: &mut Buffer) {
    use crate::app::overlay::{SessionLine, Tone};
    use crate::text::width;
    use datarig_core::profile::session::OnPooler;
    use datarig_core::sql::risk::NotLocal;
    let th = theme::cur();
    let when = (app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::SessionView), app.now());
    let Some(s) = app.overlays.session_view() else { return };
    let w = area.width.saturating_sub(4).min(110);
    let iw = (w as usize).saturating_sub(4);
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    let tone = |t: Tone| match t {
        Tone::Strong => surface(th.fg).add_modifier(Modifier::BOLD),
        Tone::Muted => surface(th.fg_muted),
        Tone::Warning => surface(th.warning),
        Tone::Error => surface(th.error),
    };
    // The app's limits line up: name, value, source.
    let col = |f: fn(&SessionLine) -> Option<&String>| s.lines.iter().filter_map(f).map(|t| width(t)).max();
    let what_w = col(|l| if let SessionLine::Intent { what, .. } = l { Some(what) } else { None }).unwrap_or(0);
    let value_w = col(|l| if let SessionLine::Intent { value, .. } = l { Some(value) } else { None }).unwrap_or(0);
    let pad = |t: &str, w: usize| format!("{t}{}", " ".repeat(w.saturating_sub(width(t)) + 2));
    // A text whole and exactly as written, wrapped: its first row indented by `first`, the rest
    // (its own lines, with their indent, and what wraps) by `more`.
    let block = |text: &str, first: usize, more: usize, style: Style| {
        let mut out: Vec<Vec<(String, Style)>> = Vec::new();
        for l in text.trim().lines().map(str::trim_end).filter(|l| !l.trim().is_empty()) {
            // Its own indent joins the row's (up to a quarter of the width); the rest is shown.
            let lead = (l.len() - l.trim_start_matches(' ').len()).min(iw / 4);
            let indent = more + lead;
            for r in crate::text::wrap_exact(&l[lead..], iw.saturating_sub(indent)) {
                let at = if out.is_empty() { first } else { indent };
                out.push(vec![(" ".repeat(at), style), (r, style)]);
            }
        }
        out
    };
    let rows = |lines: &[SessionLine]| {
        let mut out: Vec<Vec<(String, Style)>> = Vec::new();
        for l in lines {
            match l {
                SessionLine::Text(t, _) if t.is_empty() => out.push(Vec::new()),
                SessionLine::Text(t, k) => out.extend(wrap_words(t, iw).into_iter().map(|r| vec![(r, tone(*k))])),
                SessionLine::Intent { what, value, source } => out.push(vec![
                    (pad(what, what_w), surface(th.fg)),
                    (pad(value, value_w), surface(th.fg).add_modifier(Modifier::BOLD)),
                    (source.clone(), surface(th.fg_muted)),
                ]),
                SessionLine::Source(t) => out.push(vec![(t.clone(), surface(th.accent))]),
                SessionLine::Statement(sql) => out.extend(block(sql, 2, 4, surface(th.fg))),
                SessionLine::Pooled(p) => {
                    let text = match p {
                        OnPooler::PerTransaction(local) => {
                            app.i18n.msg(&Msg::SessionViewPooledLocal { sql: local.clone() }).to_string()
                        }
                        OnPooler::Refused(why) => {
                            let reason = app.i18n.label(match why {
                                NotLocal::Form => Label::SessionViewNotLocalForm,
                                NotLocal::Identity => Label::SessionViewNotLocalIdentity,
                            });
                            app.i18n.msg(&Msg::SessionViewPooledRefused { reason: reason.to_string() }).to_string()
                        }
                    };
                    out.extend(block(&text, 4, 6, surface(th.fg_dim)));
                }
            }
        }
        out
    };
    let (head, list, foot) = (rows(&s.head), rows(&s.lines), rows(std::slice::from_ref(&s.foot)));
    let title = s.title.clone();
    let keys = app.i18n.label(Label::SessionViewKeys);
    // Border (2), the head, a blank line, the list, a blank line, the footer, a blank line,
    // the buttons.
    let fixed = 6 + head.len() + foot.len();
    let room = (area.height as usize).saturating_sub(fixed).max(1);
    let shown = list.len().max(1).min(room);
    let rect = centered(area, w, (fixed + shown) as u16);
    let inner = modal(rect, &title, &keys, buf);
    let x = inner.x + 1;
    let end = inner.x + inner.width.saturating_sub(1);
    let line = |buf: &mut Buffer, y: u16, parts: &[(String, Style)]| {
        let mut cx = x;
        for (text, style) in parts {
            if cx >= end {
                break;
            }
            cx += put(buf, cx, y, text, (end - cx) as usize, *style) as u16;
        }
    };
    let mut y = inner.y;
    for r in &head {
        line(buf, y, r);
        y += 1;
    }
    y += 1;
    // The list: the scroll keeps the last line on screen.
    let max_scroll = list.len().saturating_sub(shown);
    let scroll = s.scroll.min(max_scroll);
    for (k, r) in list.iter().skip(scroll).take(shown).enumerate() {
        line(buf, y + k as u16, r);
    }
    y += shown as u16 + 1;
    for r in &foot {
        line(buf, y, r);
        y += 1;
    }
    y += 1;
    let labels = [app.i18n.label(Label::DialogButtonClose)];
    let Some(s) = app.overlays.session_view_mut() else { return };
    (s.scroll, s.max_scroll) = (scroll, max_scroll);
    button_row(buf, inner, y, &labels, Some(0), &mut s.buttons, when);
}
