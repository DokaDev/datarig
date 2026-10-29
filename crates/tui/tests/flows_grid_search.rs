//! The search inside the result grid through the real `App` event path: `/` and `?` with a
//! prompt under the grid that previews the first match while the pattern is typed, `Enter`,
//! `n`/`N` from match to match (the next page while no search is on), `Esc` (back where the
//! cursor was in the prompt; the end of the search after it), the matches highlighted, pages
//! switched, spilled rows searched, rows still on the server said, and answers of jobs the grid
//! no longer waits for dropped.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::i18n::Lang;
use datarig_core::paths::Paths;
use datarig_tui::app::grid_search::{Answer, News};
use datarig_tui::app::tabs::ResultView;
use datarig_tui::app::{AppEvent, Focus, Paging, Results};
use datarig_tui::keymap::Ctx;
use datarig_tui::theme;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The edge-case users in the grid, the grid focused.
fn grid() -> Harness {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    h
}

fn cursor(h: &Harness) -> (usize, usize) {
    (h.app.tab().grid.row, h.app.tab().grid.col)
}

/// The line under the grid as drawn at 120×40.
fn strip(h: &mut Harness) -> String {
    let screen = h.screen(120, 40);
    let r = h.app.layout.results;
    screen.lines().nth((r.y + r.height - 2) as usize).unwrap_or_default().to_string()
}

/// One row of numbers `0..n` in a column `id`, read in pages of 500 (`more`: the server has
/// more after them).
fn numbers(n: usize, more: bool) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let page = |from: usize, to: usize| (from..to).map(|i| vec![Some(i.to_string())]).collect::<Vec<_>>();
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let elapsed = Duration::from_millis(1);
    let mut from = 0;
    let mut columns = columns;
    while from < n {
        let to = (from + 500).min(n);
        let more = to < n || more;
        h.tab_db(0, DbEvent::Page { id, columns: columns.take(), rows: page(from, to), more, elapsed });
        from = to;
    }
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    h
}

/// Keys handed to the app without waiting for the search jobs they start.
fn raw(h: &mut Harness, keys: &str) {
    for c in keys.chars() {
        let code = if c == '\n' { KeyCode::Enter } else { KeyCode::Char(c) };
        h.app.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }
}

#[test]
fn the_prompt_previews_the_first_match_and_esc_goes_back() {
    let mut h = grid();
    h.keys("jl");
    assert_eq!(cursor(&h), (1, 1));
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::GridSearch);
    h.type_text("Plain");
    assert_eq!(cursor(&h), (6, 1), "the preview");
    let line = strip(&mut h);
    assert!(line.contains("/Plain") && line.contains("1/1"), "{line:?}");
    // A pattern that matches nothing puts the cursor back where the prompt opened.
    h.type_text("zz");
    assert_eq!(cursor(&h), (1, 1));
    h.key(KeyCode::Esc);
    assert_eq!(cursor(&h), (1, 1));
    assert_eq!(h.app.key_context(), Ctx::Grid, "no search is on");
    // `Backspace` on an empty pattern closes it too.
    h.keys("/");
    h.type_text("ascii");
    assert_eq!(cursor(&h), (6, 2));
    for _ in 0..5 {
        h.key(KeyCode::Backspace);
    }
    assert_eq!(h.app.key_context(), Ctx::GridSearch);
    h.key(KeyCode::Backspace);
    assert_eq!((h.app.key_context(), cursor(&h)), (Ctx::Grid, (1, 1)));
}

