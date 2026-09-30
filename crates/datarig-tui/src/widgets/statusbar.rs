//! The one-line status bar at the bottom of the workspace.

use crate::app::{App, Level};
use crate::text::{clip, clip_middle, width};
use crate::theme;
use crate::widgets::SPINNER;
use datarig_core::i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

pub(crate) fn level_color(l: Level) -> ratatui::style::Color {
    let th = theme::cur();
    match l {
        Level::Info => th.fg,
        Level::Success => th.success,
        Level::Warning => th.warning,
        Level::Error => th.error,
    }
}

pub(crate) fn draw_status(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let bar = Style::new().bg(th.surface).fg(th.fg);
    buf.set_style(area, bar);
    let total = area.width as usize;
    let sep = " │ ";
    let sep_style = Style::new().fg(th.border).bg(th.surface);

    // The connection's name in its color and its policy, as plain text.
    let conn = app.conn().map(|c| {
        let policy = app.i18n.msg(&Msg::StatusPolicy { name: c.policy.clone().unwrap_or_else(|| "default".into()) });
        (c.name.clone(), theme::profile_color(c.display_color()), policy.to_string())
    });
    // A read-only policy: a badge in words, in neutral colors.
    let read_only = app.conn().is_some_and(|c| app.read_only(c.id)).then(|| app.i18n.label(Label::StatusReadOnly));
    let exec = &app.tab().exec;
    let tx = exec.user_tx().then(|| {
        let (label, color) =
            if exec.tx_aborted { (Label::StatusTxAborted, th.error) } else { (Label::StatusTxOpen, th.accent_warm) };
        (app.i18n.label(label).to_string(), color)
    });
    let lang = lang_segment(app);
    let hints = app.hints();
    let status = app.status_line();
    // `detail_view = statusbar`: the selected cell in the message's place while the grid has
    // the focus (a fresh notice still comes first).
    let preview = (app.detail.visible
        && app.prefs.detail_view == datarig_core::config::DetailView::Statusbar
        && app.focus == crate::app::Focus::Results
        && app.transient.is_none())
    .then(|| crate::widgets::inspector::preview(app))
    .flatten()
    // The explorer's line of a table's structure, or of an object with its estimates, whole (the
    // explorer cuts deep lines, and short ones drop the estimates).
    .or_else(|| {
        (app.focus == crate::app::Focus::Tree && app.transient.is_none())
            .then(|| crate::widgets::explorer::line_preview(app))
            .flatten()
            .map(datarig_core::i18n::Localized::verbatim)
    });
    let status = match preview {
        Some(p) => Some((p, Level::Info, false)),
        None => status,
    };

    // The mode badge: the editor's mode while there is an editor (a tab), COMMAND while the
    // command line is open.
    let mut segs: Vec<(String, Style)> = Vec::new();
    if let Some((label, style)) = mode_badge(app) {
        segs.push((format!(" {label} "), style));
    }
    let push_sep = |segs: &mut Vec<(String, Style)>| segs.push((sep.to_string(), sep_style));
    let (mut name_seg, mut policy_seg) = (None, None);
    if let Some((name, color, policy)) = conn {
        name_seg = Some(segs.len());
        segs.push((format!(" {name}"), Style::new().fg(color).bg(th.surface).add_modifier(Modifier::BOLD)));
        policy_seg = Some(segs.len());
        segs.push((format!(" · {policy}"), Style::new().fg(th.fg_muted).bg(th.surface)));
    }
    if let Some(ro) = read_only {
        segs.push((" ".to_string(), Style::new().bg(th.surface)));
        segs.push((format!(" {ro} "), Style::new().fg(th.fg).bg(th.surface_alt).add_modifier(Modifier::BOLD)));
    }
    if let Some((tx, color)) = tx {
        push_sep(&mut segs);
        segs.push((tx, Style::new().fg(color).bg(th.surface).add_modifier(Modifier::BOLD)));
    }
    let mut msg_segs: Vec<(String, Style)> = Vec::new();
    if let Some((text, level, running)) = status {
        if running {
            let frame = app
                .tab()
                .exec
                .running
                .map(|r| (r.started.elapsed().as_millis() / 100) as usize % SPINNER.len())
                .unwrap_or(0);
            msg_segs.push((format!("{} ", SPINNER[frame]), Style::new().fg(th.accent).bg(th.surface)));
        }
        msg_segs.push((text.to_string(), Style::new().fg(level_color(level)).bg(th.surface)));
    }
    let msg_w: usize = msg_segs.iter().map(|(s, _)| width(s)).sum();
    // What leads the message: a separator after the segments, else one blank.
    let lead = if segs.is_empty() { " " } else { sep };
    let fixed =
        |segs: &[(String, Style)]| segs.iter().map(|(s, _)| width(s)).sum::<usize>() + width(&lang) + width(lead);
    // A message too long for its room takes the policy's (the badge still says read-only), then
    // what it needs of the connection's name, which keeps its first [`NAME_MIN`] columns: the
    // message is cut only when that is not enough.
    if let Some(i) = policy_seg
        && total.saturating_sub(fixed(&segs)) < msg_w
    {
        segs.remove(i);
    }
    let over = msg_w.saturating_sub(total.saturating_sub(fixed(&segs)));
    if let Some(i) = name_seg
        && over > 0
    {
        let name = segs[i].0[1..].to_string();
        let keep = width(&name).saturating_sub(over).max(NAME_MIN.min(width(&name)));
        segs[i].0 = format!(" {}", clip(&name, keep));
    }
    let avail = total.saturating_sub(fixed(&segs));
    // The hint line: as many entries as fit next to the message, best first.
    let hint_segs = |n: usize| -> Vec<(String, Style)> {
        let mut v: Vec<(String, Style)> = Vec::new();
        for (i, (k, label)) in hints.iter().take(n).enumerate() {
            if i > 0 {
                v.push((" · ".to_string(), Style::new().fg(th.fg_dim).bg(th.surface)));
            }
            v.push((k.clone(), Style::new().fg(th.fg).bg(th.surface).add_modifier(Modifier::BOLD)));
            v.push((format!(" {label}"), Style::new().fg(th.fg_muted).bg(th.surface)));
        }
        v
    };
    let seg_w = |v: &[(String, Style)]| v.iter().map(|(s, _)| width(s)).sum::<usize>();
    let mut hint = Vec::new();
    for n in (1..=hints.len()).rev() {
        let v = hint_segs(n);
        if avail > msg_w + seg_w(&v) + width(sep) {
            hint = v;
            break;
        }
    }
    let show_hint = !hint.is_empty();
    let hint_w = if show_hint { seg_w(&hint) + width(sep) } else { 0 };

    segs.push((lead.to_string(), sep_style));
    let msg_room = avail - hint_w;
    let mut used = 0;
    for (s, st) in msg_segs {
        // Cut in the middle: the end of a message says what to do.
        let c = clip_middle(&s, msg_room.saturating_sub(used));
        used += width(&c);
        segs.push((c, st));
    }
    let pad = msg_room.saturating_sub(used);
    segs.push((" ".repeat(pad), bar));
    if show_hint {
        push_sep(&mut segs);
        segs.extend(hint);
    }

    let mut x = area.x;
    let lang_x = draw_lang(app, &lang, area, buf);
    for (s, st) in segs {
        if x >= lang_x {
            break;
        }
        let room = (lang_x - x) as usize;
        let s = clip(&s, room);
        buf.set_stringn(x, area.y, &s, room, st);
        x += width(&s) as u16;
    }
}

