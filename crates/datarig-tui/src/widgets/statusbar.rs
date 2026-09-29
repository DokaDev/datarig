//! The one-line status bar at the bottom of the workspace.

use crate::app::{App, Level};
use crate::text::{clip, width};
use crate::theme;
use crate::widgets::SPINNER;
use datarig_core::i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

pub(crate) fn level_color(l: Level) -> ratatui::style::Color {
    match l {
        Level::Info => theme::FG,
        Level::Success => theme::SUCCESS,
        Level::Warning => theme::WARNING,
        Level::Error => theme::ERROR,
    }
}

pub(crate) fn draw_status(app: &mut App, area: Rect, buf: &mut Buffer) {
    let bar = Style::new().bg(theme::SURFACE).fg(theme::FG);
    buf.set_style(area, bar);
    let total = area.width as usize;
    let sep = " │ ";
    let sep_style = Style::new().fg(theme::BORDER).bg(theme::SURFACE);

    // The connection's name in its color and its policy, as plain text.
    let conn = app.conn().map(|c| {
        let policy = app.i18n.msg(&Msg::StatusPolicy { name: c.policy.clone().unwrap_or_else(|| "default".into()) });
        (c.name.clone(), theme::profile_color(c.display_color()), policy.to_string())
    });
    // A read-only policy: a badge in words, in neutral colors.
    let read_only = app.conn().is_some_and(|c| app.read_only(c.id)).then(|| app.i18n.label(Label::StatusReadOnly));
    let exec = &app.tab().exec;
    let tx = exec.user_tx().then(|| {
        let (label, color) = if exec.tx_aborted {
            (Label::StatusTxAborted, theme::ERROR)
        } else {
            (Label::StatusTxOpen, theme::ACCENT_WARM)
        };
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
    .flatten();
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
    if let Some((name, color, policy)) = conn {
        segs.push((format!(" {name}"), Style::new().fg(color).bg(theme::SURFACE).add_modifier(Modifier::BOLD)));
        segs.push((format!(" · {policy}"), Style::new().fg(theme::FG_MUTED).bg(theme::SURFACE)));
    }
    if let Some(ro) = read_only {
        segs.push((" ".to_string(), Style::new().bg(theme::SURFACE)));
        segs.push((format!(" {ro} "), Style::new().fg(theme::FG).bg(theme::SURFACE_ALT).add_modifier(Modifier::BOLD)));
    }
    if let Some((tx, color)) = tx {
        push_sep(&mut segs);
        segs.push((tx, Style::new().fg(color).bg(theme::SURFACE).add_modifier(Modifier::BOLD)));
    }
    // What leads the message: a separator after the segments, else one blank.
    let lead = if segs.is_empty() { " " } else { sep };
    let fixed: usize = segs.iter().map(|(s, _)| width(s)).sum::<usize>() + width(&lang) + width(lead);
    let avail = total.saturating_sub(fixed);

    let mut msg_segs: Vec<(String, Style)> = Vec::new();
    if let Some((text, level, running)) = status {
        if running {
            let frame = app
                .tab()
                .exec
                .running
                .map(|r| (r.started.elapsed().as_millis() / 100) as usize % SPINNER.len())
                .unwrap_or(0);
            msg_segs.push((format!("{} ", SPINNER[frame]), Style::new().fg(theme::ACCENT).bg(theme::SURFACE)));
        }
        msg_segs.push((text.to_string(), Style::new().fg(level_color(level)).bg(theme::SURFACE)));
    }
    let msg_w: usize = msg_segs.iter().map(|(s, _)| width(s)).sum();
    // The hint line: as many entries as fit next to the message, best first.
    let hint_segs = |n: usize| -> Vec<(String, Style)> {
        let mut v: Vec<(String, Style)> = Vec::new();
        for (i, (k, label)) in hints.iter().take(n).enumerate() {
            if i > 0 {
                v.push((" · ".to_string(), Style::new().fg(theme::FG_DIM).bg(theme::SURFACE)));
            }
            v.push((k.clone(), Style::new().fg(theme::FG).bg(theme::SURFACE).add_modifier(Modifier::BOLD)));
            v.push((format!(" {label}"), Style::new().fg(theme::FG_MUTED).bg(theme::SURFACE)));
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
        let c = clip(&s, msg_room.saturating_sub(used));
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

/// The mode badge at the left end of the status bar and its style (lualine style): NORMAL,
/// INSERT and VISUAL in their colors, COMMAND while the `:` command line is open, and a
/// neutral badge with `[editor] mode = "standard"`. `None` without an editor (no tab, a table
/// tab) and a command line.
pub(crate) fn mode_badge(app: &App) -> Option<(datarig_core::i18n::Localized, Style)> {
    use crate::widgets::editor::Mode;
    let command = app.overlays.is_open(crate::app::overlay::OverlayKind::Commands);
    // No editor on screen: no tab, or a table tab.
    if (app.tabs.is_empty() || app.tab().is_table()) && !command {
        return None;
    }
    let (label, bg) = if command {
        (Label::StatusModeCommand, theme::MODE_COMMAND)
    } else {
        let mode = app.tab().editor.mode;
        let bg = match (app.editor_mode, mode) {
            (datarig_core::config::EditorMode::Standard, _) => theme::MODE_NEUTRAL,
            (_, Mode::Normal) => theme::MODE_NORMAL,
            (_, Mode::Insert) => theme::MODE_INSERT,
            (_, Mode::Visual) => theme::MODE_VISUAL,
        };
        (mode.label(), bg)
    };
    Some((app.i18n.label(label), Style::new().bg(bg).fg(theme::MODE_FG).add_modifier(Modifier::BOLD)))
}

/// Right end of the status bar: the Hangul/Latin indicator (`status.hangul`) while Hangul is read as QWERTY keys, then the language.
pub(crate) fn lang_segment(app: &App) -> String {
    let lang = format!(" {} ", app.i18n.label(Label::StatusLang));
    if app.hangul_hint_active() { format!(" {}{lang}", app.i18n.label(Label::StatusHangul)) } else { lang }
}

/// Draw [`lang_segment`] at the right end of `area`; returns its x.
pub(crate) fn draw_lang(app: &App, seg: &str, area: Rect, buf: &mut Buffer) -> u16 {
    let x = area.x + area.width - width(seg) as u16;
    let style = Style::new().fg(theme::ACCENT_WARM).bg(theme::SURFACE).add_modifier(Modifier::BOLD);
    let lang = format!(" {} ", app.i18n.label(Label::StatusLang));
    let hint_w = width(seg) - width(&lang);
    if hint_w > 0 {
        buf.set_stringn(
            x,
            area.y,
            &seg[..seg.len() - lang.len()],
            hint_w,
            Style::new().fg(theme::WARNING).bg(theme::SURFACE),
        );
    }
    buf.set_stringn(x + hint_w as u16, area.y, &lang, width(&lang), style);
    x
}
