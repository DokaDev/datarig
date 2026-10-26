//! The statements of a Startup SQL form (a preset's, or a profile's own): a list of one-line
//! inputs, one statement per row. A row read from the file keeps that text while it is not
//! edited (its line comments and line breaks too, though it shows them flattened); an edited
//! one is the row's one line. A line break inside a string shows as `⏎` and is one again in
//! what is saved. Each row's check (a statement that cannot be a start-up setting,
//! or one a read-only environment refuses) is kept with it and redone only when the row or the
//! database changes, never while drawing.

use crate::widgets::text_input::TextInput;
use datarig_core::profile::session::{self, Dbms};
use datarig_core::sql::risk::{Classifier, ReadOnlyBlock, SetProblem};
use unicode_segmentation::UnicodeSegmentation;

/// How a row shows a line break inside a string (one line break again when saved).
const NEWLINE: &str = "⏎";

/// What the check of a row says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// Not a start-up setting: the form does not save.
    Refused(SetProblem),
    /// A setting a read-only environment refuses (a warning).
    ReadOnly(ReadOnlyBlock),
}

#[derive(Clone, Debug)]
struct Row {
    input: TextInput,
    /// The statement as the file has it and the text the row showed for it: while the row still
    /// shows that, the statement is the file's.
    original: Option<(String, String)>,
    note: Option<Note>,
}

#[derive(Clone, Debug)]
pub struct StatementList {
    /// Never empty: no statements is one empty row.
    rows: Vec<Row>,
    /// The focused row.
    pub row: usize,
    /// The first line drawn (rows and the lines under them).
    pub scroll: usize,
    /// The next draw scrolls the focused row into view (off after the wheel scrolled).
    pub follow: bool,
    /// The database the statements are for (`None`: none, nothing is checked).
    dbms: Option<Dbms>,
}

impl StatementList {
    /// The statements `sql` of `dbms`, one per row.
    pub fn new(dbms: Option<Dbms>, sql: &[String]) -> Self {
        let mut list = StatementList { rows: Vec::new(), row: 0, scroll: 0, follow: true, dbms };
        for s in sql {
            let input = TextInput::new(&list.display(s));
            let shown = input.text().to_string();
            list.rows.push(Row { input, original: Some((s.clone(), shown)), note: None });
        }
        if list.rows.is_empty() {
            list.rows.push(Row { input: TextInput::default(), original: None, note: None });
        }
        list.check_all();
        list
    }

    /// Statement `s` as one row shows it: on one line, without its line comments, a line break
    /// inside a string as [`NEWLINE`].
    fn display(&self, s: &str) -> String {
        let one = session::join_statements(self.dbms.unwrap_or(Dbms::Postgres), &[s.to_string()]);
        one.replace("\r\n", "\n").replace(['\n', '\r'], NEWLINE)
    }

    /// Check the statements again for `dbms` (the form's database changed).
    pub fn set_dbms(&mut self, dbms: Option<Dbms>) {
        if self.dbms != dbms {
            self.dbms = dbms;
            self.check_all();
        }
    }

    fn check_all(&mut self) {
        for i in 0..self.rows.len() {
            self.check(i);
        }
    }

    fn check(&mut self, i: usize) {
        let note = match (self.dbms, self.statement(i)) {
            (Some(dbms), Some(s)) => match Classifier::session_set(dbms.language(), &s) {
                Err(p) => Some(Note::Refused(p)),
                Ok(set) => set.risk.read_only().err().map(Note::ReadOnly),
            },
            _ => None,
        };
        self.rows[i].note = note;
    }

    /// How many rows it has (one at least).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Row `i`'s input.
    pub fn input_mut(&mut self, i: usize) -> Option<&mut TextInput> {
        self.rows.get_mut(i).map(|r| &mut r.input)
    }