/// The columns of the connection's name a long message leaves it.
const NAME_MIN: usize = 8;

/// The mode badge at the left end of the status bar and its style (lualine style): NORMAL,
/// INSERT and VISUAL (V-LINE by line) in their colors, COMMAND while the `:` command line is
/// open. `None` without an editor (no tab, a table tab) and a command line.
pub(crate) fn mode_badge(app: &App) -> Option<(datarig_core::i18n::Localized, Style)> {
    let th = theme::cur();
    use crate::widgets::editor::Mode;
    let command = app.overlays.is_open(crate::app::overlay::OverlayKind::Commands);
    // No editor on screen: no tab, or a table tab.
    if (app.tabs.is_empty() || app.tab().is_table()) && !command {
        return None;
    }
    let (label, bg) = if command {
        (Label::StatusModeCommand, th.mode_command)
    } else {
        let ed = &app.tab().editor;
        let bg = match ed.mode {
            Mode::Normal => th.mode_normal,
            Mode::Insert => th.mode_insert,
            Mode::Visual => th.mode_visual,
        };
        (ed.mode_label(), bg)
    };
    Some((app.i18n.label(label), Style::new().bg(bg).fg(th.mode_fg).add_modifier(Modifier::BOLD)))
}

/// Right end of the status bar: the Hangul/Latin indicator (`status.hangul`) while Hangul is read as QWERTY keys, then the language.
pub(crate) fn lang_segment(app: &App) -> String {
    let lang = format!(" {} ", app.i18n.label(Label::StatusLang));
    if app.hangul_hint_active() { format!(" {}{lang}", app.i18n.label(Label::StatusHangul)) } else { lang }
}

/// Draw [`lang_segment`] at the right end of `area`; returns its x.
pub(crate) fn draw_lang(app: &App, seg: &str, area: Rect, buf: &mut Buffer) -> u16 {
    let th = theme::cur();
    let x = area.x + area.width - width(seg) as u16;
    let style = Style::new().fg(th.accent_warm).bg(th.surface).add_modifier(Modifier::BOLD);
    let lang = format!(" {} ", app.i18n.label(Label::StatusLang));
    let hint_w = width(seg) - width(&lang);
    if hint_w > 0 {
        buf.set_stringn(x, area.y, &seg[..seg.len() - lang.len()], hint_w, Style::new().fg(th.warning).bg(th.surface));
    }
    buf.set_stringn(x + hint_w as u16, area.y, &lang, width(&lang), style);
    x
}
