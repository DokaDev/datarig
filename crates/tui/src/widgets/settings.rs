//! The settings screen: a large modal with every setting under its category, its
//! current value (`‹ value ›`) and what that value means (on a short screen the list scrolls
//! and says how many lines are out of view above and below); below the list, the selected
//! setting's description, and for `icons` a preview of the glyphs with the hint to turn them
//! off when they show as boxes; for `theme` the terminal's background, the theme being
//! previewed and the key that copies it to a new theme file. A configured theme that is not
//! used is said on top, as a config file with errors.

use crate::app::App;
use crate::app::command::{SETTINGS, SettingSpec, Values};
use crate::app::settings::{SettingsRow, groups};
use crate::text::{Align, fit, width, wrap_words};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::put;
use crate::widgets::statusbar::level_color;
use datarig_i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

pub(crate) fn draw_settings(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let Some(screen) = app.overlays.settings() else { return };
    let i18n = &app.i18n;
    let w = area.width.saturating_sub(4).min(100);
    let rect = centered(area, w, area.height.saturating_sub(2));
    let inner = modal(rect, &i18n.label(Label::SettingsTitle), &i18n.label(Label::SettingsKeys), buf);
    let iw = inner.width as usize;
    let x = inner.x + 1;
    let tw = iw.saturating_sub(2);
    let bottom = inner.y + inner.height;
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    let mut y = inner.y;
    // A config file with errors: its error, and that nothing is saved.
    if let Some(p) = app.config_problem() {
        for l in wrap_words(&p.render(i18n), tw).into_iter().take(2) {
            put(buf, x, y, &l, tw, surface(level_color(p.level)));
            y += 1;
        }
        put(buf, x, y, &i18n.label(Label::SettingsNotSaved), tw, surface(th.warning));
        y += 1;
    }
    // The configured theme is not used: why (the terminal theme draws instead).
    if let Some(p) = app.theme_problem() {
        for l in wrap_words(&p.render(i18n), tw).into_iter().take(4) {
            put(buf, x, y, &l, tw, surface(level_color(p.level)));
            y += 1;
        }
    }
    // The selected setting's description (and the icons' preview), drawn under the list.
    let groups = groups();
    let selected_spec = groups.iter().flat_map(|(_, items)| items.iter()).nth(screen.selected).map(|&k| &SETTINGS[k]);
    let lines = selected_spec.map(|spec| description(app, spec, tw)).unwrap_or_default();
    // A short screen keeps the first lines of the description under the list, which scrolls
    // in what is left.
    let list_lines = groups.iter().map(|(_, items)| 1 + items.len()).sum::<usize>();
    let reserved = lines.len().min(DESCRIPTION_MIN);
    // A blank line on top when there is room for it.
    if inner.height >= 26 && list_lines + reserved < usize::from(bottom.saturating_sub(y)) {
        y += 1;
    }
    let room = usize::from(bottom.saturating_sub(y));
    let list_room =
        if list_lines + reserved <= room { list_lines } else { room.saturating_sub(reserved).max(1).min(room) };
    let list_bottom = y + list_room as u16;
    let name_w = SETTINGS.iter().map(|s| width(&i18n.label(s.label))).max().unwrap_or(10).min(tw / 3);
    // The fixed values line up; a longer theme name widens its own row only.
    let value_w = SETTINGS.iter().flat_map(|s| s.values.fixed().iter().map(|v| width(v.0))).max().unwrap_or(6);
    let mut row = 0;
    let mut rows = Vec::new();
    // Lines scrolled off the top so the selected row shows on a short screen: the list's lines
    // are the category titles and the rows.
    let selected_line = groups
        .iter()
        .scan(0, |before, (_, items)| {
            let start = *before;
            *before += items.len();
            Some((start, items.len()))
        })
        .enumerate()
        .find_map(|(g, (start, n))| (screen.selected < start + n).then_some(g + 1 + screen.selected))
        .unwrap_or(0);
    let mut skip = (selected_line + 1).saturating_sub(list_room);
    // The lines out of view above and below, said once the list is drawn.
    let (above, below) = (skip, list_lines.saturating_sub(skip + list_room));
    let list_top = y;
    for (group, items) in groups {
        if y >= list_bottom {
            break;
        }
        if skip > 0 {
            skip -= 1;
        } else {
            put(buf, x, y, &i18n.label(group.label()), tw, surface(th.accent).add_modifier(Modifier::BOLD));
            y += 1;
        }
        for k in items {
            if y >= list_bottom {
                break;
            }
            if skip > 0 {
                skip -= 1;
                row += 1;
                continue;
            }
            let spec = &SETTINGS[k];
            let on = row == screen.selected;
            let bg = if on {
                th.selection
            } else if screen.hover == Some(row) {
                crate::widgets::hover_style()
            } else {
                Style::new().bg(th.surface)
            };
            buf.set_style(Rect::new(inner.x, y, inner.width, 1), bg);
            let mut cx = x + 2;
            put(
                buf,
                cx,
                y,
                &fit(&i18n.label(spec.label), name_w, Align::Left),
                name_w,
                Style::new().fg(th.fg).patch(bg),
            );
            cx += name_w as u16 + 2;
            let value = app.setting_shown(k);
            let shown = value.as_ref().map_or("?", |v| v.0.as_str());
            let chosen = format!("‹ {} ›", fit(shown, value_w.max(width(shown)).min(tw / 3), Align::Left));
            let style = Style::new().fg(th.accent_warm).patch(bg).add_modifier(Modifier::BOLD);
            let used = put(buf, cx, y, &chosen, tw.saturating_sub((cx - x) as usize), style);
            // The arrows only where they were drawn.
            let whole = usize::from(used) == width(&chosen);
            rows.push(SettingsRow {
                line: Rect::new(inner.x, y, inner.width, 1),
                row,
                prev: Rect::new(cx, y, used.min(2), 1),
                value: Rect::new(cx + used.min(2), y, used.saturating_sub(if whole { 4 } else { 2 }), 1),
                next: if whole { Rect::new(cx + used - 2, y, 2, 1) } else { Rect::default() },
            });
            cx += used + 2;
            if let Some((_, label)) = value {
                let left = tw.saturating_sub((cx - x) as usize);
                put(buf, cx, y, &label, left, Style::new().fg(th.fg_muted).patch(bg));
            }
            row += 1;
            y += 1;
        }
    }
    // At the right end of the list's first and last lines (one line: the lines below).
    for (count, at, up) in [(above, list_top, true), (below, list_bottom.saturating_sub(1), false)] {
        if count == 0 || (up && list_room == 1 && below > 0) {
            continue;
        }
        let count = count as u64;
        let m = if up { Msg::SessionViewMoreAbove { count } } else { Msg::SessionViewMoreBelow { count } };
        let text = format!(" {} ", i18n.msg(&m));
        let end = x + tw as u16;
        let cx = end.saturating_sub(width(&text) as u16).max(x);
        put(buf, cx, at, &text, usize::from(end - cx), Style::new().fg(th.fg_dim));
    }
    if let Some(s) = app.overlays.settings_mut() {
        s.rows = rows;
    }
    if lines.is_empty() {
        return;
    }
    // The rule above the description, unless the description needs its line.
    if usize::from(bottom.saturating_sub(y)) > lines.len() {
        buf.set_stringn(inner.x, y, "─".repeat(iw), iw, surface(th.border));
        y += 1;
    }
    for (l, style) in lines {
        if y >= bottom {
            break;
        }
        put(buf, x, y, &l, tw, style);
        y += 1;
    }
}

