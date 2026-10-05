//! The editor's SQL actions: the formatter (`editor.format`, `:format`) and the comment toggle
//! (`editor.comment_toggle`, the same as `gc`).

use super::*;
use datarig_core::sql::format::{self, Options, Refused};

impl App {
    /// Format the Visual selection or the statement under the cursor, as one undo step. From
    /// Insert mode or the search prompt it first goes back to Normal mode, as `Esc` does.
    pub(super) fn format_sql(&mut self) {
        for _ in 0..3 {
            let e = &self.tab().editor;
            if matches!(e.mode, Mode::Normal | Mode::Visual)
                && !e.awaiting_key()
                && self.key_context() != Ctx::VimSearch
            {
                break;
            }
            self.editor_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        }
        let t = self.tab_mut();
        t.popup = None;
        t.completion_due = None;
        match t.editor.sql_target() {
            Some((a, b)) => self.format_span(a, b),
            None => self.flash(Notice::new(Label::EditorFormatNothing, Level::Info)),
        }
    }

    /// `:{range}format`: lines `first..=last`.
    pub(super) fn format_lines(&mut self, first: usize, last: usize) {
        let (a, b) = self.tab().editor.lines_target(first, last);
        self.format_span(a, b);
    }

    /// Format bytes `a..b` of the active tab's text: replaced only when the formatter changed
    /// nothing but the layout; otherwise the text stays and a notice says where it stopped. A
    /// selection that starts or ends inside a token (in a comment, a string, a dollar body) is
    /// not formatted: its text means something else on its own.
    fn format_span(&mut self, a: usize, b: usize) {
        let editor = &mut self.tab_mut().editor;
        if let Some(off) = [a, b].into_iter().find(|&off| editor.splits_token(off)) {
            let line = editor.line_of(off) + 1;
            return self.flash(Notice::new(Msg::EditorFormatRefused { line: line.to_string() }, Level::Warning));
        }
        let opts = Options { case: self.prefs.format_case, indent: self.prefs.format_indent.spaces() };
        let editor = &self.tab().editor;
        let text = editor.text_between(a, b);
        let lead = text.len() - text.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']).len();
        let indent = editor.indent_before(a + lead);
        let (msg, level) = match format::format(&text, opts, &indent) {
            Ok((span, _)) if span.is_empty() => (Msg::Label(Label::EditorFormatNothing), Level::Info),
            Ok((span, out)) if text[span.clone()] == out => (Msg::Label(Label::EditorFormatUnchanged), Level::Info),
            Ok((span, out)) => {
                self.tab_mut().editor.replace_with(a + span.start, a + span.end, &out);
                self.edited();
                (Msg::Label(Label::EditorFormatDone), Level::Info)
            }
            Err(Refused::Changed { at }) => {
                let line = self.tab().editor.line_of(a + at) + 1;
                (Msg::EditorFormatRefused { line: line.to_string() }, Level::Warning)
            }
        };
        self.flash(Notice::new(msg, level));
    }

    /// `gc` on the Visual selection, else `gcc` on the cursor's line.
    pub(super) fn toggle_comment(&mut self) {
        let t = self.tab_mut();
        t.popup = None;
        t.completion_due = None;
        if matches!(t.editor.comment_toggle(), EdEvent::Changed { .. }) {
            self.edited();
        }
    }
}
