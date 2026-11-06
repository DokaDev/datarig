//! What's new: the release notes this build carries (What's new of the docs site, embedded when
//! it was built), shown once after an update and with `:whatsnew`. Nothing is fetched: the notes
//! are those of the running version, offline too.
//!
//! At launch ([`App::whats_new_at_launch`]) the newest version that ran here (`seen` in
//! `update.toml`) decides: a newer running version shows the notes of the versions since, then
//! becomes `seen`. A first start shows nothing (nothing is new to a new user), and someone who
//! used a version from before What's new sees the running version's notes. `[updates]
//! whats_new = "off"` shows nothing by itself; `seen` still moves on. A second instance, a file
//! that cannot be read or a config with errors: nothing shown and nothing written, the next launch
//! decides. While the first steps of the welcome panel show, notes wait for a later launch; a first
//! start is kept all the same.

use super::App;
use super::overlay::{Buttons, Overlay, OverlayKind};
use crate::text::{width, wrap};
use datarig_core::config::WhatsNewSetting;
use datarig_core::i18n::{Localized, Msg};
use datarig_core::update::notes::{self, Block, Section, Span};
use datarig_core::update::{self, Cache, Version};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What's new of the docs site, as this build was made.
pub const NOTES: &str = include_str!("../../../../docs/site/src/whats-new.md");

/// The notes this build carries; none if they could not be read (a test reads them whole).
pub fn notes() -> notes::Notes {
    notes::parse(NOTES).unwrap_or_default()
}

/// The What's new dialog.
#[derive(Clone, Debug)]
pub struct WhatsNewView {
    pub title: Localized,
    /// The versions shown, newest first.
    pub sections: Vec<Section>,
    /// Scroll to this version's heading when first drawn.
    pub start: Option<Version>,
    /// The first row on screen, and the last first row it can be (as drawn).
    pub scroll: usize,
    pub max_scroll: usize,
    /// How many rows show at once (as drawn).
    pub page: usize,
    /// The rows of the version headings (as drawn).
    pub heads: Vec<usize>,
    pub buttons: Buttons,
}

impl WhatsNewView {
    pub fn new(title: Localized, sections: Vec<Section>, start: Option<Version>) -> Self {
        WhatsNewView {
            title,
            sections,
            start,
            scroll: 0,
            max_scroll: 0,
            page: 1,
            heads: Vec::new(),
            buttons: Buttons::default(),
        }
    }
}

/// How a piece of a row is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Version,
    Date,
    Heading,
    Text,
    Code,
    Bold,
}

/// A row of the dialog: its pieces.
pub type Row = Vec<(String, Ink)>;

/// The rows of `sections` at `width` columns, and the rows of their version headings.
/// Paragraphs and list items wrap at spaces; an item's next rows are indented under its text.
pub fn layout(sections: &[Section], cols: usize) -> (Vec<Row>, Vec<usize>) {
    let w = cols.max(8);
    let mut rows: Vec<Row> = Vec::new();
    let mut heads = Vec::new();
    for s in sections {
        if !rows.is_empty() {
            rows.push(Vec::new());
        }
        heads.push(rows.len());
        let (version, date) = (s.version.to_string(), format!("  {}", s.date));
        // The date beside the version when it fits (it is left out on a very narrow screen).
        if width(&version) + width(&date) <= w {
            rows.push(vec![(version, Ink::Version), (date, Ink::Date)]);
        } else {
            rows.push(vec![(version, Ink::Version)]);
        }
        let mut after_item = false;
        for b in &s.blocks {
            match b {
                Block::Paragraph(spans) => {
                    rows.push(Vec::new());
                    rows.extend(wrap_spans(spans, w, "", ""));
                }
                Block::Heading(h) => {
                    rows.push(Vec::new());
                    rows.push(vec![(h.clone(), Ink::Heading)]);
                }
                Block::Item(spans) => {
                    if !after_item {
                        rows.push(Vec::new());
                    }
                    rows.extend(wrap_spans(spans, w, "- ", "  "));
                }
            }
            after_item = matches!(b, Block::Item(_));
        }
    }
    (rows, heads)
}