#[test]
fn enter_n_and_shift_n_go_from_cell_to_cell_and_around_the_ends() {
    let mut h = grid();
    h.keys("/");
    h.type_text("lang");
    h.key(KeyCode::Enter);
    // The profile column of rows 1, 5, 6 and 7 (row 0 is the cursor's: searched after it).
    assert_eq!(cursor(&h), (0, 4));
    assert_eq!(h.app.key_context(), Ctx::GridMatches);
    assert_eq!(h.app.grid_search_pattern(), Some("lang"));
    let line = strip(&mut h);
    assert!(line.contains("/lang") && line.contains(" 1/4 "), "{line:?}");
    h.keys("n");
    assert_eq!(cursor(&h), (4, 4));
    h.keys("nn");
    assert_eq!(cursor(&h), (6, 4));
    assert!(strip(&mut h).contains("4/4"));
    h.keys("n");
    assert_eq!(cursor(&h), (0, 4));
    assert!(h.status(120, 40).contains("Search hit the bottom, continuing at the top"));
    h.keys("N");
    assert_eq!(cursor(&h), (6, 4));
    assert!(h.status(120, 40).contains("Search hit the top, continuing at the bottom"));
    // Off a match the count says so; `N` goes back from there.
    h.keys("kh");
    assert!(strip(&mut h).contains("-/4"), "{}", strip(&mut h));
    h.keys("N");
    assert_eq!(cursor(&h), (4, 4));
    // `?` searches backward: `n` keeps its direction, `N` turns it around.
    h.keys("?");
    h.type_text("lang");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (0, 4));
    assert!(strip(&mut h).contains("?lang"));
    h.keys("n");
    assert_eq!(cursor(&h), (6, 4), "around the start");
    h.keys("N");
    assert_eq!(cursor(&h), (0, 4));
}

#[test]
fn the_full_value_is_searched_and_null_never_matches() {
    let mut h = grid();
    // Past the 40-column cut of the address shown.
    h.keys("/");
    h.type_text("10\u{6A13}$");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (0, 3));
    // The tab inside an address is searched as sent, not as the arrow the grid shows.
    h.keys("/");
    h.type_text("\\t");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (7, 3));
    // NULL has no text: a pattern of the empty text finds no cell, nor does the label shown.
    for p in ["^$", "NULL"] {
        h.keys("/");
        h.type_text(p);
        h.key(KeyCode::Enter);
        assert_eq!(cursor(&h), (7, 3), "{p}");
        assert!(h.status(120, 40).contains(&format!("Pattern not found: {p}")), "{p}");
    }
    // Case matters unless the pattern says otherwise; column names are not searched.
    h.keys("gg0/");
    h.type_text("plain");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (0, 0));
    h.keys("/");
    h.type_text("(?i)plain");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (6, 1));
    h.keys("/");
    h.type_text("nickname");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (6, 1));
}

#[test]
fn an_invalid_or_missing_pattern_is_said_and_the_cursor_stays() {
    let mut h = grid();
    h.keys("j/");
    h.key(KeyCode::Enter);
    assert!(h.status(120, 40).contains("No previous search pattern"));
    assert_eq!((cursor(&h), h.app.key_context()), ((1, 0), Ctx::Grid));
    h.keys("/");
    h.type_text("(ab");
    h.key(KeyCode::Enter);
    assert!(h.status(120, 40).contains("Not a valid search pattern"), "{}", h.status(120, 40));
    assert_eq!(cursor(&h), (1, 0));
    // An empty pattern takes the last one; a paste is typed into the prompt.
    h.keys("/");
    h.event(Event::Paste("Emoji".into()));
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (3, 1));
    h.keys("gg/");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (3, 1));
}

#[test]
fn esc_ends_the_search_and_n_is_the_next_page_again() {
    let mut h = numbers(500, true);
    h.sent();
    h.keys("/");
    h.type_text("^42$");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (42, 0));
    // While the search is on `n` goes to its matches, never to the next page.
    h.keys("n");
    assert_eq!(cursor(&h), (42, 0));
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::FetchMore { .. })));
    // Where it fits, the line says the server has more rows (with the next page's key while
    // `n` is the search's); narrower, a short note; the count always stays.
    let screen = h.screen(200, 40);
    let r = h.app.layout.results;
    let line = screen.lines().nth((r.y + r.height - 2) as usize).unwrap_or_default();
    assert!(
        line.contains("/^42$")
            && line.contains("1/1 in 500 rows read \u{b7} more rows on the server (Space r n: next page)"),
        "{line:?}"
    );
    let line = strip(&mut h);
    assert!(line.contains("/^42$") && line.contains(" 1/1 more on server "), "{line:?}");
    assert!(!h.screen(120, 40).contains("n: next page"), "never `n` for the next page while it is the search's");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!(h.app.focus, Focus::Results, "the first Esc only ends the search");
    assert!(!strip(&mut h).contains("^42$"));
    h.keys("n");
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::FetchMore { .. })), "the next page");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.focus, Focus::Editor, "the next Esc goes back as before");
}

