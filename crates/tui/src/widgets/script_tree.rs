//! The folder tree of the saved queries: "save as" with a name field below the
//! tree, "open" with a filter line; and of the file system for the SSH key file.

use crate::app::App;
use crate::app::key_picker::{self, KeyLook};
use crate::app::script_tree::{TreeFocus, TreeMode, TreeRow};
use crate::icons;
use crate::text::{Align, fit};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::put;
use datarig_core::i18n::Label;
use datarig_core::scripts;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Draw the tree dialog; returns the hardware cursor while its name or filter has the keyboard.
pub(crate) fn draw_script_tree(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let th = theme::cur();
    let icons_on = app.icons_on();
    let t = app.overlays.script_tree()?;
    let save = matches!(t.mode, TreeMode::Save { .. });
    let key_file = t.mode == TreeMode::KeyFile;
    let (title, footer, field) = match t.mode {
        TreeMode::Save { .. } => {
            (app.i18n.label(Label::NameSaveScript), app.i18n.label(Label::ScriptTreeKeysSave), Label::ScriptTreeName)
        }
        TreeMode::Open => (
            app.i18n.label(Label::ChooserOpenScriptTitle),
            app.i18n.label(Label::ScriptTreeKeysOpen),
            Label::ScriptTreeFilter,
        ),
        TreeMode::KeyFile => {
            (app.i18n.label(Label::KeyPickerTitle), app.i18n.label(Label::KeyPickerKeys), Label::KeyPickerFilter)
        }
    };
    let field = app.i18n.label(field).to_string();
    // Key file mode: the folder shown, `~/…` under the home folder.
    let top = if key_file {
        key_picker::shown_dir(&t.root, app.home().as_deref())
    } else {
        app.i18n.label(Label::ExplorerScripts).to_string()
    };
    let unreadable = app.i18n.label(Label::ScriptTreeUnreadable).to_string();
    let error = t.error.as_ref().map(|e| app.i18n.msg(e).to_string());
    let rows = t.rows();
    let selected = t.selected();
    let tree_focus = t.focus == TreeFocus::Tree;
    let w = area.width.saturating_sub(8).min(72);
    // The tree, a rule, the field and an error line.
    let list_h = rows.len().clamp(3, (area.height as usize).saturating_sub(10).max(3)) as u16;
    let rect = centered(area, w, list_h + 6);
    let inner = modal(rect, &title, &footer, buf);
    let iw = inner.width as usize;
    let list = Rect::new(inner.x, inner.y, inner.width, list_h);
    // Keep the selection on screen unless the wheel moved the rows away from it.
    let t = app.overlays.script_tree_mut()?;
    let h = list_h as usize;
    if !t.detached {
        if selected < t.scroll {
            t.scroll = selected;
        } else if selected >= t.scroll + h {
            t.scroll = selected + 1 - h;
        }
    }
    t.scroll = t.scroll.min(rows.len().saturating_sub(h));
    t.list = list;
    let scroll = t.scroll;
    let t = app.overlays.script_tree()?;
    for (i, (row, depth)) in rows.iter().enumerate().skip(scroll).take(h) {
        let y = list.y + (i - scroll) as u16;
        let bg = match (i == selected, tree_focus) {
            (true, true) => th.selection,
            (true, false) => Style::new().bg(th.surface_alt),
            _ => Style::new().bg(th.surface),
        };
        buf.set_stringn(list.x, y, fit("", iw, Align::Left), iw, bg);
        let mut x = list.x + (*depth * 2) as u16;
        let right = list.x + list.width;
        let room = |x: u16| right.saturating_sub(x + 1) as usize;
        let dim = Style::new().fg(th.fg_dim).patch(bg);
        let arrow = match t.is_open(row) {
            Some(true) => "▾ ",
            Some(false) => "▸ ",
            None => "  ",
        };
        x += put(buf, x, y, arrow, room(x), dim);
        let (text, style) = match row {
            TreeRow::Up => ("../".to_string(), Style::new().fg(th.accent).patch(bg)),
            TreeRow::Top => (
                if top.ends_with('/') { top.clone() } else { format!("{top}/") },
                Style::new().fg(th.fg).patch(bg).add_modifier(Modifier::BOLD),
            ),
            TreeRow::Folder(p) => {
                (format!("{}/", scripts::display_name(p, true)), Style::new().fg(th.accent).patch(bg))
            }
            // Key file mode: the whole name; likely keys stand out, public keys and the like
            // are dimmed.
            TreeRow::File(p) if key_file => {
                let name = p.rsplit('/').next().unwrap_or(p).to_string();
                let style = match key_picker::key_look(&name) {
                    KeyLook::Likely => Style::new().fg(th.accent_warm).patch(bg).add_modifier(Modifier::BOLD),
                    KeyLook::Dim => Style::new().fg(th.fg_dim).patch(bg),
                    KeyLook::Other => Style::new().fg(th.fg).patch(bg),
                };
                (name, style)
            }
            TreeRow::File(p) => {
                if icons_on {
                    x += put(buf, x, y, &format!("{} ", icons::SCRIPT), room(x), dim);
                }
                (scripts::display_name(p, false).to_string(), Style::new().fg(th.fg).patch(bg))
            }
        };
        x += put(buf, x, y, &text, room(x), style);
        if t.unreadable(row) {
            put(buf, x + 1, y, &unreadable, room(x + 1), Style::new().fg(th.warning).patch(bg));
        }
    }
    let rule = inner.y + list_h;
    buf.set_stringn(inner.x, rule, "─".repeat(iw), iw, Style::new().fg(th.border).bg(th.surface));
    let fy = rule + 1;
    let label = format!("{field}: ");
    let lw = put(buf, inner.x + 1, fy, &label, iw.saturating_sub(2), Style::new().fg(th.fg_muted).bg(th.surface));
    // A long error wraps onto the two lines below the field.
    let lines = error.as_ref().map(|e| crate::text::wrap_words(e, iw.saturating_sub(2))).unwrap_or_default();
    for (i, l) in lines.iter().take(2).enumerate() {
        put(buf, inner.x + 1, fy + 1 + i as u16, l, iw.saturating_sub(2), Style::new().fg(th.error).bg(th.surface));
    }
    // The folder the name goes into, before the name.
    let folder = t.folder().map(|f| format!("{f}/")).filter(|_| save).unwrap_or_default();
    let mut ix = inner.x + 1 + lw;
    ix += put(
        buf,
        ix,
        fy,
        &folder,
        iw.saturating_sub((ix - inner.x) as usize + 1),
        Style::new().fg(th.fg_dim).bg(th.surface),
    );
    let t = app.overlays.script_tree_mut()?;
    let input = Rect::new(ix, fy, (inner.x + inner.width).saturating_sub(ix + 1), 1);
    let bg = if tree_focus { Style::new().bg(th.surface) } else { th.selection };
    let cx = t.input.render(input, buf, Style::new().fg(th.fg).patch(bg), !tree_focus, false, None);
    (!tree_focus).then_some((cx, fy))
}