    /// The focused row's input.
    pub fn current_mut(&mut self) -> &mut TextInput {
        let i = self.row.min(self.rows.len() - 1);
        &mut self.rows[i].input
    }

    pub fn text(&self, i: usize) -> &str {
        self.rows.get(i).map_or("", |r| r.input.text())
    }

    pub fn note(&self, i: usize) -> Option<&Note> {
        self.rows.get(i).and_then(|r| r.note.as_ref())
    }

    /// The focused row was edited: its text is its own now, checked again.
    pub fn edited(&mut self) {
        self.row = self.row.min(self.rows.len() - 1);
        self.check(self.row);
        self.follow = true;
    }

    /// Row `i`'s statement: the file's text while the row shows what it showed for it (whatever
    /// the database became since), else the row's text without a closing `;`, each
    /// [`NEWLINE`] a line break (`None`: an empty row).
    fn statement(&self, i: usize) -> Option<String> {
        let r = self.rows.get(i)?;
        if let Some((file, shown)) = &r.original
            && r.input.text() == shown
        {
            return Some(file.clone());
        }
        let t = r.input.text().trim().trim_end_matches(';').trim_end();
        (!t.is_empty()).then(|| t.replace(NEWLINE, "\n"))
    }

    /// The statements, in order (empty rows left out).
    pub fn statements(&self) -> Vec<String> {
        (0..self.rows.len()).filter_map(|i| self.statement(i)).collect()
    }

    /// The rows that hold a statement, with their numbers among the statements (from 1).
    fn numbered(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        (0..self.rows.len()).filter(|&i| self.statement(i).is_some()).enumerate().map(|(n, i)| (n + 1, i))
    }

    /// The first statement that cannot be a start-up setting, and why (`index` from 1).
    pub fn problem(&self) -> Option<(usize, SetProblem)> {
        self.numbered().find_map(|(n, i)| match self.note(i) {
            Some(Note::Refused(p)) => Some((n, p.clone())),
            _ => None,
        })
    }

    /// The first statement a read-only environment refuses (`index` from 1).
    pub fn read_only_refused(&self) -> Option<(usize, ReadOnlyBlock)> {
        self.numbered().find_map(|(n, i)| match self.note(i) {
            Some(Note::ReadOnly(b)) => Some((n, *b)),
            _ => None,
        })
    }

    /// Focus the first row whose statement is refused, if any.
    pub fn focus_problem(&mut self) {
        if let Some(i) = (0..self.rows.len()).find(|&i| matches!(self.note(i), Some(Note::Refused(_)))) {
            self.focus(i);
        }
    }

    /// Focus row `i`.
    pub fn focus(&mut self, i: usize) {
        self.row = i.min(self.rows.len() - 1);
        self.follow = true;
    }

    /// The focus comes into the list: on its first row from above, on its last from below.
    pub fn enter(&mut self, from_above: bool) {
        self.focus(if from_above { 0 } else { self.rows.len() - 1 });
    }

    /// An empty row below the focused one, focused.
    pub fn add_below(&mut self) {
        let at = (self.row + 1).min(self.rows.len());
        self.rows.insert(at, Row { input: TextInput::default(), original: None, note: None });
        self.focus(at);
    }

    /// An empty row at the end, focused.
    pub fn add_last(&mut self) {
        self.row = self.rows.len() - 1;
        self.add_below();
    }

    /// Remove row `i` (the only row is emptied instead). The focus stays on the row it was on;
    /// when that was row `i`, it goes to the row that took its place (the one after it, else the
    /// new last row).
    pub fn remove(&mut self, i: usize) {
        if i >= self.rows.len() {
            return;
        }
        if self.rows.len() == 1 {
            self.rows[0] = Row { input: TextInput::default(), original: None, note: None };
        } else {
            self.rows.remove(i);
        }
        let row = if i < self.row { self.row - 1 } else { self.row };
        self.focus(row);
    }