#[test]
fn a_match_on_another_page_shows_that_page_and_later_rows_wait_for_the_next_search() {
    let mut h = numbers(1_200, false);
    h.keys("/");
    h.type_text("^(10|700|1100)$");
    h.key(KeyCode::Enter);
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((10, 0), 0));
    h.keys("n");
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((700, 0), 1));
    assert!(h.screen(120, 40).contains("Results \u{b7} rows 501\u{2013}1,000"));
    h.keys("n");
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((1100, 0), 2));
    assert!(strip(&mut h).contains("3/3"));
    h.keys("n");
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((10, 0), 0));
    // `Esc` in the prompt puts the page back too.
    h.keys("/");
    h.type_text("^1199$");
    assert_eq!(h.app.tab().grid.page, 2);
    h.key(KeyCode::Esc);
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((10, 0), 0));

    // Rows fetched after a search are searched by the next `n`.
    let mut h = numbers(500, true);
    h.keys("/");
    h.type_text("^(7|600)$");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (7, 0));
    let id = h.app.tab().exec.query_id;
    let rows = (500..1_000).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(0, DbEvent::Page { id, columns: None, rows, more: false, elapsed: Duration::from_millis(1) });
    assert_eq!(h.app.key_context(), Ctx::GridMatches, "more rows keep the search");
    h.keys("n");
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((600, 0), 1));
    assert!(strip(&mut h).contains("2/2"));
}

#[test]
fn the_matches_are_highlighted_and_the_cursors_match_apart() {
    let mut h = grid();
    h.keys("/");
    h.type_text("lang");
    h.key(KeyCode::Enter);
    h.keys("n");
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let g = &h.app.tab().grid;
    let rect = |r, c| g.cell_rect(r, c, 8).expect("on screen");
    let at = |r, c| buf[(rect(r, c).x + 1, rect(r, c).y)].clone();
    assert_eq!(at(0, 4).bg, theme::DARK.grid_match.bg.unwrap(), "another match");
    assert_eq!(at(4, 4).bg, theme::DARK.grid_match_current.bg.unwrap(), "the cursor's match");
    assert!(at(4, 4).modifier.contains(ratatui::style::Modifier::UNDERLINED));
    assert_ne!(at(1, 4).bg, theme::DARK.grid_match.bg.unwrap(), "not a match");
    // Each frame looks up the cells on screen only, and nothing without a search.
    h.app.take_grid_search_work();
    h.draw(120, 40);
    let shown = g_cells(&h);
    assert_eq!(h.app.take_grid_search_work(), shown);
    h.key(KeyCode::Esc);
    h.draw(120, 40);
    assert_eq!(h.app.take_grid_search_work(), 0);
}

/// The cells the grid drew last.
fn g_cells(h: &Harness) -> usize {
    let g = &h.app.tab().grid;
    g.hit_cols.len() * g.page_rows.min(8)
}

#[test]
fn a_click_closes_the_prompt_and_keeps_the_search() {
    let mut h = grid();
    h.keys("/");
    h.type_text("lang");
    h.key(KeyCode::Enter);
    h.keys("/");
    h.type_text("Plain");
    assert_eq!(cursor(&h), (6, 1));
    h.draw(120, 40);
    let (x, y) = (h.app.tab().grid.hit_cols[2].0 + 2, h.app.tab().grid.data_y + 2);
    let down =
        MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: x, row: y, modifiers: KeyModifiers::NONE };
    h.event(Event::Mouse(down));
    assert_eq!(cursor(&h), (0, 4), "back where the prompt opened");
    assert_eq!(h.app.grid_search_pattern(), Some("lang"), "the search before stays");
    // A click on a match makes it the one `n` goes on from.
    h.draw(120, 40);
    let g = &h.app.tab().grid;
    let rect = g.cell_rect(5, 4, 8).expect("the profile of row 6 on screen");
    let on = MouseEvent { column: rect.x + 1, row: rect.y, ..down };
    h.event(Event::Mouse(on));
    assert_eq!(cursor(&h), (5, 4));
    h.keys("n");
    assert_eq!(cursor(&h), (6, 4), "the match after the clicked one");
    h.keys("N");
    assert_eq!(cursor(&h), (5, 4));
}

