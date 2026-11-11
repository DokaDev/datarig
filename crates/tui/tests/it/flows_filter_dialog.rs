//! The filter and sort dialog of a table tab: `f` (or `Space r f`) opens it on the grid's
//! column, `Space r o` on the order; conditions are built with an operator and typed values,
//! applied with `Enter` (the statement sent is the compiled query, terms not changed sent as
//! written), dropped with `Esc`; undo goes back a step, then to the query before the last
//! applied change; a refusal of the app keeps the dialog open and says why; a question opens
//! above it and a no keeps it; it closes once the statement is sent.

use crate::common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_i18n::Lang;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

/// Open `shop.users` from the explorer (a table tab, which runs its query).
fn open_users(h: &mut Harness) {
    h.key(KeyCode::BackTab);
    h.goto("shop");
    h.key(KeyCode::Char('l'));
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok((vec!["users".into()], Vec::new()).into()) });
    h.goto("users");
    h.key(KeyCode::Enter);
    assert!(h.app.tab().is_table());
}

/// The statements of the runs sent since the last look.
fn runs(h: &mut Harness) -> Vec<Vec<String>> {
    h.sent()
        .into_iter()
        .filter_map(|c| match c {
            DbCommand::Execute { statements, .. } => Some(statements),
            _ => None,
        })
        .collect()
}

/// Answer the active tab's current run with a first page of `shop.users` rows.
fn answer(h: &mut Harness) {
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..3).map(|i| vec![Some(i.to_string()), Some(format!("n{i}"))]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
}

/// `shop.users` open and answered, the grid focused, nothing sent since.
fn users() -> Harness {
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    answer(&mut h);
    h.sent();
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h
}

fn open(h: &Harness) -> bool {
    h.app.overlays.is_open(OverlayKind::FilterDialog)
}

/// In the open dialog: a condition added on the selected column (from the column list), its
/// operator picked `down` places below the first the column takes, `value` typed.
fn condition(h: &mut Harness, down: usize, value: &str) {
    // The column list, then `+ add condition` (the first line of a column without conditions).
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    if down > 0 {
        h.key(KeyCode::Left);
        h.key(KeyCode::Char(' '));
        for _ in 0..down {
            h.key(KeyCode::Down);
        }
        h.key(KeyCode::Enter);
    }
    h.type_text(value);
}

/// Back to the column list from anywhere in the right pane.
fn to_columns(h: &mut Harness) {
    while h.app.key_context() != Ctx::FilterDialog {
        h.key(KeyCode::Tab);
    }
    for _ in 0..8 {
        h.key(KeyCode::BackTab);
        if h.screen(120, 34).contains("\u{25b8} ") && h.app.key_context() == Ctx::FilterDialog {
            break;
        }
    }
}

#[test]
fn f_opens_the_dialog_and_enter_sends_the_condition_built() {
    let mut h = users();
    h.keys("f");
    assert!(open(&h));
    assert_eq!(h.app.key_context(), Ctx::FilterDialog);
    // On the grid's column (`id`), a condition `= 1`.
    condition(&mut h, 0, "1");
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText, "typing goes into the value");
    assert!(runs(&mut h).is_empty(), "nothing is sent while the draft is built");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id = 1)")]]);
    assert!(!open(&h), "the dialog closes once the statement is sent");
    assert_eq!(h.app.key_context(), Ctx::Grid);
    // `Space r f` opens it too, with what is applied as its draft.
    answer(&mut h);
    h.keys(" rf");
    assert!(open(&h));
    assert!(h.screen(120, 34).contains("id = 1"), "{}", h.screen(120, 34));
}

#[test]
fn each_operator_sends_its_term() {
    // In the picker's order for a number column: =, between, ≠, ≠ or NULL, <, ≤, >, ≥, (the
    // patterns, dimmed on a number) in list, not in list, is NULL, is not NULL.
    let cases: [(usize, &str, &str); 12] = [
        (0, "7", "id = 7"),
        (1, "1", "id BETWEEN 1 AND 2"),
        (2, "7", "id <> 7"),
        (3, "7", "id IS DISTINCT FROM 7"),
        (4, "7", "id < 7"),
        (5, "7", "id <= 7"),
        (6, "7", "id > 7"),
        (7, "7", "id >= 7"),
        (10, "1, 2", "id IN (1, 2)"),
        (11, "1, 2", "id NOT IN (1, 2)"),
        (12, "", "id IS NULL"),
        (13, "", "id IS NOT NULL"),
    ];
    for (down, value, term) in cases {
        let mut h = users();
        h.keys("f");
        condition(&mut h, down, value);
        if down == 1 {
            // `between`: the second value in its own field.
            h.key(KeyCode::Right);
            h.type_text("2");
        }
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({term})")]], "{term}");
    }
}