    /// `Backspace` on the focused row when it is empty and not the only one: it goes, and the
    /// cursor goes to the end of the row before it (the first row: to the row after it, its
    /// cursor where it was). `false`: nothing done (the row's input takes the key).
    pub fn backspace_empty(&mut self) -> bool {
        if !self.text(self.row).is_empty() || self.rows.len() == 1 {
            return false;
        }
        let i = self.row;
        self.rows.remove(i);
        self.focus(i.saturating_sub(1));
        if i > 0 {
            let input = &mut self.rows[i - 1].input;
            let text = input.text().to_string();
            input.set(&text);
        }
        true
    }

    /// `Enter`: a filled row gets an empty one below it (`true`); an empty one goes, unless it is
    /// the only one, and the focus leaves the list (`false`).
    pub fn enter_key(&mut self) -> bool {
        if !self.text(self.row).trim().is_empty() {
            self.add_below();
            return true;
        }
        if self.rows.len() > 1 {
            let i = self.row;
            self.rows.remove(i);
            self.focus(i.min(self.rows.len() - 1));
        }
        false
    }

    /// `↑`/`↓`: the row above or below (`false`: none, the focus leaves the list).
    pub fn step(&mut self, down: bool) -> bool {
        match (down, self.row) {
            (true, r) if r + 1 < self.rows.len() => self.focus(r + 1),
            (false, r) if r > 0 => self.focus(r - 1),
            _ => return false,
        }
        true
    }

