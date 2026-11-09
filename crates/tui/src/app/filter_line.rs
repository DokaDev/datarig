//! The filter line of a table tab, its fields and keys: `f` (or `Space r f`) edits the
//! condition, `Space r o` the order, `Space r x` clears both and runs the tab again, and
//! `Space r v` opens the statement in a new console.
//!
//! In a field `Enter` (or the run key) applies what both fields hold: the order is read into
//! keys first (refused and said when it is not a list of the table's columns), then the query
//! runs as any run of the tab does ([`App::table_run`]); a refusal leaves the focus on the
//! field it is about, else the keys go back to the grid. `Esc` puts back what is applied and
//! leaves, `Tab` and `Shift+Tab` move between the fields, `Ctrl+L` empties the field (applied
//! with `Enter`), `Ctrl+W` deletes a word, `Ctrl+U` the text before the cursor.
//!
//! Completion in the condition is the editor's ([`App::complete_text`]) on the statement the
//! condition goes into (`SELECT * FROM <table> WHERE <condition>`): the table's columns,
//! operators and keywords, functions, the values of an enumerated type or of a check inside a
//! literal; never values read from the table. In the order: the table's columns, then `ASC`,
//! `DESC` (and `NULLS FIRST`, `NULLS LAST` where the server has them). `Ctrl+Space` asks for it,
//! typing opens it after `[editor] completion_delay_ms` as in the editor. `:where` and
//! `:orderby` complete their argument the same way.

use super::*;
use crate::widgets::filter_line::{Applied, FIELD_CAP};
use crate::widgets::text_input::InputResult;
use datarig_core::query::{self, Filter, KeyOrigin, Query, Target};
use datarig_core::sql::complete::{Candidate, Completion, MAX_ITEMS};
use datarig_core::sql::lexer::{LexState, Tok, lex_in};

/// What the table keys of the grid do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAction {
    /// Focus the condition (`f`).
    EditFilter,
    /// Focus the order.
    EditOrder,
    /// No condition, no order, and a run.
    Clear,
    /// The statement of the rows shown (else of the draft) in a new console.
    OpenSql,
}

impl App {
    /// A field of the active table tab's filter line has the keys.
    pub fn filter_line_focused(&self) -> bool {
        self.focus == Focus::Results && !self.tabs.is_empty() && self.filter_line_field().is_some()
    }

    /// The field of the active table tab's filter line that has the focus, if one does.
    pub(crate) fn filter_line_field(&self) -> Option<table_query::Field> {
        let t = self.tab();
        t.is_table().then(|| t.doc.table_query.as_ref().and_then(|q| q.line.focus)).flatten()
    }

    /// What table tab `id` holds as applied, as its filter line shows it.
    pub(crate) fn filter_line_applied(&self, id: TabId) -> Applied {
        let dialect = self.tab_dialect(id);
        let Some(q) = self.table_query(id).map(|tq| tq.applied().clone()) else {
            return Applied {
                filter: String::new(),
                order: String::new(),
                keys: Vec::new(),
                tie: None,
                dialect,
                failed: false,
                mark: None,
            };
        };
        let keys: Vec<_> = q.order.iter().filter(|k| k.origin == KeyOrigin::User).cloned().collect();
        // As the statement of what is applied was sent (nothing said before one is): a
        // structure read since changes the next statement, not what this one read.
        let run = self.table_query(id).and_then(|tq| tq.applied_run());
        let tie = run.filter(|_| !keys.is_empty()).map(|run| {
            let ties: Vec<&str> =
                run.order.iter().filter(|k| k.origin == KeyOrigin::TieBreak).map(|k| k.col.as_str()).collect();
            match (ties.is_empty(), run.total) {
                (false, _) => format!("+ {}", ties.join(", ")),
                (true, true) => String::new(),
                (true, false) => self.i18n.label(Label::TableOrderNotUnique).to_string(),
            }
        });
        let filter = table_query::filter_text(&q).unwrap_or_default().to_string();
        let tq = self.table_query(id);
        // Our refusal of what a field holds, else where the server said the failed filter is
        // wrong, while the field still holds that text.
        let mark = tq.and_then(|tq| {
            let refused = tq.refusal.as_ref().and_then(|r| {
                let at = r.at.clone().filter(|_| tq.line.field(r.field).text() == r.text)?;
                Some((r.field, at))
            });
            refused.or_else(|| {
                let at = tq.failure()?.at.clone().filter(|_| tq.line.where_.text() == filter)?;
                Some((table_query::Field::Where, at))
            })
        });
        Applied {
            failed: tq.is_some_and(|tq| tq.failure().is_some()),
            mark,
            filter,
            order: query::order_text(&keys, dialect),
            keys,
            tie: tie.filter(|t| !t.is_empty()),
            dialect,
        }
    }

