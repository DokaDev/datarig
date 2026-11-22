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
    /// Made by paste `.0` of [`StatementList::pastes`], with the text `.1` it was given then.
    pasted: Option<(usize, String)>,
}

/// Text pasted as rows of their own, kept so that the rows are split again when the database
/// kind changes (while they are as the paste made them).
#[derive(Clone, Debug)]
struct Paste {
    raw: String,
    /// On one line: what the focused row had before and after the cursor.
    head: String,
    tail: String,
    /// A text with a line break: rows of its own.
    lines: bool,
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
    pastes: Vec<Paste>,
}

impl StatementList {
    /// The statements `sql` of `dbms`, one per row.
    pub fn new(dbms: Option<Dbms>, sql: &[String]) -> Self {
        let mut list = StatementList { rows: Vec::new(), row: 0, scroll: 0, follow: true, dbms, pastes: Vec::new() };
        for s in sql {
            let input = TextInput::new(&list.display(s));
            let shown = input.text().to_string();
            list.rows.push(Row { input, original: Some((s.clone(), shown)), note: None, pasted: None });
        }
        if list.rows.is_empty() {
            list.rows.push(Row { input: TextInput::default(), original: None, note: None, pasted: None });
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

    /// Check the statements again for `dbms` (the form's database changed). Pasted rows are
    /// split again when the database kind changes.
    pub fn set_dbms(&mut self, dbms: Option<Dbms>) {
        if self.dbms != dbms {
            let other = self.dbms.unwrap_or(Dbms::Postgres) != dbms.unwrap_or(Dbms::Postgres);
            self.dbms = dbms;
            if other {
                self.split_pastes_again();
            }
            self.check_all();
        }
    }

    /// The rows of each paste whose rows are all still as it made them, split again as the
    /// form's database reads it.
    fn split_pastes_again(&mut self) {
        for id in 0..self.pastes.len() {
            let at: Vec<usize> =
                (0..self.rows.len()).filter(|&i| self.rows[i].pasted.as_ref().is_some_and(|p| p.0 == id)).collect();
            let (Some(&first), Some(&last)) = (at.first(), at.last()) else { continue };
            let whole = last - first + 1 == at.len();
            if !whole
                || !at.iter().all(|&i| self.rows[i].pasted.as_ref().is_some_and(|p| p.1 == self.rows[i].input.text()))
            {
                continue;
            }
            let rows = self.paste_rows(&self.pastes[id].clone());
            let made: Vec<Row> = rows
                .into_iter()
                .map(|t| Row { input: TextInput::new(&t), original: None, note: None, pasted: Some((id, t)) })
                .collect();
            self.rows.splice(first..=last, made);
        }
        if self.rows.is_empty() {
            self.rows.push(Row { input: TextInput::default(), original: None, note: None, pasted: None });
        }
        self.row = self.row.min(self.rows.len() - 1);
    }

    /// The statements of pasted text `text`, each as a row shows it.
    fn parts_of(&self, text: &str) -> Vec<String> {
        let dbms = self.dbms.unwrap_or(Dbms::Postgres);
        session::split_statements(dbms, text)
            .iter()
            .flat_map(|p| self.split_lines(p))
            .map(|p| self.display(&p))
            .filter(|p| !p.is_empty())
            .collect()
    }

    /// The rows paste `p` makes.
    fn paste_rows(&self, p: &Paste) -> Vec<String> {
        let parts = self.parts_of(&p.raw);
        if p.lines {
            return parts;
        }
        if parts.len() < 2 {
            return vec![format!("{}{}{}", p.head, p.raw, p.tail)];
        }
        let last = parts.len() - 1;
        parts
            .iter()
            .enumerate()
            .map(|(n, s)| match n {
                0 => format!("{}{s}", p.head),
                n if n == last => format!("{s}{}", p.tail),
                _ => s.clone(),
            })
            .collect()
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
                Ok(set) => set.read_only().err().map(Note::ReadOnly),
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
            Some(Note::ReadOnly(b)) => Some((n, b.clone())),
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
        self.rows.insert(at, Row { input: TextInput::default(), original: None, note: None, pasted: None });
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
            self.rows[0] = Row { input: TextInput::default(), original: None, note: None, pasted: None };
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

    /// Statement text `text` cut before each line that starts with `SET` (outside strings and
    /// comments): a statement of its own, which no `;` ended.
    fn split_lines(&self, text: &str) -> Vec<String> {
        use datarig_core::sql::lexer::{Tok, lex_in};
        let dialect = self.dbms.unwrap_or(Dbms::Postgres).language().dialect();
        let mut cuts = vec![0];
        let mut line_start = false;
        // Inside an executable comment (`/*! … */`): its lines are one piece of the statement.
        let mut exec = false;
        for t in lex_in(text, dialect) {
            match t.kind {
                Tok::Whitespace => line_start |= t.text(text).contains(['\n', '\r']),
                Tok::LineComment | Tok::BlockComment => {}
                Tok::ExecComment => {
                    exec = t.text(text).starts_with("/*");
                    line_start = false;
                }
                _ => {
                    if line_start && !exec && t.start > 0 && t.is_word() && t.text(text).eq_ignore_ascii_case("SET") {
                        cuts.push(t.start);
                    }
                    line_start = false;
                }
            }
        }
        cuts.push(text.len());
        cuts.windows(2).map(|w| text[w[0]..w[1]].trim().to_string()).filter(|p| !p.is_empty()).collect()
    }

    /// Text pasted into the focused row. Its statements are those `;` ends, and each line that
    /// starts with `SET` starts one too. A text with a line break is rows of its own: in the
    /// focused row when that is empty, else after it. On one line, several statements become rows
    /// after the focused one (what followed the cursor goes at the end of the last); one goes in
    /// at the cursor.
    pub fn paste(&mut self, text: &str) {
        let parts = self.parts_of(text);
        if text.contains(['\n', '\r']) {
            if parts.is_empty() {
                return;
            }
            let id = self.pastes.len();
            self.pastes.push(Paste { raw: text.to_string(), head: String::new(), tail: String::new(), lines: true });
            let i = self.row.min(self.rows.len() - 1);
            let mut at = if self.text(i).trim().is_empty() {
                self.rows.remove(i);
                i
            } else {
                i + 1
            };
            for p in parts {
                let row = Row { input: TextInput::new(&p), original: None, note: None, pasted: Some((id, p)) };
                self.rows.insert(at, row);
                self.check(at);
                at += 1;
            }
            self.focus(at - 1);
            return;
        }
        if parts.len() < 2 {
            self.current_mut().insert_str(text);
            self.edited();
            return;
        }
        let i = self.row.min(self.rows.len() - 1);
        let (head, tail) = {
            let t = self.rows[i].input.text();
            let at: usize = t.graphemes(true).take(self.rows[i].input.cursor()).map(str::len).sum();
            (t[..at].to_string(), t[at..].to_string())
        };
        let id = self.pastes.len();
        let paste = Paste { raw: text.to_string(), head, tail, lines: false };
        let rows = self.paste_rows(&paste);
        self.pastes.push(paste);
        let last = rows.len() - 1;
        for (n, text) in rows.into_iter().enumerate() {
            let row = Row { input: TextInput::new(&text), original: None, note: None, pasted: Some((id, text)) };
            if n == 0 {
                self.rows[i] = row;
            } else {
                self.rows.insert(i + n, row);
            }
            self.check(i + n);
        }
        self.focus(i + last);
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

    /// Lines that each start with `SET` are statements of their own without a `;`; a pasted
    /// block (a text with a line break) never joins the row it lands on.
    #[test]
    fn a_paste_of_lines_makes_a_row_of_each() {
        let mut l = StatementList::new(Some(Dbms::MySql), &[]);
        l.paste("SET @c=3\nSET @d=4\n");
        assert_eq!(l.statements(), ["SET @c=3", "SET @d=4"]);
        // A statement on two lines stays one; a SET in a string starts nothing.
        let mut l = StatementList::new(Some(Dbms::MySql), &[]);
        l.paste("SET @a = 1,\n  @b = 2\nSET @c = 'x\nSET y'");
        assert_eq!(l.statements(), ["SET @a = 1, @b = 2", "SET @c = 'x\nSET y'"]);
        let mut l = StatementList::new(Some(Dbms::MySql), &["SET autocommit = 0".to_string()]);
        l.paste("\nSET @x1 = 1");
        assert_eq!(l.statements(), ["SET autocommit = 0", "SET @x1 = 1"]);
        assert_eq!(l.row, 1);
    }

    /// A pasted statement keeps the executable comments it starts or ends with (`/*! … */`,
    /// `/*M! … */`, versioned too), and one that is only such a comment is a row: the server
    /// may run what they hold. Also before the form knows its database.
    #[test]
    fn a_paste_keeps_executable_comments() {
        for dbms in [Some(Dbms::MySql), None] {
            let mut l = StatementList::new(dbms, &[]);
            l.paste(
                "SET @a = 1 /*M! , @b = 2 */;\n/*M!100500 SET @c = 3 */;\n/*! SET @d = 4 */ ;\n\
                 /*M! SET @e = 5 */ SET @f = 6;\nSET @g = 7 /*!80000 , @h = 8 */",
            );
            assert_eq!(
                l.statements(),
                [
                    "SET @a = 1 /*M! , @b = 2 */",
                    "/*M!100500 SET @c = 3 */",
                    "/*! SET @d = 4 */",
                    "/*M! SET @e = 5 */ SET @f = 6",
                    "SET @g = 7 /*!80000 , @h = 8 */",
                ],
                "{dbms:?}"
            );
        }
    }

    /// Rows pasted before the form has a database, or for another kind, are split again when
    /// the database kind changes, as long as they are as the paste made them.
    #[test]
    fn pasted_rows_are_split_again_when_the_database_kind_changes() {
        for first in [None, Some(Dbms::Postgres)] {
            for (text, want) in [
                ("SET @a = 5--1\n/*M! , @b = 2 */", vec!["SET @a = 5--1 /*M! , @b = 2 */"]),
                ("SET @a = 5--1\nSET @b = 2", vec!["SET @a = 5--1", "SET @b = 2"]),
                ("SET @a = 1 /* x /* y */;\nSET @b = 2", vec!["SET @a = 1", "SET @b = 2"]),
                (
                    "SET @a = 1 /* x /* y */ /*M! , @b = 2 */;\nSET @c = 3",
                    vec!["SET @a = 1 /* x /* y */ /*M! , @b = 2 */", "SET @c = 3"],
                ),
                ("SET @a = 1; SET @b = 5--1", vec!["SET @a = 1", "SET @b = 5--1"]),
            ] {
                let mut l = StatementList::new(first, &[]);
                l.paste(text);
                l.set_dbms(Some(Dbms::MySql));
                assert_eq!(l.statements(), want, "{first:?} {text:?}");
                let mut direct = StatementList::new(Some(Dbms::MySql), &[]);
                direct.paste(text);
                assert_eq!(direct.statements(), want, "pasted for MySQL: {text:?}");
            }
        }
        // A row edited since is the user's: it stays as it is.
        let mut l = StatementList::new(None, &[]);
        l.paste("SET @a = 1 /* x /* y */;\nSET @b = 2");
        l.current_mut().insert_str(" ");
        l.edited();
        l.set_dbms(Some(Dbms::MySql));
        assert_eq!(l.statements(), ["SET @a = 1"]);
    }

    /// A line inside an executable comment that starts with `SET` starts no row: the comment is
    /// one piece of its statement.
    #[test]
    fn a_paste_keeps_an_executable_comment_of_several_lines_in_one_row() {
        let mut l = StatementList::new(Some(Dbms::MySql), &[]);
        l.paste("/*!40101\nSET @a = 1 */;\nSET @b = 2 /*!50000\nSET @c = 3 */\nSET @d = 4");
        assert_eq!(l.statements(), ["/*!40101 SET @a = 1 */", "SET @b = 2 /*!50000 SET @c = 3 */", "SET @d = 4"]);
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