#[test]
fn text_patterns_send_their_like() {
    // A text column: no `between`; `contains` and `starts with` after the comparisons, offered
    // once its collation is known to take them.
    for (down, term) in [(7, "name ILIKE '%a!_b%' ESCAPE '!'"), (8, "name LIKE 'a!_b%' ESCAPE '!'")] {
        let mut h = users();
        let keys = datarig_core::driver::keys::OrderKeys {
            primary_key: vec!["id".into()],
            deterministic: [("name".to_string(), true)].into(),
            ..Default::default()
        };
        keys_came_with(&mut h, "local-pg", keys);
        h.key(KeyCode::Right);
        h.keys("f");
        condition(&mut h, down, "a_b");
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({term})")]], "{term}");
    }
}

#[test]
fn terms_not_changed_are_sent_as_written() {
    // An `OR` at the top is one expression chip; a function term another; odd spacing stays.
    for text in ["id > 1 OR name = 'x'", "lower(name) = 'a'  AND   id > 1"] {
        let mut h = users();
        h.command(&format!("where {text}"));
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({text})")]]);
        answer(&mut h);
        h.keys("f");
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({text})")]], "untouched: byte for byte");
    }
    // A condition added: the others stay as written, joined with ` AND `.
    let mut h = users();
    h.command("where id > 1 OR name = 'x'");
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    condition(&mut h, 0, "3");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ((id > 1 OR name = 'x') AND id = 3)")]]);
}

#[test]
fn a_chip_taken_out_leaves_the_others_as_written() {
    let mut h = users();
    h.command("where id  >  1 AND lower(name) = 'x'");
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    // Columns, the right pane, then the `Where` chips: `x` takes the first out.
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Char('x'));
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (lower(name) = 'x')")]]);
}

#[test]
fn a_condition_without_its_value_is_not_sent() {
    let mut h = users();
    h.command("where name = 'x'");
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    condition(&mut h, 0, "");
    assert!(h.screen(120, 34).contains("incomplete"), "{}", h.screen(120, 34));
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name = 'x')")]]);
}

#[test]
fn undo_goes_back_a_step_then_to_the_query_before_the_last_change() {
    let mut h = users();
    h.keys("f");
    condition(&mut h, 0, "12");
    // Typing in one field is one step.
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert!(!h.screen(120, 34).contains("id = 12"));
    h.key(KeyCode::Esc);
    assert!(!open(&h));
    assert!(runs(&mut h).is_empty(), "Esc sends nothing");
    // The filter applied, then cleared from the grid: `f`, `u`, `Enter` brings it back.
    h.command("where id > 1");
    runs(&mut h);
    answer(&mut h);
    h.keys(" rx");
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
    answer(&mut h);
    h.keys("f");
    h.keys("u");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1)")]]);
}

#[test]
fn esc_drops_the_draft() {
    let mut h = users();
    h.keys("f");
    condition(&mut h, 0, "5");
    h.key(KeyCode::Esc);
    assert!(!open(&h));
    assert!(runs(&mut h).is_empty());
    h.keys("f");
    assert!(!h.screen(120, 34).contains("id = 5"), "the draft was dropped");
}

#[test]
fn a_limit_that_is_not_a_number_keeps_the_dialog_open_and_says_why() {
    let mut h = users();
    h.keys("f");
    // Columns, the right pane, then the `Limit` field (no chips, no keys yet).
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
    h.type_text("ten");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert!(open(&h), "a refusal keeps the dialog open");
    assert!(h.screen(120, 34).contains("Limit refused"), "{}", h.screen(120, 34));
    h.key_mod(KeyCode::Char('u'), KeyModifiers::CONTROL);
    h.type_text("10");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h).len(), 1);
    assert!(!open(&h));
}

#[test]
fn a_busy_tab_keeps_the_dialog_open() {
    let mut h = users();
    h.command("where id > 1");
    runs(&mut h);
    // On `name`: a column without a condition.
    h.key(KeyCode::Right);
    h.keys("f");
    condition(&mut h, 0, "b");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert!(open(&h));
    assert!(h.screen(120, 34).contains("already running"), "{}", h.screen(120, 34));
}

#[test]
fn pending_edits_are_asked_about_above_the_dialog_and_a_no_keeps_it() {
    let mut h = users();
    // A row deleted: a pending change of the rows held.
    if let datarig_tui::app::Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[0], |_| Vec::new()));
    }
    h.keys("f");
    condition(&mut h, 0, "1");
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.confirm().is_some_and(|c| matches!(c.action, ConfirmAction::LeaveEdits { .. })));
    assert!(open(&h), "under the question");
    assert!(runs(&mut h).is_empty());
    // `Enter` keeps the edits: no run, the dialog with its draft.
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert!(open(&h));
    assert!(h.screen(120, 34).contains("id = 1"));
    // Asked again, `y` leaves the edits: the statement is sent and the dialog closes.
    h.key(KeyCode::Enter);
    h.key(KeyCode::Char('y'));
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id = 1)")]]);
    assert!(!open(&h));
}

