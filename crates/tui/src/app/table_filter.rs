//! The table keys of a table tab's grid: `f` (or `Space r f`) opens the filter and sort dialog
//! on the cursor's column, `Space r o` opens it on the order, `Space r x` clears the condition,
//! the order and the limit and runs the tab again, `X` clears the condition and the order (the
//! limit kept), and `Space r v` opens the statement in a new console. `=` and `!` filter by the
//! cursor's value ([`App::grid_pick`]), `F` opens the column's menu ([`App::open_column_menu`]). `s` and `S` (`Space r s`, `Space r S`) sort by the grid's column
//! ([`App::table_sort`]). `Space r H` (`Ctrl+R` in the dialog) lists the filters this table ran
//! before ([`App::open_filter_history`]): one picked goes into the dialog's draft, and runs only
//! when the dialog applies it, checked as any text typed.
//!
//! Completion of a condition is the editor's ([`App::complete_text`]) on the statement the
//! condition goes into (`SELECT * FROM <table> WHERE <condition>`): the table's columns, the
//! keywords of a condition (not a statement's), functions, the values of an enumerated type or
//! of a check inside a literal; never values read from the table. In an order: the table's
//! columns, then `ASC`, `DESC` (and `NULLS FIRST`, `NULLS LAST` where the server has them).
//! `:where` and `:orderby` complete their argument this way.

use super::*;
use datarig_core::query::{self, Query, Target};
use datarig_core::sql::complete::{Candidate, Completion, Kind, MAX_ITEMS};
use datarig_core::sql::lexer::{LexState, Tok, lex_in};

/// What the table keys of the grid do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAction {
    /// The filter and sort dialog on the cursor's column (`f`).
    EditFilter,
    /// The filter and sort dialog on the order.
    EditOrder,
    /// No condition, no order, and a run.
    Clear,
    /// The statement of the rows shown (else of the draft) in a new console.
    OpenSql,
    /// Sort by the cursor's column alone: going up, going down, no order (`s`).
    SortCycle,
    /// Sort by the cursor's column as one more key, or turn it where it is (`S`).
    SortAdd,
    /// The cursor's column's menu: its sort, its filter, hiding it (`F`).
    ColumnMenu,
    /// Only the value of the cursor's cell or of the cells selected in one column (`=`).
    Only,
    /// Everything but those values (`!`): rows whose value is NULL kept unless NULL is among the
    /// cells.
    Exclude,
    /// No condition and no order, the limit kept, and a run (`X`).
    ClearAll,
    /// The cursor's column hidden from the grid (`C`).
    Hide,
    /// The filters and orders this table ran before (`Ctrl+R` in the dialog, `Space r H`).
    History,
}

impl App {
    /// The table keys of the grid.
    pub(super) fn table_action(&mut self, a: TableAction) {
        let id = self.tab().id;
        // The keys of the cursor's column or the selected cells: none while the grid shows none.
        let cells = matches!(
            a,
            TableAction::SortCycle
                | TableAction::SortAdd
                | TableAction::ColumnMenu
                | TableAction::Only
                | TableAction::Exclude
                | TableAction::Hide
        );
        if cells && self.refused_without_cell() {
            return;
        }
        match a {
            TableAction::EditFilter => self.open_filter_dialog(false),
            TableAction::EditOrder => self.open_filter_dialog(true),
            TableAction::Clear => {
                // Refused before the draft changes: a busy tab keeps what it would run next.
                if self.tab_busy(id) {
                    return self.flash_busy();
                }
                let Some(name) = self.table_name(id) else { return };
                let Some(tq) = self.table_query_mut(id) else { return };
                tq.draft = Query::table(name);
                let q = tq.draft.clone();
                self.table_run(id, q);
            }
            TableAction::OpenSql => self.table_open_sql(id),
            TableAction::SortCycle => self.table_sort(id, self.tab().grid.col, query::SortStep::Only),
            TableAction::SortAdd => self.table_sort(id, self.tab().grid.col, query::SortStep::Add),
            TableAction::ColumnMenu => self.open_column_menu(),
            TableAction::Only => self.grid_filter(super::filter_value::Pick::Only),
            TableAction::Exclude => self.grid_filter(super::filter_value::Pick::Exclude),
            TableAction::ClearAll => self.table_clear_all(id),
            TableAction::Hide => self.table_hide_column(),
            TableAction::History => self.open_filter_history(self.filter_dialog().map_or(id, |d| d.tab)),
        }
    }

    /// `X`: table tab `id` runs again without its condition and its order; its limit stays. Said
    /// when there is neither to clear. The query before is the one the dialog's undo brings back.
    fn table_clear_all(&mut self, id: TabId) {
        if self.tab_busy(id) {
            return self.flash_busy();
        }
        let Some(tq) = self.table_query(id) else { return };
        let base = tq.applied().clone();
        let user = base.order.iter().any(|k| k.origin == query::KeyOrigin::User);
        if base.filter.as_ref().is_none_or(|f| f.text.trim().is_empty()) && !user {
            return self.flash(Notice::new(Label::TableClearAllNothing, Level::Info));
        }
        let q = Query { filter: None, order: Vec::new(), ..base };
        let Some(tq) = self.table_query_mut(id) else { return };
        tq.draft = q.clone();
        tq.refusal = None;
        self.table_run(id, q);
    }

