//! The editor of a cell's value of several lines: the query editor's widget in a box over the
//! screen (`Ctx::CellEditor`), for a value with line breaks, a JSON column's, or the text of the
//! cell editor's line taken there (`Ctrl+L`). Its vim modes work as in a tab; in Normal mode
//! `Enter` or `:w` keeps the text as the line's `Enter` does, `Esc` or `:q` cancels. The `:` line
//! is its own, on the box's last line: it takes those and the editor's commands (`:s`, a line).

use super::*;
use crate::widgets::editor::{EdEvent, Editor, ExDone, Mode};
use datarig_core::sql::dialect::Language;
use ratatui::layout::Rect;

/// The editor of several lines of an open cell editor.
pub struct Lines {
    pub editor: Editor,
    /// The `:` line, while it is open.
    pub ex: Option<TextInput>,
    /// Where the box and the editor's text were drawn last (for the mouse).
    pub area: Rect,
    pub text_area: Rect,
    /// The value's line breaks are `\r\n`: the editor's lines are joined with them again.
    pub crlf: bool,
}

impl Lines {
    /// The text typed so far, its lines joined as the value's were.
    pub fn text(&self) -> String {
        let text = self.editor.text();
        if self.crlf { text.replace('\n', "\r\n") } else { text }
    }
}

/// Whether `text`'s line breaks are all `\r\n` (some, and no other `\r` or `\n`).
fn crlf(text: &str) -> bool {
    text.contains("\r\n") && !text.replace("\r\n", "").contains(['\r', '\n'])
}

impl App {
    /// The cell editor is open in the active tab, its text in several lines.
    pub fn cell_lines_open(&self) -> bool {
        self.cell_edit_open() && self.tab().cell_edit.as_ref().is_some_and(|e| e.lines.is_some())
    }

    /// Open cell editor `e` in the active tab; with `lines`, `text` in the editor of several
    /// lines (Normal mode, on the first line). A text whose line breaks are not all `\n` or all
    /// `\r\n` is not opened: the editor would not keep them.
    pub(super) fn open_cell_edit(&mut self, e: CellEdit, text: &str, lines: bool) {
        if lines && text.contains('\r') && !crlf(text) {
            let column = rows_of(self.tab()).and_then(|rs| rs.columns.get(e.at.1)).map(|c| c.meta.name.clone());
            return self.say_refused(Msg::EditWhyLineBreaks { column: column.unwrap_or_default() });
        }
        self.tabs.active_mut().cell_edit = Some(e);
        if lines {
            self.cell_edit_to_lines_at(text, false);
        }
    }

    /// `Ctrl+L` in the cell editor: its text so far to the editor of several lines, typing on
    /// at its end.
    pub(super) fn cell_edit_to_lines(&mut self) {
        let text = self.tab().cell_edit.as_ref().map(|e| e.input.text().to_string()).unwrap_or_default();
        self.cell_edit_to_lines_at(&text, true);
    }

    fn cell_edit_to_lines_at(&mut self, text: &str, typing: bool) {
        let lang = Language::Sql(self.tab_dialect(self.tab().id));
        let Some(e) = self.tabs.active_mut().cell_edit.as_mut() else { return };
        let crlf = crlf(text);
        let mut editor = Editor::new(&if crlf { text.replace("\r\n", "\n") } else { text.to_string() });
        editor.set_language(lang);
        if typing {
            editor.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
        }
        e.lines = Some(Box::new(Lines { editor, ex: None, area: Rect::default(), text_area: Rect::default(), crlf }));
    }

    fn cell_lines(&mut self) -> Option<&mut Lines> {
        self.tabs.active_mut().cell_edit.as_mut()?.lines.as_deref_mut()
    }