/// `spans` as rows of at most `w` columns: the first starts with `first`, the rest with `more`
/// (both as wide). Words wrap at spaces; a word wider than a row is cut.
fn wrap_spans(spans: &[Span], w: usize, first: &str, more: &str) -> Vec<Row> {
    let room = w.saturating_sub(width(first)).max(4);
    // The words, each with its ink and whether a space comes before it.
    let mut words: Vec<(String, Ink, bool)> = Vec::new();
    let mut space = false;
    for span in spans {
        let (text, ink) = match span {
            Span::Text(t) => (t, Ink::Text),
            Span::Code(t) => (t, Ink::Code),
            Span::Bold(t) => (t, Ink::Bold),
        };
        for (i, part) in text.split(' ').enumerate() {
            if i > 0 {
                space = true;
            }
            if part.is_empty() {
                continue;
            }
            for (j, piece) in wrap(part, room).into_iter().enumerate() {
                words.push((piece, ink, space || j > 0));
                space = false;
            }
        }
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut row: Row = Vec::new();
    let mut used = 0;
    for (word, ink, space) in words {
        let sep = usize::from(space && used > 0);
        if used > 0 && used + sep + width(&word) > room {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        if used == 0 {
            let lead = if rows.is_empty() { first } else { more };
            if !lead.is_empty() {
                row.push((lead.to_string(), Ink::Text));
            }
        } else if sep == 1 {
            let between = ink_between(&row, ink);
            push(&mut row, " ", between);
            used += 1;
        }
        used += width(&word);
        push(&mut row, &word, ink);
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

/// The ink of a space between two words: the words' own when both have it (a space inside code
/// keeps the code's colour), else plain text.
fn ink_between(row: &Row, next: Ink) -> Ink {
    if row.last().is_some_and(|(_, i)| *i == next) { next } else { Ink::Text }
}

/// Add `text` to the row, joined with the last piece when it has the same ink.
fn push(row: &mut Row, text: &str, ink: Ink) {
    match row.last_mut() {
        Some((t, i)) if *i == ink => t.push_str(text),
        _ => row.push((text.to_string(), ink)),
    }
}

impl App {
    /// `:whatsnew`: every version this build carries, scrolled to the running one.
    pub(super) fn open_whats_new(&mut self) {
        let running = Version::running();
        let sections = notes().sections;
        // A build newer than its notes (a release candidate): the newest it has.
        let start = sections.iter().find(|s| s.version <= running).map(|s| s.version);
        let title = self.i18n.label(datarig_core::i18n::Label::WhatsNewTitle);
        self.overlays.close(OverlayKind::WhatsNew);
        self.overlays.push(Overlay::WhatsNew(WhatsNewView::new(title, sections, start)));
    }

    /// Launch: show What's new once after an update, and keep the running version as the
    /// newest seen ([`update::whats_new`] decides).
    pub(super) fn whats_new_at_launch(&mut self) {
        // The setting is not known with a config that has errors, nor whether this machine ran
        // datarig before with a usage file that cannot be read: the next launch decides.
        if self.whats_new_paused || self.config_broken || self.tips.unreadable {
            return;
        }
        let cache = Cache::load(&self.paths);
        let running = Version::running();
        let existing = self.tips.existed || self.tips.usage.onboarding_done;
        let on = self.prefs.whats_new == WhatsNewSetting::On;
        let plan = update::whats_new(&cache, &running, existing, on, !self.read_only);
        // The welcome panel's first steps come first: notes to show wait for a later launch
        // (nothing written). A first start is still kept, else the next launch would take this
        // install for one from before What's new.
        if plan.show.is_some() && self.onboarding_shown() {
            return;
        }
        if let (Some(seen), Cache::Known(path, state)) = (plan.seen, &cache) {
            let mut state = (**state).clone();
            state.seen = Some(seen);
            if let Err(e) = update::save(path, &state) {
                // Not kept: showing it would show it again at every launch.
                datarig_core::fault::ErrorLog::new(self.paths.errors_log())
                    .record("update.save", &datarig_core::fault::Fault::io(&e));
                return;
            }
        }
        let Some(after) = plan.show else { return };
        let all = notes();
        let sections: Vec<Section> = match after {
            Some(seen) => all.between(&seen, &running).into_iter().cloned().collect(),
            None => all.get(&running).cloned().into_iter().collect(),
        };
        if sections.is_empty() {
            return;
        }
        let title = self.i18n.msg(&Msg::WhatsNewTitleVersion { version: running.to_string() });
        self.overlays.push(Overlay::WhatsNew(WhatsNewView::new(title, sections, None)));
    }

    /// Keys of What's new: `j`/`k` and the arrows scroll a row, `Ctrl+D`/`Ctrl+U` half a page,
    /// `PageDown`/`Space`/`PageUp` a page, `g`/`G` to the top or the end, `]`/`[` to the next or
    /// previous version; `Esc`, `q` and `Enter` close. Held keys only scroll.
    pub(super) fn whats_new_key(&mut self, key: KeyEvent, repeat: bool) {
        let Some(v) = self.overlays.whats_new_mut() else { return };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half = (v.page / 2).max(1);
        let to = |v: &mut WhatsNewView, row: usize| v.scroll = row.min(v.max_scroll);
        match key.code {
            KeyCode::Char('d') if ctrl => to(v, v.scroll + half),
            KeyCode::Char('u') if ctrl => v.scroll = v.scroll.saturating_sub(half),
            KeyCode::Char('j') | KeyCode::Down => to(v, v.scroll + 1),
            KeyCode::Char('k') | KeyCode::Up => v.scroll = v.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => to(v, v.scroll + v.page.max(1)),
            KeyCode::PageUp => v.scroll = v.scroll.saturating_sub(v.page.max(1)),
            KeyCode::Char('g') | KeyCode::Home => v.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => v.scroll = v.max_scroll,
            KeyCode::Char(']') => {
                if let Some(h) = v.heads.iter().find(|h| **h > v.scroll).copied() {
                    to(v, h);
                }
            }
            KeyCode::Char('[') => v.scroll = v.heads.iter().rev().find(|h| **h < v.scroll).copied().unwrap_or(0),
            _ if repeat => {}
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.overlays.close(OverlayKind::WhatsNew),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