#[test]
fn a_spilled_result_is_searched_through_its_file() {
    let state = TempState::new("search");
    let mut cfg = test_db_config();
    cfg.result_window_rows = 1_000;
    let mut h = Harness::with_config(&cfg, Lang::En);
    h.app.set_paths(Paths { data: None, state: Some(state.0.clone()) });
    h.db(DbEvent::Connected);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let mut columns = Some(vec![meta("id", "text", false, false), meta("x", "text", false, false)]);
    for p in 0..10 {
        let rows = (p * 500..p * 500 + 500).map(|i| vec![Some(format!("r{i:05}")), None]).collect();
        let more = p < 9;
        h.tab_db(0, DbEvent::Page { id, columns: columns.take(), rows, more, elapsed: Duration::from_millis(1) });
    }
    let Results::Rows(rs) = &h.app.tab().results else { panic!("rows") };
    assert!(rs.rows.spilled() > 0 && !rs.rows.resident().contains(&3), "{:?}", rs.rows.resident());
    h.key(KeyCode::Tab);
    h.keys("G/");
    h.type_text("^r0000[3-4]$");
    h.key(KeyCode::Enter);
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((3, 0), 0), "around the end, from the file");
    assert!(h.screen(120, 40).contains("r00003"));
    h.keys("n");
    assert_eq!(cursor(&h), (4, 0));
    assert!(strip(&mut h).contains("2/2"));
}

#[test]
fn an_answer_the_grid_no_longer_waits_for_is_dropped() {
    let mut h = grid();
    let tab = h.app.tab().id;
    // An answer of no job of the tab moves nothing.
    h.app.on_app_event(AppEvent::GridSearch(Answer { job: 999, tab, news: News::First { cell: 30, wrapped: false } }));
    assert_eq!(cursor(&h), (0, 0));
    // The prompt's job is out (not handed back yet) when the tab's result is replaced under it:
    // its answers, when they come, are about rows the grid does not show and are dropped by
    // the result they were for (nothing else ended the job first).
    raw(&mut h, "/Plain");
    assert_eq!(h.app.key_context(), Ctx::GridSearch);
    let (cols, rows) = edge_rows();
    let cols = cols.into_iter().collect();
    let rs = datarig_tui::widgets::grid::ResultSet::in_memory(cols, rows, false, "NULL");
    h.app.tabs.active_mut().results = Results::Rows(rs);
    assert!(h.app.pump_grid_search(Duration::from_secs(30)) > 0, "the jobs' answers came");
    assert_eq!(cursor(&h), (0, 0), "the new rows' cursor stays");
    assert_ne!(h.app.key_context(), Ctx::GridSearch, "the search of the old rows went");
    assert!(h.app.grid_search_strip().is_none());
    // A pattern typed further replaces the job before it: only the last one's matches count.
    raw(&mut h, "/fl");
    h.app.pump_grid_search(Duration::from_secs(30));
    assert_eq!(cursor(&h), (3, 4));
    assert!(strip(&mut h).contains("1/1"), "{}", strip(&mut h));
}

#[test]
fn another_result_tab_or_a_new_run_drops_the_search() {
    let mut h = grid();
    h.keys("/");
    h.type_text("lang");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::GridMatches);
    // The Chart tab and back: the search is gone.
    h.keys("L");
    assert_eq!(h.app.tab().exec.view, ResultView::Chart);
    h.keys("H");
    assert_eq!(h.app.tab().exec.view, ResultView::Rows);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // A run again: its new rows have no search either.
    h.keys("/");
    h.type_text("lang");
    h.key(KeyCode::Enter);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let (cols, rows) = edge_rows();
    h.tab_db(0, DbEvent::Page { id, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(1) });
    h.app.focus = Focus::Results;
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // `:noh` ends it too.
    h.keys("/");
    h.type_text("lang");
    h.key(KeyCode::Enter);
    h.command("noh");
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(!strip(&mut h).contains("/lang"));
}

#[test]
fn the_messages_and_chart_tabs_do_not_search() {
    let mut h = grid();
    h.keys("L");
    assert_eq!(h.app.tab().exec.view, ResultView::Chart);
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::Chart);
    h.keys("L");
    assert_eq!(h.app.tab().exec.view, ResultView::Messages);
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert!(!h.app.grid_prompt_open());
}