    /// Text pasted into the focused row: several statements become rows of their own, after it
    /// (what followed the cursor goes at the end of the last); one goes in at the cursor, on one line.
    pub fn paste(&mut self, text: &str) {
        let dbms = self.dbms.unwrap_or(Dbms::Postgres);
        let parts = session::split_statements(dbms, text);
        if parts.len() < 2 {
            let one = if text.contains(['\n', '\r']) { self.display(text) } else { text.to_string() };
            self.current_mut().insert_str(&one);
            self.edited();
            return;
        }
        let i = self.row.min(self.rows.len() - 1);
        let (head, tail) = {
            let t = self.rows[i].input.text();
            let at: usize = t.graphemes(true).take(self.rows[i].input.cursor()).map(str::len).sum();
            (t[..at].to_string(), t[at..].to_string())
        };
        let shown: Vec<String> = parts.iter().map(|p| self.display(p)).collect();
        let last = shown.len() - 1;
        for (n, s) in shown.iter().enumerate() {
            let text = match n {
                0 if last == 0 => format!("{head}{s}{tail}"),
                0 => format!("{head}{s}"),
                n if n == last => format!("{s}{tail}"),
                _ => s.clone(),
            };
            let row = Row { input: TextInput::new(&text), original: None, note: None };
            if n == 0 {
                self.rows[i] = row;
            } else {
                self.rows.insert(i + n, row);
            }
            self.check(i + n);
        }
        let end = i + last;
        self.focus(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(sql: &[&str]) -> StatementList {
        StatementList::new(Some(Dbms::Postgres), &sql.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn an_unedited_row_keeps_the_files_text_an_edited_one_is_its_line() {
        let mut l = list(&["SET a = 1 -- why\n", "SET b\n  = 2"]);
        assert_eq!((l.text(0), l.text(1)), ("SET a = 1", "SET b = 2"));
        assert_eq!(l.statements(), ["SET a = 1 -- why\n", "SET b\n  = 2"]);
        l.focus(1);
        l.current_mut().insert_str("0;");
        l.edited();
        assert_eq!(l.statements(), ["SET a = 1 -- why\n", "SET b = 20"]);
    }

    /// An unedited row stays the file's after the database changed (PostgreSQL and MySQL read
    /// line comments differently, so the row would show another text for it now).
    #[test]
    fn an_unedited_row_stays_the_files_after_the_database_changed() {
        let file = "SET a = 1 # mysql comment\n".to_string();
        let mut l = StatementList::new(Some(Dbms::MySql), std::slice::from_ref(&file));
        assert_eq!(l.text(0), "SET a = 1");
        l.set_dbms(Some(Dbms::Postgres));
        assert_eq!(l.statements(), [file]);
    }

    /// A line break inside a string shows as `⏎` on the row, and is one again when the row is
    /// edited; unedited, the file's text is kept.
    #[test]
    fn a_line_break_in_a_string_shows_as_a_mark() {
        let file = "SET application_name = 'a\nb'".to_string();
        let mut l = StatementList::new(Some(Dbms::Postgres), std::slice::from_ref(&file));
        assert_eq!(l.text(0), "SET application_name = 'a⏎b'");
        assert_eq!(l.statements(), [file]);
        l.focus(0);
        l.current_mut().insert_str(" ");
        l.edited();
        assert_eq!(l.statements(), ["SET application_name = 'a\nb'"]);
    }

    #[test]
    fn no_statements_is_one_empty_row_and_empty_rows_are_not_statements() {
        let mut l = list(&[]);
        assert_eq!((l.row_count(), l.statements().len()), (1, 0));
        l.current_mut().insert_str("SET a = 1");
        l.edited();
        assert!(l.enter_key(), "a filled row gets one below");
        assert_eq!((l.row_count(), l.row), (2, 1));
        assert_eq!(l.statements(), ["SET a = 1"]);
        assert!(!l.enter_key(), "an empty one goes and the focus leaves");
        assert_eq!((l.row_count(), l.row), (1, 0));
    }

    #[test]
    fn checks_are_kept_per_row_and_numbered_among_the_statements() {
        let mut l = list(&["SET a = 1", "DELETE FROM t", "SET search_path = x"]);
        assert!(matches!(l.note(1), Some(Note::Refused(_))));
        assert!(matches!(l.note(2), Some(Note::Refused(_))));
        assert!(!matches!(l.note(0), Some(Note::Refused(_))), "a setting");
        l.focus(0);
        l.add_below();
        assert_eq!(l.problem().map(|(i, _)| i), Some(2), "the empty row is not counted");
        l.focus_problem();
        assert_eq!(l.row, 2);
    }

    #[test]
    fn a_paste_of_several_statements_makes_rows() {
        let mut l = list(&[]);
        l.paste("SET a = 1;\n-- b\nSET b = 2; SET c = 3");
        assert_eq!(l.statements(), ["SET a = 1", "SET b = 2", "SET c = 3"]);
        assert_eq!(l.row, 2);
        l.paste("SET d = 4");
        assert_eq!(l.text(2), "SET c = 3SET d = 4", "one goes in at the cursor");
    }

    #[test]
    fn rows_are_removed_and_the_focus_stays_near() {
        let mut l = list(&["SET a = 1", "SET b = 2", "SET c = 3"]);
        l.focus(2);
        l.remove(2);
        assert_eq!((l.row_count(), l.row), (2, 1));
        l.remove(0);
        assert_eq!((l.row_count(), l.row, l.text(0)), (1, 0, "SET b = 2"));
        l.remove(0);
        assert_eq!((l.row_count(), l.text(0)), (1, ""), "the only row is emptied");
        assert!(!l.backspace_empty(), "the only row stays");
        l.add_below();
        assert!(l.backspace_empty());
        assert_eq!((l.row_count(), l.row), (1, 0));
        assert!(!l.step(true) && !l.step(false), "one row: the arrows leave");
    }

    #[test]
    fn backspace_on_an_empty_row_puts_the_cursor_at_the_end_of_the_row_before() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        let mut l = list(&["SET a = 1"]);
        l.current_mut().handle_key(&KeyEvent::from(KeyCode::Home));
        l.add_below();
        assert!(l.backspace_empty());
        l.current_mut().insert_str("0");
        assert_eq!(l.text(0), "SET a = 10");
    }
}
