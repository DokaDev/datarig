//! The settings screen: a large modal with every setting under its category, its
//! current value (`‹ value ›`) and what that value means; below the list, the selected
//! setting's description, and for `icons` a preview of the glyphs with the hint to turn them
//! off when they show as boxes.

use crate::app::App;
use crate::app::command::SETTINGS;
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

pub(crate) fn draw_settings(app: &App, area: Rect, buf: &mut Buffer) {
    let Some(screen) = app.overlays.settings() else { return };
    let i18n = &app.i18n;
    let w = area.width.saturating_sub(4).min(100);
    let rect = centered(area, w, area.height.saturating_sub(2));
    let inner = modal(rect, &i18n.label(Label::SettingsTitle), &i18n.label(Label::SettingsKeys), buf);
    let iw = inner.width as usize;
    let x = inner.x + 1;
    let tw = iw.saturating_sub(2);
    let bottom = inner.y + inner.height;
    let surface = |fg| Style::new().fg(fg).bg(theme::SURFACE);
    let mut y = inner.y;
    // A config file with errors: its error, and that nothing is saved.
    if let Some(p) = app.config_problem() {
        for l in wrap_words(&p.render(i18n), tw).into_iter().take(2) {
            put(buf, x, y, &l, tw, surface(level_color(p.level)));
            y += 1;
        }
        put(buf, x, y, &i18n.label(Label::SettingsNotSaved), tw, surface(theme::WARNING));
        y += 1;
    }
    // A blank line on top when there is room for it.
    if inner.height >= 26 {
        y += 1;
    }
    let name_w = SETTINGS.iter().map(|s| width(&i18n.label(s.label))).max().unwrap_or(10).min(tw / 3);
    let value_w = SETTINGS.iter().flat_map(|s| s.values.iter().map(|v| width(v.0))).max().unwrap_or(6);
    let mut row = 0;
    let mut selected_spec = None;
    for (group, items) in groups() {
        if y >= bottom {
            break;
        }
        put(buf, x, y, &i18n.label(group.label()), tw, surface(theme::ACCENT).add_modifier(Modifier::BOLD));
        y += 1;
        for k in items {
            if y >= bottom {
                break;
            }
            let spec = &SETTINGS[k];
            let on = row == screen.selected;
            let bg = if on { theme::SELECTION_BG } else { theme::SURFACE };
            buf.set_style(Rect::new(inner.x, y, inner.width, 1), Style::new().bg(bg));
            let mut cx = x + 2;
            put(
                buf,
                cx,
                y,
                &fit(&i18n.label(spec.label), name_w, Align::Left),
                name_w,
                Style::new().fg(theme::FG).bg(bg),
            );
            cx += name_w as u16 + 2;
            let value = app.setting_value(k).and_then(|v| spec.values.get(v));
            let shown = value.map_or("?", |v| v.0);
            let chosen = format!("‹ {} ›", fit(shown, value_w, Align::Left));
            let style = Style::new().fg(theme::ACCENT_WARM).bg(bg).add_modifier(Modifier::BOLD);
            cx += put(buf, cx, y, &chosen, tw.saturating_sub((cx - x) as usize), style) + 2;
            if let Some(v) = value {
                let left = tw.saturating_sub((cx - x) as usize);
                put(buf, cx, y, &i18n.label(v.2), left, Style::new().fg(theme::FG_MUTED).bg(bg));
            }
            if on {
                selected_spec = Some(spec);
            }
            row += 1;
            y += 1;
        }
    }
    // The selected setting's description (and the icons' preview).
    let Some(spec) = selected_spec else { return };
    y += 1;
    let mut lines: Vec<(String, Style)> = Vec::new();
    // For `icons` the preview and its hint come first: they must show on a small screen.
    if spec.key == "icons" {
        let preview = i18n.msg(&Msg::SettingIconsPreview { preview: crate::icons::preview() });
        lines.push((preview.to_string(), surface(theme::FG).add_modifier(Modifier::BOLD)));
        let hint = wrap_words(&i18n.label(Label::SettingIconsPreviewHint), tw);
        lines.extend(hint.into_iter().map(|l| (l, surface(theme::WARNING))));
    }
    lines.extend(wrap_words(&i18n.label(spec.about), tw).into_iter().map(|l| (l, surface(theme::FG))));
    if let Some(path) = app.config_file() {
        lines.push((i18n.msg(&Msg::SettingsFile { path }).to_string(), surface(theme::FG_DIM)));
    }
    if y < bottom {
        buf.set_stringn(inner.x, y - 1, "─".repeat(iw), iw, surface(theme::BORDER));
    }
    for (l, style) in lines {
        if y >= bottom {
            break;
        }
        put(buf, x, y, &l, tw, style);
        y += 1;
    }
}