/// A test's state directory, removed when dropped.
struct TempState(PathBuf);

impl TempState {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-flows-grid-search-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempState(dir)
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(Path::new(&self.0));
    }
}

#[test]
fn ctrl_w_deletes_a_word_of_the_pattern_and_never_closes_the_tab() {
    let mut h = grid();
    // An open transaction: a Ctrl+W that closed the tab would ask first.
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.keys("/");
    h.type_text("lang ko");
    h.ctrl('w');
    assert!(h.overlay_kind().is_none(), "nothing is asked");
    assert_eq!((h.app.tabs.len(), h.app.key_context()), (1, Ctx::GridSearch));
    let line = strip(&mut h);
    assert!(line.contains("/lang ") && !line.contains("/lang ko"), "{line:?}");
    h.ctrl('w');
    h.ctrl('w');
    assert!(!strip(&mut h).contains("lang"));
    h.type_text("lang");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (0, 4));
}

#[test]
fn rows_that_come_while_a_search_runs_are_searched_by_the_next_n() {
    let mut h = numbers(500, true);
    raw(&mut h, "/^(7|600)$\n");
    let id = h.app.tab().exec.query_id;
    let rows = (500..1_000).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(0, DbEvent::Page { id, columns: None, rows, more: false, elapsed: Duration::from_millis(1) });
    h.app.pump_grid_search(Duration::from_secs(30));
    assert_eq!(cursor(&h), (7, 0));
    // The matches found are of the 500 rows the job read: `n` searches the 1,000 there are now.
    h.keys("n");
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((600, 0), 1));
    assert!(strip(&mut h).contains("2/2"));
}

#[test]
fn the_page_end_hint_names_the_next_page_key_of_the_search() {
    let mut h = numbers(1_000, false);
    h.keys("G");
    assert!(h.status(120, 40).contains("End of the page: n shows the next one"));
    h.keys("gg/");
    h.type_text("^3$");
    h.key(KeyCode::Enter);
    h.keys("G");
    let status = h.status(120, 40);
    assert!(status.contains("End of the page: Space r n shows the next one"), "{status}");
}

#[test]
fn a_search_job_does_not_move_a_cursor_that_moved_before_its_answer() {
    let mut h = numbers(500, false);
    raw(&mut h, "/^300$\nj");
    h.app.pump_grid_search(Duration::from_secs(30));
    assert_eq!(cursor(&h), (1, 0), "the cursor stays where it was moved");
    assert_eq!(h.app.key_context(), Ctx::GridMatches);
    h.keys("n");
    assert_eq!(cursor(&h), (300, 0));
}

#[test]
fn the_highlight_keeps_an_unfocused_cursor_and_a_range() {
    let mut h = grid();
    h.keys("/");
    h.type_text("lang");
    h.key(KeyCode::Enter);
    h.keys("nvj");
    assert_eq!(cursor(&h), (5, 4));
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let g = &h.app.tab().grid;
    let at = |r, c| {
        let a = g.cell_rect(r, c, 8).expect("on screen");
        buf[(a.x + 1, a.y)].clone()
    };
    assert_eq!(at(4, 4).bg, theme::DARK.range.bg.unwrap(), "a match in the range keeps the range");
    assert!(at(4, 4).modifier.contains(ratatui::style::Modifier::UNDERLINED), "and says it matches");
    h.app.focus = Focus::Editor;
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let g = &h.app.tab().grid;
    let a = g.cell_rect(5, 4, 8).unwrap();
    assert_eq!(buf[(a.x + 1, a.y)].bg, theme::DARK.selection_unfocused().bg.unwrap(), "the cursor shows");
}

#[test]
fn esc_in_the_prompt_brings_back_a_selected_range() {
    let mut h = numbers(1_200, false);
    h.keys("jv");
    let before = h.app.tab().grid.anchor;
    assert_eq!(before, Some((1, 0)));
    h.keys("/");
    h.type_text("^1100$");
    assert_eq!((h.app.tab().grid.page, h.app.tab().grid.anchor), (2, None), "the preview's page drops it");
    h.key(KeyCode::Esc);
    assert_eq!((cursor(&h), h.app.tab().grid.page, h.app.tab().grid.anchor), ((1, 0), 0, before));
}

