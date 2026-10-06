//! Rendering building blocks. Stateful widgets ([`editor`], [`grid`], [`tree`], [`text_input`])
//! own their state and key handling; the other modules draw parts of the [`App`] state
//! (explorer, status bar, popups, dialogs, command line, key guide, quick connect, profile
//! form).

pub mod editor;
pub mod grid;
pub mod plan;
pub mod tabbar;
pub mod text_input;
pub mod tree;

pub(crate) mod chooser;
pub(crate) mod cmdline;
pub(crate) mod dialog;
pub(crate) mod explorer;
pub(crate) mod form;
pub(crate) mod guide;
pub(crate) mod inspector;
pub(crate) mod menu;
pub(crate) mod popup;
pub(crate) mod quick;
pub(crate) mod script_tree;
pub(crate) mod settings;
pub(crate) mod statusbar;
pub(crate) mod tab_list;

use crate::text::{clip, width};
use crate::theme;
use datarig_core::i18n::Localized;
use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType};

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(crate) fn panel(title: &Localized, focused: bool, width: u16) -> Block<'static> {
    let t = clip(title, (width as usize).saturating_sub(6));
    frame(focused).title(Line::from(Span::styled(format!(" {t} "), title_style(focused))))
}

/// A panel's border and background, without a title.
fn frame(focused: bool) -> Block<'static> {
    let th = theme::cur();
    let border = if focused { th.accent } else { th.border };
    Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(border).bg(th.bg)).style(th.base())
}

fn title_style(focused: bool) -> Style {
    let th = theme::cur();
    let style = Style::new().fg(if focused { th.fg } else { th.fg_muted });
    if focused { style.add_modifier(Modifier::BOLD) } else { style }
}

/// A panel with a status on the right of its top border (the results' paging state): the
/// status keeps its place and the title is clipped before it (whole graphemes, `…`). `status`
/// is the texts from the longest to the shortest and their style: the first that leaves the
/// title enough room is used, and none when none fits.
pub(crate) fn panel_with_status(
    title: &Localized,
    status: Option<(&[String], Style)>,
    focused: bool,
    width: u16,
) -> Block<'static> {
    let (t, right) = match status {
        Some((texts, style)) => {
            let (t, s) = title_and_status(title, texts, width);
            (t, s.map(|s| Span::styled(format!(" {s} "), style)))
        }
        None => (clip(title, (width as usize).saturating_sub(4)), None),
    };
    let mut block = frame(focused).title(Line::from(Span::styled(format!(" {t} "), title_style(focused))));
    if let Some(r) = right {
        block = block.title(Line::from(r).right_aligned());
    }
    block
}

/// The title and the status a top border `width` columns wide shows: between the corners, the
/// title and the status each with a blank on both sides and at least one border dash between
/// them. The status keeps its place; the title is clipped (whole graphemes, `…`) but keeps up
/// to 8 columns, else the next shorter status is tried, else none.
pub(crate) fn title_and_status<S: AsRef<str>>(title: &str, texts: &[S], width: u16) -> (String, Option<String>) {
    let room = (width as usize).saturating_sub(2);
    let min_title = crate::text::width(title).min(8);
    let fits = |s: &str| (min_title + 2) + 1 + (crate::text::width(s) + 2) <= room;
    let status = texts.iter().map(AsRef::as_ref).find(|s| fits(s)).map(str::to_string);
    let taken = status.as_ref().map_or(0, |s| crate::text::width(s) + 3);
    (clip(title, room.saturating_sub(taken + 2)), status)
}

/// The spinner frame `now` (the app's clock, so tests draw a fixed frame).
pub(crate) fn spinner_at(started: std::time::Instant, now: std::time::Instant) -> &'static str {
    SPINNER[(now.saturating_duration_since(started).as_millis() / 100) as usize % SPINNER.len()]
}

/// Draw `text` at (x, y) clipped to `w` columns; returns the width used.
pub(crate) fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, w: usize, style: Style) -> u16 {
    let t = clip(text, w);
    buf.set_stringn(x, y, &t, w, style);
    width(&t) as u16
}

/// A list row under the pointer (not the selected one): the theme's alternate surface, or
/// underlined where that is the surface itself (the terminal theme), so it always shows.
pub(crate) fn hover_style() -> Style {
    let th = theme::cur();
    if th.surface_alt == th.surface {
        Style::new().bg(th.surface).add_modifier(Modifier::UNDERLINED)
    } else {
        Style::new().bg(th.surface_alt)
    }
}

/// A small clickable target under the pointer (a tab's `×`, a scroll mark, a result tab, a
/// paging arrow): the text on the selection, as a selected row reads on every theme.
pub(crate) fn pointer_style() -> Style {
    let th = theme::cur();
    Style::new().fg(th.fg).patch(th.selection).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests;