    /// Completion of `text` (the cursor at byte `cursor`) as part `f` of table tab `id`'s
    /// query, offsets in `text`: the condition through the completion engine
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
                let completion = full.completion.filter(|c| c.replace_start >= head.len()).map(|mut c| {
                    c.replace_start -= head.len();
                    c
                });
                out.completion = condition_keywords(completion, text, cursor, target.dialect, force);
                out.wants_checks = full.wants_checks.filter(|w| w.literal >= head.len()).map(|mut w| {
                    w.literal -= head.len();
                    w
                });
            }
            table_query::Field::Order => out.completion = order_completion(text, cursor, &target, force),
            // A number: nothing to complete.
            table_query::Field::Limit => {}
        }
        out
    }
}

/// The keywords a condition is written with, sorted: its operators, `CASE`, literals, and the
/// functions and casts the lexer reads as keywords. Statement words (`ALTER`, `SELECT`, …) are
/// not offered in a condition.
const PG_CONDITION_KEYWORDS: &[&str] = &[
    "ALL",
    "AND",
    "ANY",
    "ARRAY",
    "AS",
    "BETWEEN",
    "CASE",
    "CAST",
    "COALESCE",
    "COLLATE",
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "DISTINCT",
    "ELSE",
    "END",
    "ESCAPE",
    "EXISTS",
    "EXTRACT",
    "FALSE",
    "FROM",
    "ILIKE",
    "IN",
    "INTERVAL",
    "IS",
    "LIKE",
    "NOT",
    "NULL",
    "NULLIF",
    "OR",
    "SIMILAR",
    "SOME",
    "SYMMETRIC",
    "THEN",
    "TO",
    "TRUE",
    "UNKNOWN",
    "WHEN",
];

/// [`PG_CONDITION_KEYWORDS`] of MySQL.
const MYSQL_CONDITION_KEYWORDS: &[&str] = &[
    "ALL",
    "AND",
    "ANY",
    "AS",
    "BETWEEN",
    "BINARY",
    "CASE",
    "CAST",
    "COLLATE",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "DIV",
    "ELSE",
    "END",
    "ESCAPE",
    "EXISTS",
    "FALSE",
    "FROM",
    "IN",
    "INTERVAL",
    "IS",
    "LIKE",
    "MOD",
    "NOT",
    "NULL",
    "OR",
    "REGEXP",
    "RLIKE",
    "SOME",
    "THEN",
    "TRUE",
    "UNKNOWN",
    "WHEN",
    "XOR",
];

/// The keywords of a condition of dialect `d`.
pub(crate) fn condition_keywords_of(d: Dialect) -> &'static [&'static str] {
    match d {
        Dialect::Postgres => PG_CONDITION_KEYWORDS,
        Dialect::MySql(_) => MYSQL_CONDITION_KEYWORDS,
    }
}

/// `completion` of condition `text` (the cursor at byte `cursor`) with its keywords those of a
/// condition ([`condition_keywords_of`]) instead of the statement's: offered after the other
/// entries as the editor offers keywords (by the prefix typed, or all of them when `force`),
/// never inside a string, a quoted name or a comment, nor where a table is named.
fn condition_keywords(
    completion: Option<Completion>,
    text: &str,
    cursor: usize,
    d: Dialect,
    force: bool,
) -> Option<Completion> {
    let mut items: Vec<Candidate> = Vec::new();
    let mut at = None;
    if let Some(c) = completion {
        at = Some((c.replace_start, c.trail));
        items = c.items.into_iter().filter(|i| i.kind != Kind::Keyword).collect();
    }
    let before = &text[..cursor.min(text.len())];
    let start =
        before.char_indices().rev().take_while(|(_, c)| is_ident_char(*c)).last().map_or(before.len(), |(i, _)| i);
    let prefix = &before[start..];
    // Inside a token that is not a word: no keyword is typed there.
    let inside = lex_in(text, d).iter().any(|t| {
        t.start < cursor
            && cursor <= t.end
            && matches!(t.kind, Tok::Str | Tok::Dollar | Tok::QuotedIdent | Tok::LineComment | Tok::BlockComment)
    });
    let relations = items.iter().any(|i| matches!(i.kind, Kind::Table | Kind::View | Kind::Schema));
    let values = items.iter().any(|i| i.kind == Kind::Value);
    let qualified = before[..start].ends_with('.');
    if (!prefix.is_empty() || force) && !inside && !relations && !values && !qualified {
        let lower = !prefix.is_empty() && prefix.chars().all(|c| !c.is_uppercase());
        let typed = prefix.to_ascii_uppercase();
        items.extend(condition_keywords_of(d).iter().filter(|k| k.starts_with(typed.as_str())).map(|k| Candidate {
            label: if lower { k.to_lowercase() } else { k.to_string() },
            kind: Kind::Keyword,
            detail: None,
            insert: None,
        }));
    }
    items.truncate(MAX_ITEMS);
    if items.is_empty() {
        return None;
    }
    let (replace_start, trail) = at.unwrap_or((start, 0));
    Some(Completion { items, replace_start, trail })
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
