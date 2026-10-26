//! The export dialog ([`crate::app::export_form`]): a field a line, its label in a column, the
//! notes under the rows, the format and the quoting as they are now, what Export did not like
//! under its field, then Export and Cancel.

use crate::app::export_form::{ExportField, ExportHit, quote_label, scope_label};
use crate::app::overlay::OverlayKind;
use crate::app::{App, Level, Notice};
use crate::text::{width, wrap_words};
use crate::theme;
use crate::widgets::dialog::{button_row, centered, modal};
use crate::widgets::put;
use crate::widgets::statusbar::level_color;
use datarig_core::export::stream::Quote;
use datarig_core::i18n::{Label, Localized, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// A line of the dialog's body.
enum Line {
    Field(ExportField),
    /// A note under a field, in its level's color.
    Note(String, Level),
}

/// Draw the export dialog; returns the hardware cursor (the focused text field's).
pub(crate) fn draw_export_dialog(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let th = theme::cur();
    let when = (app.overlays.top().map(|o| o.kind()) == Some(OverlayKind::Export), app.now());
    let f = app.overlays.export_form()?;
    let i18n = &app.i18n;
    let title = i18n.msg(&Msg::ExportDialogTitle { name: f.name.clone() });
    let footer = i18n.label(Label::ExportDialogKeys);
    let fields: Vec<ExportField> = f.fields().into_iter().filter(|x| !x.is_button()).collect();
    let w = area.width.saturating_sub(4).min(100);
    let iw = w.saturating_sub(2) as usize;
    // The labels' column stays put whatever fields the format shows.
    let every = [
        ExportField::Scope,
        ExportField::Format,
        ExportField::Delimiter,
        ExportField::Quote,
        ExportField::Header,
        ExportField::Null,
        ExportField::Path,
    ];
    let label_w = every.iter().map(|x| width(&i18n.label(x.label()))).max().unwrap_or(8) + 2;
    let note_w = iw.saturating_sub(label_w + 2).max(10);
    // The notes of each field as they are now; what Export did not like is the error's color
    // (a note that says it already takes that color).
    let mut lines = Vec::new();
    for &field in &fields {
        lines.push(Line::Field(field));
        let mut notes = match field {
            ExportField::Scope => app.export_scope_notes(f),
            ExportField::Format => app.export_refusal(f.scope, f.format).into_iter().collect(),
            ExportField::Quote if f.quote == Quote::Never => {
                vec![Notice::new(Label::ExportNoteNoQuotes, Level::Warning)]
            }
            // `Tab` found more than one name: they are listed; its folder is being read, or
            // could not be: that is said.
            ExportField::Path => f
                .completion
                .as_ref()
                .filter(|c| c.text == f.path.text())
                .and_then(|c| match &c.note {
                    Some(n) => Some(n.clone()),
                    None if c.matches.len() > 1 => {
                        Some(Notice::new(Msg::ExportPathMatches { names: c.names() }, Level::Info))
                    }
                    None => None,
                })
                .into_iter()
                .collect(),
            _ => Vec::new(),
        };
        if let Some((at, e)) = &f.error
            && *at == field
        {
            match notes.iter_mut().find(|n| n.msg == e.msg) {
                Some(n) => n.level = Level::Error,
                None => notes.push(e.clone()),
            }
        }
        for n in notes {
            lines.extend(wrap_words(&n.render(i18n), note_w).into_iter().map(|l| Line::Note(l, n.level)));
        }
    }
    let value = |field: ExportField| -> String {
        match field {
            ExportField::Scope => i18n.label(scope_label(f.scope)).to_string(),
            ExportField::Format => i18n.label(f.format.dialog_label()).to_string(),
            ExportField::Quote => i18n.label(quote_label(f.quote)).to_string(),
            ExportField::Header => {
                format!("[{}] {}", if f.header { "x" } else { " " }, i18n.label(Label::ExportHeaderOn))
            }
            _ => String::new(),
        }
    };
    let values: Vec<String> = fields.iter().map(|x| value(*x)).collect();
    let hint = |field: ExportField| -> Localized {
        match field {
            ExportField::Format => Localized::verbatim(format!(".{}", f.format.extension())),
            ExportField::Delimiter => i18n.label(Label::ExportDelimiterHint),
            ExportField::Null => i18n.label(Label::ExportNullHint),
            _ => Localized::verbatim(String::new()),
        }
    };
    let hints: Vec<Localized> = fields.iter().map(|x| hint(*x)).collect();
    let labels: Vec<Localized> = fields.iter().map(|x| i18n.label(x.label())).collect();
    let buttons = [i18n.label(Label::DialogButtonExport), i18n.label(Label::DialogButtonCancel)];
    let browse = format!("[{}]", i18n.label(Label::ExportPathBrowse));
    // Border (2), a blank line, the lines, a blank line, the buttons.
    let rect = centered(area, w, (lines.len() + 5) as u16);
    let inner = modal(rect, &title, &footer, buf);
    let right = inner.x + inner.width;
    let room = |x: u16| right.saturating_sub(x + 1) as usize;
    let vx = inner.x + 1 + label_w as u16;
    let surface = |fg| Style::new().fg(fg).bg(th.surface);
    let f = app.overlays.export_form_mut()?;
    f.hits.clear();
    let mut cursor = None;
    let mut y = inner.y + 1;
    for line in &lines {
        if y + 2 >= inner.y + inner.height {
            break;
        }
        match line {
            Line::Note(text, level) => {
                let color = match level {
                    Level::Info | Level::Success => th.fg_muted,
                    l => level_color(*l),
                };
                put(buf, vx, y, text, room(vx), surface(color));
            }
            Line::Field(field) => {
                let i = fields.iter().position(|x| x == field).unwrap_or(0);
                let focused = f.focus == *field;
                f.hits.push((Rect::new(inner.x, y, inner.width, 1), ExportHit::Field(*field)));
                let label_style =
                    if focused { surface(th.accent).add_modifier(Modifier::BOLD) } else { surface(th.fg_muted) };
                put(buf, inner.x + 1, y, &labels[i], label_w, label_style);
                let bg = if focused { th.selection } else { Style::new().bg(th.surface_alt) };
                let mut x = vx;
                if field.is_text() {
                    let input_w = match field {
                        // Room for the `[Browse…]` button after it.
                        ExportField::Path => room(vx).saturating_sub(width(&browse) + 1) as u16,
                        ExportField::Delimiter => 6,
                        _ => 16,
                    };
                    let rect = Rect::new(x, y, input_w.min(room(vx) as u16), 1);
                    f.hits.push((rect, ExportHit::Input(*field)));
                    let input = f.input_mut(*field)?;
                    let cx = input.render(rect, buf, Style::new().fg(th.fg).patch(bg), focused, false, None);
                    if focused {
                        cursor = Some((cx, y));
                    }
                    x += rect.width;
                    if *field == ExportField::Path {
                        let used = put(buf, x + 1, y, &browse, room(x + 1), surface(th.accent));
                        f.hits.push((Rect::new(x + 1, y, used, 1), ExportHit::Browse));
                        x += used + 1;
                    }
                } else {
                    let text = if field.is_choice() { format!("{} ▾ ", values[i]) } else { values[i].clone() };
                    let used = put(buf, x, y, &text, room(x), Style::new().fg(th.fg).patch(bg));
                    f.hits.push((Rect::new(x, y, used, 1), ExportHit::Value(*field)));
                    x += used;
                }
                if !hints[i].is_empty() {
                    put(buf, x + 2, y, &hints[i], room(x + 2), surface(th.fg_dim));
                }
            }
        }
        y += 1;
    }
    let focus = Some(usize::from(f.focus == ExportField::Cancel));
    button_row(buf, inner, y + 1, &buttons, focus, &mut f.buttons, when);
    cursor
}