    /// The fields of the active table tab follow what it holds as applied where they are not
    /// edited (before they are drawn or read).
    pub(crate) fn filter_line_follow(&mut self) {
        if self.tabs.is_empty() || !self.tab().is_table() {
            return;
        }
        self.filter_line_follow_tab(self.tab().id);
    }

    /// [`App::filter_line_follow`] for table tab `id` (before and after what is applied
    /// changes: a field that held the applied text follows it).
    pub(super) fn filter_line_follow_tab(&mut self, id: TabId) {
        if self.table_query(id).is_none() {
            return;
        }
        let a = self.filter_line_applied(id);
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.follow((&a.filter, &a.order));
        }
    }

    /// The table keys of the grid.
    pub(super) fn table_action(&mut self, a: TableAction) {
        let id = self.tab().id;
        match a {
            TableAction::EditFilter => self.filter_line_focus(id, table_query::Field::Where),
            TableAction::EditOrder => self.filter_line_focus(id, table_query::Field::Order),
            TableAction::Clear => {
                let Some(name) = self.table_name(id) else { return };
                let Some(tq) = self.table_query_mut(id) else { return };
                tq.line.focus = None;
                tq.draft = Query::table(name);
                let q = tq.draft.clone();
                self.table_run(id, q);
            }
            TableAction::OpenSql => self.table_open_sql(id),
        }
    }

    /// Give field `f` of table tab `id`'s filter line the keys, the cursor at its end.
    fn filter_line_focus(&mut self, id: TabId, f: table_query::Field) {
        self.close_search_prompt();
        self.filter_popup_close();
        self.filter_line_follow();
        self.focus = Focus::Results;
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.focus = Some(f);
            let input = tq.line.field_mut(f);
            let text = input.text().to_string();
            input.set(&text);
        }
    }

    /// The grid takes the keys back; what the fields hold stays.
    pub(super) fn filter_line_blur(&mut self) {
        if self.tabs.is_empty() || !self.tab().is_table() {
            return;
        }
        let id = self.tab().id;
        self.filter_popup_close();
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.focus = None;
        }
    }

    /// A key while a field of the filter line has the focus.
    pub(super) fn filter_line_key(&mut self, key: KeyEvent) {
        use table_query::Field;
        let id = self.tab().id;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => return self.filter_line_apply(),
            KeyCode::Esc => return self.filter_line_cancel(),
            _ => {}
        }
        let Some(tq) = self.table_query_mut(id) else { return };
        let Some(f) = tq.line.focus else { return };
        // (changed, the character typed); `None`: no edit.
        let edit = match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                let next = if f == Field::Where { Field::Order } else { Field::Where };
                tq.line.focus = Some(next);
                Some((false, None))
            }
            KeyCode::Char('w') if ctrl => {
                (tq.line.field_mut(f).delete_word_back() == InputResult::Changed).then_some((true, None))
            }
            KeyCode::Char('l') if ctrl => {
                tq.line.field_mut(f).set("");
                Some((false, None))
            }
            _ => match tq.line.field_mut(f).handle_key(&key) {
                InputResult::Changed => Some((true, if let KeyCode::Char(c) = key.code { Some(c) } else { None })),
                InputResult::Moved => Some((false, None)),
                InputResult::Ignored => None,
            },
        };
        if let Some((changed, typed)) = edit {
            self.filter_typed(changed, typed);
        }
    }

    /// The popup of the filter line closes, and no completion is due.
    fn filter_popup_close(&mut self) {
        let t = self.tab_mut();
        if t.popup.as_ref().is_some_and(|p| matches!(p.owner, PopupOwner::Filter(_))) {
            t.popup = None;
            t.values_wait = None;
        }
        t.completion_due = None;
    }

    /// Text pasted into the focused field: on one line, the field holding at most
    /// [`FIELD_CAP`] bytes (the rest is left out, and said).
    pub(super) fn filter_line_paste(&mut self, text: &str) {
        let id = self.tab().id;
        self.filter_popup_close();
        let Some(tq) = self.table_query_mut(id) else { return };
        let Some(f) = tq.line.focus else { return };
        let input = tq.line.field_mut(f);
        let room = FIELD_CAP.saturating_sub(input.text().len());
        let mut end = text.len().min(room);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        input.insert_str(&text[..end]);
        if end < text.len() {
            self.flash(Notice::new(Label::TableFilterPasteCut, Level::Warning));
        }
    }

    /// `Esc` in a field: both fields back to what is applied, and the query the next run
    /// sends with them (an apply that was refused, or did not run, set it), the keys back to
    /// the grid.
    fn filter_line_cancel(&mut self) {
        let id = self.tab().id;
        self.filter_popup_close();
        let a = self.filter_line_applied(id);
        if let Some(tq) = self.table_query_mut(id) {
            let applied = tq.applied().clone();
            tq.draft.filter = applied.filter;
            tq.draft.order = applied.order;
            tq.line.restore((&a.filter, &a.order));
            tq.line.focus = None;
            tq.refusal = None;
        }
    }

    /// `Enter` (or the run key) in a field: the fields' query runs. The order is read into
    /// keys first and written back as the app writes it; a refused order or filter keeps the
    /// focus on its field, else the grid takes the keys back.
    pub(super) fn filter_line_apply(&mut self) {
        use table_query::{Field, Refusal};
        let id = self.tab().id;
        self.filter_popup_close();
        let Some(target) = self.table_target(id) else { return };
        let d = self.tab_dialect(id);
        let Some(tq) = self.table_query_mut(id) else { return };
        let (filter, order) = (tq.line.where_.text().to_string(), tq.line.order.text().to_string());
        let keys = if order.trim().is_empty() {
            Ok(Vec::new())
        } else if target.columns.is_empty() {
            // Unknown, not absent: no column is said not to exist.
            Err((None, Notice::new(Label::TableOrderColumnsUnknown, Level::Warning)))
        } else {
            query::parse_order(&order, &target).map_err(|e| {
                let why = table_query::order_why(&self.i18n, &e);
                (Some(e.range), Notice::new(Msg::TableOrderRefused { why }, Level::Warning))
            })
        };
        let Some(tq) = self.table_query_mut(id) else { return };
        let keys = match keys {
            Ok(keys) => keys,
            Err((at, notice)) => {
                tq.refusal = Some(Refusal { field: Field::Order, at, notice: notice.clone(), text: order });
                tq.line.focus = Some(Field::Order);
                return self.tab_status(id, notice);
            }
        };
        tq.line.order.set(&query::order_text(&keys, d));
        let filter = (!filter.trim().is_empty())
            .then(|| Filter { recognised: query::recognise(&filter, &target), text: filter });
        tq.draft = Query { filter, order: keys, ..tq.draft.clone() };
        tq.refusal = None;
        tq.line.focus = None;
        let q = tq.draft.clone();
        self.table_run(id, q);
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.focus = tq.refusal.as_ref().map(|r| r.field);
        }
    }

    /// A click on the filter line at (x, y): a field takes the focus with the cursor under the
    /// pointer; the refresh mark runs what is applied again.
    pub(super) fn filter_line_click(&mut self, x: u16, y: u16) {
        use table_query::Field;
        let l = self.layout;
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let id = self.tab().id;
        if inside(l.filter_refresh) {
            self.filter_line_blur();
            let Some(q) = self.table_query_mut(id).map(|tq| tq.applied().clone()) else { return };
            return self.table_run(id, q);
        }
        let f = if inside(l.filter_order) {
            Field::Order
        } else if inside(l.filter_where) {
            Field::Where
        } else {
            return;
        };
        self.filter_line_focus(id, f);
        if let Some(tq) = self.table_query_mut(id) {
            tq.line.field_mut(f).click(x, y);
        }
    }

    /// Completion of `text` (the cursor at byte `cursor`) as field `f` of table tab `id`'s
    /// filter line holds it, offsets in `text`: the condition through the completion engine
    /// on the statement it goes into, the order from the table's columns.
    pub(super) fn filter_completion(
        &mut self,
        id: TabId,
        f: table_query::Field,
        text: &str,
        cursor: usize,
        force: bool,
    ) -> datarig_core::sql::complete::Outcome {
        let mut out = datarig_core::sql::complete::Outcome::default();
        let (Some(name), Some(target)) = (self.table_name(id), self.table_target(id)) else { return out };
        match f {
            table_query::Field::Where => {
                // The statement as the run writes it, up to the condition.
                let head = format!("{} WHERE ", query::compile(&Query::table(name), &target).sql);
                let full =
                    self.complete_text(&format!("{head}{text}"), head.len() + cursor, LexState::default(), force);
                out.completion = full.completion.filter(|c| c.replace_start >= head.len()).map(|mut c| {
                    c.replace_start -= head.len();
                    c
                });
                out.wants_checks = full.wants_checks.filter(|w| w.literal >= head.len()).map(|mut w| {
                    w.literal -= head.len();
                    w
                });
            }
            table_query::Field::Order => out.completion = order_completion(text, cursor, &target, force),
        }
        out
    }

    /// Completion at the cursor of the focused field of the active table tab's filter line.
    /// `literal`: the checks' values came late, and the popup shows only if it offers values
    /// for the literal whose value starts there.
    pub(super) fn filter_complete(&mut self, force: bool, literal: Option<usize>) {
        let Some(f) = self.filter_line_field() else { return };
        let id = self.tab().id;
        let Some(input) = self.table_query(id).map(|q| q.line.field(f)) else { return };
        let (text, cursor) = (input.text().to_string(), input.cursor_byte());
        let out = self.filter_completion(id, f, &text, cursor, force);
        let popup = out.completion.map(|c| Popup {
            items: c.items,
            selected: 0,
            replace_start: c.replace_start,
            trail: c.trail,
            owner: PopupOwner::Filter(f),
        });
        self.completed(popup, literal);
        if let Some(w) = out.wants_checks {
            self.ask_checks(w.schema, w.table, w.literal, PopupOwner::Filter(f));
        }
    }

    /// A key changed or moved through the focused field (`typed`: the character it typed): an
    /// open popup follows the text, a character that starts a name, a qualified name or a
    /// literal schedules one (`[editor] completion_delay_ms`, as in the editor), anything else
    /// closes it.
    fn filter_typed(&mut self, changed: bool, typed: Option<char>) {
        let open = self.tab().popup.as_ref().is_some_and(|p| matches!(p.owner, PopupOwner::Filter(_)));
        let d = self.tab_dialect(self.tab().id);
        let values = self
            .tab()
            .popup
            .as_ref()
            .is_some_and(|p| p.items.first().is_some_and(|c| c.kind == datarig_core::sql::complete::Kind::Value));
        let starts = |c: char| is_ident_char(c) || c == '.' || d.string_quotes().contains(&c);
        match typed {
            _ if !changed => {
                let t = self.tab_mut();
                t.popup = None;
                t.completion_due = None;
                t.values_wait = None;
            }
            Some(c) if open && (is_ident_char(c) || values) => self.filter_complete(false, None),
            Some(c) if starts(c) => {
                let pending = is_ident_char(c) && self.tab().completion_due.is_some();
                let prefix = self.filter_word_before().chars().count();
                if c == '.' || !is_ident_char(c) || pending || prefix >= COMPLETION_MIN_PREFIX {
                    let delay = Duration::from_millis(self.prefs.completion_delay.millis());
                    if delay.is_zero() {
                        self.tab_mut().completion_due = None;
                        self.filter_complete(false, None);
                    } else {
                        self.tab_mut().completion_due = Some(self.now() + delay);
                    }
                } else {
                    self.tab_mut().completion_due = None;
                }
            }
            // A deletion: an open popup follows it.
            None if open => {
                self.tab_mut().completion_due = None;
                self.filter_complete(false, None);
            }
            _ => {
                let t = self.tab_mut();
                t.popup = None;
                t.completion_due = None;
            }
        }
    }

    /// The name characters right before the cursor of the focused field.
    fn filter_word_before(&self) -> String {
        let Some(f) = self.filter_line_field() else { return String::new() };
        let Some(input) = self.table_query(self.tab().id).map(|q| q.line.field(f)) else { return String::new() };
        let before = &input.text()[..input.cursor_byte()];
        let start =
            before.char_indices().rev().take_while(|(_, c)| is_ident_char(*c)).last().map_or(before.len(), |(i, _)| i);
        before[start..].to_string()
    }
}

