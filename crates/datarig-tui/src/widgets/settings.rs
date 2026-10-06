//! The settings screen: a large modal with every setting under its category, its
//! current value (`‹ value ›`) and what that value means; below the list, the selected
//! setting's description, and for `icons` a preview of the glyphs with the hint to turn them
//! off when they show as boxes; for `theme` the terminal's background and the theme being
//! previewed. A configured theme that is not used is said on top, as a config file with errors.

use crate::app::App;
use crate::app::command::{SETTINGS, Values};
use crate::app::settings::groups;
use crate::text::{Align, fit, width, wrap_words};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::put;
use crate::widgets::statusbar::level_color;
use datarig_core::i18n::{Label, Msg};
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
    // A blank line on top when there is room for it.
    if inner.height >= 26 {
        y += 1;
    }
    let name_w = SETTINGS.iter().map(|s| width(&i18n.label(s.label))).max().unwrap_or(10).min(tw / 3);
    // The fixed values line up; a longer theme name widens its own row only.
    let value_w = SETTINGS.iter().flat_map(|s| s.values.fixed().iter().map(|v| width(v.0))).max().unwrap_or(6);
    let mut row = 0;
    let mut selected_spec = None;
    let mut rows = Vec::new();
    for (group, items) in groups() {
        if y >= bottom {
            break;
        }
        put(buf, x, y, &i18n.label(group.label()), tw, surface(th.accent).add_modifier(Modifier::BOLD));
        y += 1;
        for k in items {
            if y >= bottom {
                break;
            }
            let spec = &SETTINGS[k];
            let on = row == screen.selected;
            let bg = if on { th.selection } else { Style::new().bg(th.surface) };
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
            rows.push((Rect::new(inner.x, y, inner.width, 1), row, Rect::new(cx, y, used, 1)));
            cx += used + 2;
            if let Some((_, label)) = value {
                let left = tw.saturating_sub((cx - x) as usize);
                put(buf, cx, y, &label, left, Style::new().fg(th.fg_muted).patch(bg));
            }
            if on {
                selected_spec = Some(spec);
            }
            row += 1;
            y += 1;
        }
    }
    if let Some(s) = app.overlays.settings_mut() {
        s.rows = rows;
    }
    // The selected setting's description (and the icons' preview).
    let Some(spec) = selected_spec else { return };
    let Some(screen) = app.overlays.settings() else { return };
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
        if let Some(p) = &screen.themes.preview {
            let name = screen.themes.names.get(p.index).cloned().unwrap_or_default();
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
    }
    lines.extend(wrap_words(&i18n.label(spec.about), tw).into_iter().map(|l| (l, surface(th.fg))));
    if let Some(path) = app.config_file() {
        lines.push((i18n.msg(&Msg::SettingsFile { path }).to_string(), surface(th.fg_dim)));
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