#[test]
fn a_search_that_matches_nothing_ends_and_n_pages_again() {
    let mut h = numbers(500, true);
    h.sent();
    h.keys("/");
    h.type_text("zzz");
    h.key(KeyCode::Enter);
    assert!(h.status(120, 40).contains("Pattern not found: zzz"));
    assert_eq!(h.app.key_context(), Ctx::Grid, "no search is on");
    h.keys("n");
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::FetchMore { .. })), "the next page");
}

#[test]
fn no_prompt_opens_where_the_grid_has_no_line_for_it() {
    let mut h = grid();
    // The panes keep the grid four lines at least (a shorter terminal is too small for them):
    // the grid as a frame left it with three.
    h.keys("----------");
    h.draw(120, 24);
    assert!(h.app.layout.grid_area.height >= 4, "{:?}", h.app.layout.grid_area);
    h.app.layout.grid_area.height = 3;
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::Grid, "no prompt");
    let status = h.status(120, 24);
    assert!(status.contains("No room for the search line under the grid"), "{status}");
    // With the line it needs, it opens.
    h.keys("/");
    assert_eq!(h.app.key_context(), Ctx::GridSearch);
}

#[test]
fn past_the_cap_n_and_shift_n_seek_from_the_cursor() {
    let mut h = numbers(500, false);
    h.app.set_grid_search_cap(3);
    h.keys("/");
    h.type_text("7$");
    h.key(KeyCode::Enter);
    assert_eq!(cursor(&h), (7, 0));
    assert!(strip(&mut h).contains("1/3+"), "{}", strip(&mut h));
    let scans = h.app.grid_search_scans();
    h.keys("nn");
    assert_eq!(cursor(&h), (27, 0), "from the matches kept");
    // Past the last match kept: the next one from the cursor, no full search again.
    h.keys("n");
    assert_eq!(cursor(&h), (37, 0));
    h.keys("n");
    assert_eq!(cursor(&h), (47, 0));
    h.keys("N");
    assert_eq!(cursor(&h), (37, 0));
    h.keys("G");
    h.keys("n");
    assert_eq!(cursor(&h), (7, 0), "around the end");
    assert!(h.status(120, 40).contains("Search hit the bottom"));
    h.keys("N");
    assert_eq!(cursor(&h), (497, 0), "around the start");
    assert!(h.status(120, 40).contains("Search hit the top"));
    assert_eq!(h.app.grid_search_scans(), scans, "every n and N only seeks");
}

#[test]
fn n_while_the_matches_are_being_found_does_not_start_over() {
    let mut h = numbers(500, false);
    h.keys("/");
    h.type_text("^(7|17|27)$");
    let scans = h.app.grid_search_scans();
    raw(&mut h, "\nnnn");
    h.app.pump_grid_search(Duration::from_secs(30));
    assert_eq!(h.app.grid_search_scans(), scans + 1, "one search of every row, the n keys beside it");
    assert!(strip(&mut h).contains("/3"), "the count came: {}", strip(&mut h));
    h.keys("n");
    let at = cursor(&h);
    assert!([(7, 0), (17, 0), (27, 0)].contains(&at), "{at:?}");
}

#[test]
fn the_search_line_says_what_the_paging_line_would() {
    let mut h = numbers(500, true);
    h.app.tab_mut().exec.paging = Paging::Stopped;
    let screen = h.screen(200, 40);
    assert!(screen.contains("Showing 500 rows \u{b7} fetching stopped"), "the paging line: {screen}");
    h.keys("/");
    h.type_text("^42$");
    h.key(KeyCode::Enter);
    let screen = h.screen(200, 40);
    let r = h.app.layout.results;
    let line = screen.lines().nth((r.y + r.height - 2) as usize).unwrap_or_default();
    assert!(line.contains("/^42$") && line.contains("1/1 Showing 500 rows \u{b7} fetching stopped"), "{line:?}");
    let line = strip(&mut h);
    assert!(line.contains(" 1/1 more on server "), "{line:?}");
}

/// The line under the grid as drawn at `w`×`hh`.
fn strip_at(h: &mut Harness, w: u16, hh: u16) -> String {
    let screen = h.screen(w, hh);
    let r = h.app.layout.results;
    screen.lines().nth((r.y + r.height - 2) as usize).unwrap_or_default().to_string()
}