/// The lines of the description that keep their place under the list on a short screen.
const DESCRIPTION_MIN: usize = 3;

/// The lines under the list for setting `spec`, `tw` columns wide: its description, after the
/// icons' preview or the theme's state for those, and the config file's path.
fn description(app: &App, spec: &SettingSpec, tw: usize) -> Vec<(String, Style)> {
    let th = theme::cur();
    let i18n = &app.i18n;
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    let mut lines: Vec<(String, Style)> = Vec::new();
    // For `icons` the preview and its hint come first: they must show on a small screen.
    if spec.key == "icons" {
        let preview = i18n.msg(&Msg::SettingIconsPreview { preview: crate::icons::preview() });
        lines.push((preview.to_string(), surface(th.fg).add_modifier(Modifier::BOLD)));
        let hint = wrap_words(&i18n.label(Label::SettingIconsPreviewHint), tw);
        lines.extend(hint.into_iter().map(|l| (l, surface(th.warning))));
    }
    // For `theme`: the preview (or why it cannot be used) and the terminal's background.
    if spec.values == Values::Themes {
        if let Some(p) = app.overlays.settings().and_then(|s| s.themes.preview.as_ref()) {
            let names = app.overlays.settings().map(|s| s.themes.names.as_slice()).unwrap_or_default();
            let name = names.get(p.index).cloned().unwrap_or_default();
            let (text, color) = match &p.error {
                Some(e) => (e.render(i18n).to_string(), th.error),
                None => (
                    i18n.msg(&Msg::SettingThemePreview { name, current: app.theme_name.clone() }).to_string(),
                    th.warning,
                ),
            };
            lines.extend(wrap_words(&text, tw).into_iter().map(|l| (l, surface(color))));
        }
        let background = match app.background {
            theme::Background::Light => Label::SettingThemeBackgroundLight,
            theme::Background::Dark => Label::SettingThemeBackgroundDark,
            theme::Background::Unknown => Label::SettingThemeBackgroundUnknown,
        };
        lines.extend(wrap_words(&i18n.label(background), tw).into_iter().map(|l| (l, surface(th.fg_muted))));
        let hint = wrap_words(&i18n.label(Label::ThemeAuthorCopyHint), tw);
        lines.extend(hint.into_iter().map(|l| (l, surface(th.fg_muted))));
    }
    lines.extend(wrap_words(&i18n.label(spec.about), tw).into_iter().map(|l| (l, surface(th.fg))));
    if let Some(path) = app.config_file() {
        lines.push((i18n.msg(&Msg::SettingsFile { path }).to_string(), surface(th.fg_dim)));
    }
    lines
}