    /// A key of the editor of several lines: the `:` line's while it is open; else `Enter` (keep)
    /// and `Esc` (cancel) in Normal mode with no command typed, `:` (outside Insert mode) opens
    /// the `:` line; any other key is the editor's.
    pub(crate) fn cell_lines_key(&mut self, key: KeyEvent) {
        let Some(l) = self.cell_lines() else { return };
        if let Some(ex) = l.ex.as_mut() {
            match key.code {
                KeyCode::Esc => l.ex = None,
                KeyCode::Backspace if ex.text().is_empty() => l.ex = None,
                KeyCode::Enter => {
                    let text = ex.text().to_string();
                    l.ex = None;
                    self.cell_lines_ex(&text);
                }
                _ => {
                    ex.handle_key(&key);
                }
            }
            return;
        }
        let e = &mut l.editor;
        let plain = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let idle = e.mode == Mode::Normal && !e.searching() && !e.command_started();
        match key.code {
            KeyCode::Enter if idle && plain => return self.cell_edit_enter(),
            KeyCode::Esc if idle => {
                self.tabs.active_mut().cell_edit = None;
                return;
            }
            KeyCode::Char(':') if e.mode != Mode::Insert && !e.searching() && !e.awaiting_key() => {
                let range = e.cmdline_range();
                l.ex = Some(TextInput::new(&range));
                return;
            }
            _ => {}
        }
        // The system clipboard is read only for the command that puts it (`"+p`).
        if e.reads_clipboard(&key) {
            let text = self.editor_clipboard_text();
            if let Some(l) = self.cell_lines() {
                l.editor.set_clipboard_text(text.as_deref());
            }
        }
        let pairs = self.prefs.auto_pairs == datarig_core::config::AutoPairs::On;
        let soft_tab = self.prefs.soft_tab_backspace == datarig_core::config::SoftTabBackspace::On;
        let Some(l) = self.cell_lines() else { return };
        l.editor.auto_pairs = pairs;
        l.editor.soft_tab_backspace = soft_tab;
        let ev = l.editor.handle_key(key);
        self.editor_said(|a| a.cell_lines().map(|l| &mut l.editor));
        if matches!(ev, EdEvent::Changed { .. })
            && let Some(e) = self.tabs.active_mut().cell_edit.as_mut()
        {
            e.error = None;
        }
    }

    /// Run `text` typed on the `:` line: `w`, `wq` and `x` keep the text, `q` and `q!` cancel;
    /// anything else is an editor command.
    fn cell_lines_ex(&mut self, text: &str) {
        match text.trim() {
            "" => {}
            "w" | "wq" | "x" => self.cell_edit_enter(),
            "q" | "q!" => self.tabs.active_mut().cell_edit = None,
            command => {
                let Some(l) = self.cell_lines() else { return };
                let version = l.editor.version();
                let result = l.editor.ex(command);
                let changed = l.editor.version() != version;
                if changed && let Some(e) = self.tabs.active_mut().cell_edit.as_mut() {
                    e.error = None;
                }
                let msg = match result {
                    Ok(ExDone::Substituted { count, lines }) if count > 2 => {
                        let msg = if lines == 1 {
                            Msg::EditorExSubstitutedOneLine { count: count as u64 }
                        } else {
                            Msg::EditorExSubstituted { count: count as u64, lines: lines.to_string() }
                        };
                        Notice::new(msg, Level::Info)
                    }
                    // The text is not SQL: nothing formats it.
                    Ok(ExDone::Format { .. }) => {
                        Notice::new(Msg::EditorExUnsupported { command: command.to_string() }, Level::Error)
                    }
                    Ok(_) => return,
                    Err(e) => Notice::new(super::super::cmdline::ex_error(e), Level::Error),
                };
                self.flash(msg);
            }
        }
    }

    /// A press at (`x`, `y`) while the editor of several lines is open: outside its box it
    /// closes it, changing nothing, and the press goes on (`false`); inside it is the box's (a
    /// left one on the text moves the cursor there).
    pub(super) fn cell_lines_click(&mut self, x: u16, y: u16, left: bool) -> bool {
        let Some(l) = self.cell_lines() else { return false };
        let within = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        if !within(l.area) {
            self.tabs.active_mut().cell_edit = None;
            return false;
        }
        if left && l.ex.is_none() && within(l.text_area) {
            l.editor.click(x - l.text_area.x, y - l.text_area.y);
        }
        true
    }

    /// Text pasted into the editor of several lines (its line breaks as they are), or into its
    /// `:` line (on one line).
    pub(super) fn cell_lines_paste(&mut self, text: &str) {
        let Some(l) = self.cell_lines() else { return };
        if let Some(ex) = l.ex.as_mut() {
            ex.insert_str(&text.replace(['\r', '\n'], " "));
            return;
        }
        if matches!(l.editor.paste(text), EdEvent::Changed { .. })
            && let Some(e) = self.tabs.active_mut().cell_edit.as_mut()
        {
            e.error = None;
        }
    }
}