/// The view's last row, as drawn at 120×40.
fn view_end(h: &mut Harness) -> usize {
    h.draw(120, 40);
    let g = &h.app.tab().grid;
    g.top + g.page_rows
}

#[test]
fn a_match_on_another_page_fills_the_view_up_to_the_pages_end() {
    let mut h = numbers(1_200, false);
    h.keys("/");
    h.type_text("^1100$");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().grid.page, 2);
    view_end(&mut h);
    // Back to the last row of the first page: the view ends there, no blank lines below.
    h.keys("?");
    h.type_text("^499$");
    h.key(KeyCode::Enter);
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((499, 0), 0));
    assert_eq!(view_end(&mut h), 500, "top {}", h.app.tab().grid.top);
    // A match late on the next page.
    h.keys("/");
    h.type_text("^999$");
    h.key(KeyCode::Enter);
    assert_eq!((cursor(&h), h.app.tab().grid.page), ((999, 0), 1));
    assert_eq!(view_end(&mut h), 1_000);
    // A preview on another page, then `Esc`: back with the view as it was.
    let top = h.app.tab().grid.top;
    h.keys("/");
    h.type_text("^5$");
    assert_eq!(h.app.tab().grid.page, 0);
    view_end(&mut h);
    h.key(KeyCode::Esc);
    assert_eq!((cursor(&h), view_end(&mut h), h.app.tab().grid.top), ((999, 0), 1_000, top));
}

#[test]
fn the_short_note_fits_a_small_terminal() {
    let mut h = numbers(500, true);
    h.keys("/");
    h.type_text("^42$");
    h.key(KeyCode::Enter);
    let line = strip_at(&mut h, 80, 24);
    assert!(line.contains("/^42$") && line.contains(" 1/1 more on server "), "{line:?}");
}

#[test]
fn a_long_pattern_keeps_its_end_and_marks_its_cut_start() {
    let mut h = numbers(500, true);
    h.keys("/");
    h.type_text("(?i)(needle|haystack|1234567890)");
    let line = strip_at(&mut h, 80, 24);
    assert!(line.contains("/\u{2026}") && line.contains("1234567890)"), "the end typed, the start marked: {line:?}");
    assert!(!line.contains("/?i)") && !line.contains("more on"), "{line:?}");
    // Wide enough: the whole pattern, then the count.
    let line = strip_at(&mut h, 200, 40);
    assert!(line.contains("/(?i)(needle|haystack|1234567890)"), "{line:?}");
}

/// Numbers `0..n` in a column `id` read with the portal held open (the policy's idle timeout
/// closes it), the grid focused.
fn held(n: usize) -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::TxOpen(true));
    let columns = Some(vec![meta("id", "int8", true, false)]);
    let rows = (0..n).map(|i| vec![Some(i.to_string())]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: true, elapsed: Duration::from_millis(2) });
    h.key(KeyCode::Tab);
    h
}

#[test]
fn the_prompt_names_the_next_page_key_of_the_search() {
    let mut h = held(500);
    // The portal closes for being idle: the next page runs the SELECT again, by its key.
    h.advance(Duration::from_secs(31));
    assert!(strip_at(&mut h, 200, 40).contains("paging closed \u{2014} n runs the SELECT again"));
    // While the prompt is open `n` is typed, and after `Enter` it is the search's.
    h.keys("/");
    h.type_text("^42$");
    let line = strip_at(&mut h, 200, 40);
    assert!(line.contains("/^42$") && line.contains("Space r n runs the SELECT again"), "{line:?}");
}

#[test]
fn a_background_tabs_idle_close_names_the_next_page_key_of_its_search() {
    let mut h = held(500);
    h.keys("/");
    h.type_text("^42$");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.key_context(), Ctx::GridMatches);
    // Another tab is active when the portal of the first one closes for being idle.
    h.ctrl('t');
    assert_eq!(h.app.tabs.active_index(), 1);
    h.advance(Duration::from_secs(31));
    h.keys("gT");
    h.app.focus = Focus::Results;
    assert_eq!((h.app.tabs.active_index(), h.app.key_context()), (0, Ctx::GridMatches));
    let status = h.status(200, 40);
    assert!(status.contains("Space r n runs the SELECT again"), "{status}");
}
