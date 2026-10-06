//! Centered modal boxes and the connect-time password prompt.

use crate::app::action::Action;
use crate::app::overlay::Overlay;
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

/// A row of buttons centered in `inner` at `y`, three columns apart. The one `Enter` presses
/// (`focus`) is in brackets (so it shows without color) and reversed, the others are raised;
/// the one under the pointer (`hover`) is underlined, the focus does not move to it. Returns
/// where each was drawn, for the mouse; nothing is drawn below the box.
pub(crate) fn button_row(
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
        let mut style = if focus == Some(i) {
            Style::new().fg(th.fg).patch(th.selection).add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else if hover == Some(i) {
            Style::new().fg(th.accent).bg(th.surface_alt)
        } else {
            Style::new().fg(th.fg).bg(th.surface_alt)
        };
        if hover == Some(i) {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
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
    let Some(c) = app.overlays.confirm() else { return };
    let (title, text, keys) = (app.i18n.label(c.title), app.i18n.msg(&c.text), app.i18n.label(c.keys));
    let w = area.width.saturating_sub(8).min(60);
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
    c.buttons.rects = button_row(buf, inner, inner.y + 2 + n as u16, &labels, focus, c.buttons.hover);
}

/// The run confirmation: the connection (its name in its color) and its
/// policy, each statement that asks with why, its target and its first line, then the buttons,
/// Cancel first. As many statements as fit are listed; the rest are counted.
pub(crate) fn draw_run_confirm(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let Some(c) = app.overlays.run_confirm() else { return };
    let title = app.i18n.msg(&Msg::SafetyConfirmTitle { count: c.items.len() as u64 });
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
    // The connection: its name in its color and its policy, and whether it is
    // read-only.
    if let Some(p) = app.profile(c.profile) {
        let color = theme::profile_color(p.display_color());
        let (policy, values) = app.policy_of(c.profile);
        let mut parts = vec![
            (
                format!("{} {}", crate::widgets::explorer::CONNECTED, p.name),
                surface(color).add_modifier(Modifier::BOLD),
            ),
            (format!(" · {}", app.i18n.msg(&Msg::StatusPolicy { name: policy })), surface(th.fg_muted)),
        ];
        if values.read_only {
            let ro = app.i18n.label(Label::StatusReadOnly);
            parts.push((format!("  {ro}"), surface(th.fg).add_modifier(Modifier::BOLD)));
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
    c.buttons.rects = button_row(buf, inner, y + 1, &labels, focus, c.buttons.hover);
}

/// The icons question: a live preview of a few Nerd Font glyphs (drawn as the
/// terminal draws them), what to answer when they look wrong, and the buttons, Yes then No; No
/// has the focus at first.
pub(crate) fn draw_icons_ask(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
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
    q.buttons.rects = button_row(buf, inner, by, &labels, focus, q.buttons.hover);
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
    p.buttons.rects = button_row(buf, inner, cy + 2, &labels, Some(0), p.buttons.hover);
    let bg = if save_focus { Style::new().bg(th.surface_alt) } else { th.selection };
    let cx = p.input.render(input, buf, Style::new().fg(th.fg).patch(bg), !save_focus, mask, None);
    cursor.or(Some((cx, y)))
}