/// Completion of an `ORDER BY` text (the cursor at byte `cursor`) from `target`'s columns: a
/// column at the start or after a comma, then its direction (`ASC`, `DESC`, and on PostgreSQL
/// `NULLS FIRST`, `NULLS LAST`), after a direction `NULLS FIRST`/`NULLS LAST`, after `NULLS` its
/// two words. Names are written as the dialect reads them back (quoted only when needed); a
/// name being typed in quotes is completed in quotes. `None` when nothing is typed and `force`
/// is not set, or nothing fits.
pub fn order_completion(text: &str, cursor: usize, target: &Target, force: bool) -> Option<Completion> {
    let d = target.dialect;
    let toks = lex_in(text, d);
    let at = toks.iter().find(|t| t.start < cursor && cursor <= t.end && (t.is_word() || t.kind == Tok::QuotedIdent));
    let (start, typed, quoted) = match at {
        Some(t) if t.kind == Tok::QuotedIdent => {
            let quote = d.opening_quote(t.text(text)).unwrap_or(d.ident_quote());
            (t.start, text[t.start + quote.len_utf8()..cursor].to_string(), true)
        }
        Some(t) => (t.start, text[t.start..cursor].to_string(), false),
        None => (cursor, String::new(), false),
    };
    if typed.is_empty() && !force && !quoted {
        return None;
    }
    let before: Vec<&datarig_core::sql::lexer::Token> =
        toks.iter().filter(|t| t.end <= start && !t.is_trivia()).collect();
    let last = before.last().map(|t| (t.kind, t.text(text).to_ascii_uppercase()));
    let pg = matches!(d, Dialect::Postgres);
    let words: &[&str] = match &last {
        None | Some((Tok::Comma, _)) => &[],
        Some((Tok::Keyword | Tok::Ident, w)) if w == "NULLS" => &["FIRST", "LAST"],
        Some((Tok::Keyword | Tok::Ident, w)) if w == "FIRST" || w == "LAST" => return None,
        Some((Tok::Keyword | Tok::Ident, w)) if (w == "ASC" || w == "DESC") && pg => &["NULLS FIRST", "NULLS LAST"],
        Some((Tok::Keyword | Tok::Ident, w)) if w == "ASC" || w == "DESC" => return None,
        Some((Tok::Keyword | Tok::Ident | Tok::QuotedIdent, _)) if pg => &["ASC", "DESC", "NULLS FIRST", "NULLS LAST"],
        Some((Tok::Keyword | Tok::Ident | Tok::QuotedIdent, _)) => &["ASC", "DESC"],
        _ => return None,
    };
    let column_place = matches!(last, None | Some((Tok::Comma, _)));
    let mut items: Vec<Candidate> = Vec::new();
    if column_place {
        let lower = typed.to_lowercase();
        let fits = |n: &str| if quoted { n.starts_with(&typed) } else { n.to_lowercase().starts_with(&lower) };
        for c in target.columns.iter().filter(|c| fits(&c.name)) {
            let label = if quoted { d.force_quote_ident(&c.name) } else { d.quote_ident(&c.name) };
            items.push(Candidate {
                label,
                kind: datarig_core::sql::complete::Kind::Column,
                detail: None,
                insert: None,
            });
        }
    } else if !quoted {
        let upper = typed.to_ascii_uppercase();
        for w in words.iter().filter(|w| w.starts_with(&upper)) {
            items.push(Candidate {
                label: w.to_string(),
                kind: datarig_core::sql::complete::Kind::Keyword,
                detail: None,
                insert: None,
            });
        }
    }
    items.truncate(MAX_ITEMS);
    // A quoted name typed up to its closing quote: the label brings that quote itself.
    let trail = match at {
        Some(t) if quoted && t.end > cursor => t.end - cursor,
        _ => 0,
    };
    (!items.is_empty()).then_some(Completion { items, replace_start: start, trail })
}