#[test]
fn a_server_error_closes_the_dialog_and_f_shows_it() {
    let mut h = users();
    h.keys("f");
    condition(&mut h, 0, "1");
    h.key(KeyCode::Enter);
    runs(&mut h);
    assert!(!open(&h));
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let error = datarig_core::driver::DbError::Server(datarig_core::driver::ServerMessage {
        text: "permission denied for table users".into(),
        position: None,
        hint: None,
    });
    h.tab_db(index, DbEvent::Failed { id, error, cancelled: false });
    h.keys("f");
    let screen = h.screen(120, 34);
    assert!(screen.contains("permission denied for table users"), "{screen}");
    assert!(screen.contains("id = 1"), "the draft is the failed request: {screen}");
}

#[test]
fn the_dialog_screens() {
    let mut h = users();
    keys_came(&mut h, "local-pg", &["id"]);
    h.command("where id > 1 OR name = 'x'");
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    assert_screen!("filter_dialog_column_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_dialog_column_en_120x34", Lang::En, h.draw(120, 34));
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.key(KeyCode::Left);
    h.key(KeyCode::Char(' '));
    assert_screen!("filter_dialog_operator_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_dialog_operator_en_120x34", Lang::En, h.draw(120, 34));
    h.key(KeyCode::Esc);
    h.key(KeyCode::Right);
    h.type_text("4");
    assert_screen!("filter_dialog_typing_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_dialog_typing_en_120x34", Lang::En, h.draw(120, 34));
    h.key(KeyCode::Down);
    h.key(KeyCode::Down);
    h.key(KeyCode::Right);
    assert_screen!("filter_dialog_sort_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_dialog_sort_en_120x34", Lang::En, h.draw(120, 34));
    to_columns(&mut h);
    h.keys("/");
    h.type_text("na");
    assert_screen!("filter_dialog_search_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_dialog_search_en_120x34", Lang::En, h.draw(120, 34));
    h.key(KeyCode::Enter);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Tab);
    h.type_text("x");
    h.key(KeyCode::Enter);
    assert_screen!("filter_dialog_refused_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_dialog_refused_en_120x34", Lang::En, h.draw(120, 34));
}

#[test]
fn space_r_x_clears_the_filter_and_the_order_and_runs_again() {
    let mut h = users();
    h.command("where id > 1");
    h.command("orderby name desc");
    runs(&mut h);
    answer(&mut h);
    h.keys(" rx");
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
}

#[test]
fn the_keys_say_why_on_a_querys_rows() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    for keys in ["f", " rf", " ro", " rx", " rv"] {
        h.keys(keys);
        assert_eq!(h.app.key_context(), Ctx::Grid, "{keys:?}");
        let status = h.status(120, 30);
        assert!(status.contains("Only a table tab has a filter dialog"), "{keys:?}: {status}");
    }
    assert!(runs(&mut h).is_empty());
    assert_eq!(h.app.tabs.len(), 1);
}

#[test]
fn space_r_v_opens_the_statement_in_a_console() {
    let mut h = users();
    keys_came(&mut h, "local-pg", &["id"]);
    h.command("where id > 1");
    runs(&mut h);
    answer(&mut h);
    h.command("orderby name desc");
    runs(&mut h);
    answer(&mut h);
    let tabs = h.app.tabs.len();
    h.keys(" rv");
    assert_eq!(h.app.tabs.len(), tabs + 1);
    assert!(!h.app.tab().is_table());
    assert_eq!(h.app.tab().editor.text(), format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" DESC, "id" ASC"#));
    assert!(runs(&mut h).is_empty(), "opened, not run");
}

#[test]
fn a_paste_is_one_line_and_has_a_cap() {
    let mut h = users();
    h.key(KeyCode::Right);
    h.keys("f");
    condition(&mut h, 0, "");
    h.event(ratatui::crossterm::event::Event::Paste("a\nb\r\nc".into()));
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name = 'a b  c')")]]);
    let mut h = users();
    h.keys("f");
    condition(&mut h, 0, "");
    h.event(ratatui::crossterm::event::Event::Paste("x".repeat(20_000)));
    assert!(h.screen(120, 34).contains("Pasted text cut"), "{}", h.screen(120, 34));
}

#[test]
fn the_search_prompt_says_it_searches_the_held_rows_of_a_filter() {
    let mut h = users();
    h.keys("/");
    assert!(!h.screen(120, 30).contains("searching held rows"));
    h.key(KeyCode::Esc);
    h.command("where id > 1");
    answer(&mut h);
    h.keys("/");
    assert!(h.screen(120, 30).contains("searching held rows"), "{}", h.screen(120, 30));
}
