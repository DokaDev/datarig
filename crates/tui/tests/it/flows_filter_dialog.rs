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
    assert_eq!(h.app.filter_dialog_popup(), None, "`f` opens the whole dialog");
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

/// An empty string is a value: changing `name = ''` to `in list` or `not in list` keeps it (the
/// condition is not left out), and back again keeps an empty first item of the list.
#[test]
fn an_empty_string_value_survives_a_change_of_operator() {
    // The operator picker of a text column: =, ≠, ≠ or NULL, <, ≤, >, ≥, contains, starts with,
    // in list, not in list.
    let change = |h: &mut Harness, from: usize, to: usize| {
        // From the value's start back to its operator, its picker open.
        h.key(KeyCode::Home);
        h.key(KeyCode::Left);
        h.key(KeyCode::Char(' '));
        for _ in from..to {
            h.key(KeyCode::Down);
        }
        for _ in to..from {
            h.key(KeyCode::Up);
        }
        h.key(KeyCode::Enter);
    };
    for (to, term) in [(9, "name IN ('')"), (10, "name NOT IN ('')")] {
        let mut h = users();
        h.keys("f");
        h.key(KeyCode::Char('j'));
        condition(&mut h, 0, "''");
        assert_eq!(h.app.filter_dialog_draft().unwrap().0, "name = ''");
        change(&mut h, 0, to);
        assert_eq!(h.app.filter_dialog_draft().unwrap().0, term, "{}", h.screen(120, 34));
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({term})")]]);
    }
    let mut h = users();
    h.keys("f");
    h.key(KeyCode::Char('j'));
    condition(&mut h, 9, "'', 'a'");
    assert_eq!(h.app.filter_dialog_draft().unwrap().0, "name IN ('', 'a')");
    change(&mut h, 9, 0);
    assert_eq!(h.app.filter_dialog_draft().unwrap().0, "name = ''", "{}", h.screen(120, 34));
}

/// An empty string is a value across the pattern boundary too: `= ''` changed to `contains` or
/// `starts with` keeps it (an empty pattern), and an empty pattern changed to `=` or a list
/// keeps it; only a value never given leaves the condition incomplete.
#[test]
fn an_empty_string_survives_a_change_between_patterns_and_comparisons() {
    // The operator picker of a text column: =, ≠, ≠ or NULL, <, ≤, >, ≥, contains, starts with,
    // in list, not in list.
    let change = |h: &mut Harness, from: usize, to: usize| {
        h.key(KeyCode::Home);
        h.key(KeyCode::Left);
        h.key(KeyCode::Char(' '));
        for _ in from..to {
            h.key(KeyCode::Down);
        }
        for _ in to..from {
            h.key(KeyCode::Up);
        }
        h.key(KeyCode::Enter);
    };
    let collation_known = |h: &mut Harness| {
        let keys = datarig_core::driver::keys::OrderKeys {
            primary_key: vec!["id".into()],
            deterministic: [("name".to_string(), true)].into(),
            ..Default::default()
        };
        keys_came_with(h, "local-pg", keys);
        h.key(KeyCode::Right);
    };
    let contains = "name ILIKE '%%' ESCAPE '!'";
    let starts = "name LIKE '%' ESCAPE '!'";
    for (from, to, first, then) in [
        (0, 7, "name = ''", contains),
        (0, 8, "name = ''", starts),
        (9, 7, "name IN ('')", contains),
        (10, 8, "name NOT IN ('')", starts),
        (7, 0, contains, "name = ''"),
        (8, 0, starts, "name = ''"),
        (7, 9, contains, "name IN ('')"),
        (8, 10, starts, "name NOT IN ('')"),
    ] {
        let mut h = users();
        collation_known(&mut h);
        h.keys("f");
        h.key(KeyCode::Char('j'));
        condition(&mut h, from, "''");
        assert_eq!(h.app.filter_dialog_draft().unwrap().0, first, "{}", h.screen(120, 34));
        change(&mut h, from, to);
        assert_eq!(h.app.filter_dialog_draft().unwrap().0, then, "{from} -> {to}: {}", h.screen(120, 34));
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({then})")]], "{from} -> {to}");
    }
    // A value never given stays incomplete, across the boundary as within it.
    let mut h = users();
    collation_known(&mut h);
    h.keys("f");
    h.key(KeyCode::Char('j'));
    condition(&mut h, 0, "");
    change(&mut h, 0, 7);
    assert_eq!(h.app.filter_dialog_draft().unwrap().0, "", "{}", h.screen(120, 34));
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

/// `not in list` that keeps NULL rows, its operator picked again as it is (with the picker's
/// `Enter` and with `Space`), still keeps them: the condition and what is sent are as they were.
#[test]
fn not_in_list_keeping_nulls_picked_again_keeps_them() {
    let term = "(name NOT IN ('a', 'b') OR name IS NULL)";
    for pick in [KeyCode::Enter, KeyCode::Char(' ')] {
        let mut h = users();
        h.command(&format!("where {term}"));
        runs(&mut h);
        answer(&mut h);
        h.keys("f");
        h.key(KeyCode::Char('j'));
        // From the column list to the condition's value, back to its operator, its picker open
        // on the operator it has.
        h.key(KeyCode::Right);
        h.key(KeyCode::Right);
        h.key(KeyCode::Home);
        h.key(KeyCode::Left);
        h.key(KeyCode::Char(' '));
        h.key(pick);
        assert_eq!(h.app.filter_dialog_draft().unwrap().0, term, "{pick:?}: {}", h.screen(120, 34));
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({term})")]], "{pick:?}");
    }
}

/// A bound of `between` cleared leaves the other in its own field: the condition waits for the
/// missing bound (it is not sent), and the bound still given is there after the column is left
/// and taken again, so typing the missing one completes the condition.
#[test]
fn a_between_bound_cleared_keeps_the_other_across_a_change_of_column() {
    // (the field of the bound cleared, the bound typed there again, the term sent)
    for (field, typed, term) in [(0, "2", "id BETWEEN 2 AND 10"), (1, "7", "id BETWEEN 1 AND 7")] {
        let mut h = users();
        h.command("where id BETWEEN 1 AND 10");
        runs(&mut h);
        answer(&mut h);
        h.keys("f");
        // The first bound's field, then the one to clear.
        h.key(KeyCode::Right);
        h.key(KeyCode::Right);
        h.key(KeyCode::End);
        for _ in 0..field {
            h.key(KeyCode::Right);
            h.key(KeyCode::End);
        }
        assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
        h.key(KeyCode::Backspace);
        h.key(KeyCode::Backspace);
        assert_eq!(h.app.filter_dialog_draft().unwrap().0, "", "field {field}");
        assert!(h.screen(120, 34).contains("incomplete"), "{}", h.screen(120, 34));
        // Back through the fields and the operator to the column list, another column, then
        // `id` again: its condition's fields as they were left.
        for _ in 0..=field {
            h.key(KeyCode::Home);
            h.key(KeyCode::Left);
        }
        h.key(KeyCode::Left);
        h.key(KeyCode::Char('j'));
        assert!(h.screen(120, 34).contains("\u{2502} name  text"), "{}", h.screen(120, 34));
        h.key(KeyCode::Char('k'));
        let s = h.screen(120, 34);
        assert!(s.contains("\u{2502} id  int8"), "{s}");
        assert!(s.contains("incomplete"), "field {field}: {s}");
        assert!(s.contains(if field == 0 { "10" } else { "1" }), "field {field}: {s}");
        h.key(KeyCode::Right);
        h.key(KeyCode::Right);
        h.key(KeyCode::End);
        for _ in 0..field {
            h.key(KeyCode::Right);
            h.key(KeyCode::End);
        }
        h.type_text(typed);
        assert_eq!(h.app.filter_dialog_draft().unwrap().0, term, "{}", h.screen(120, 34));
        h.key(KeyCode::Enter);
        assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ({term})")]]);
    }
}

/// Taking `between` again in the operator list, with its first bound cleared, changes nothing:
/// the second bound stays in its own field.
#[test]
fn between_picked_again_keeps_a_bound_in_its_own_field() {
    for accept in [KeyCode::Enter, KeyCode::Char(' ')] {
        let mut h = users();
        h.command("where id BETWEEN 1 AND 10");
        runs(&mut h);
        answer(&mut h);
        h.keys("f");
        h.key(KeyCode::Right);
        h.key(KeyCode::Right);
        h.key(KeyCode::End);
        h.key(KeyCode::Backspace);
        assert_eq!(h.app.filter_dialog_draft().unwrap().0, "", "{accept:?}");
        // Back to the operator, its list, and `between` (the one shown) taken again.
        h.key(KeyCode::Home);
        h.key(KeyCode::Left);
        h.key(KeyCode::Char(' '));
        h.key(accept);
        // The pick goes on to the first bound's field.
        assert_eq!(h.app.key_context(), Ctx::FilterDialogText, "{accept:?}: {}", h.screen(120, 34));
        h.key(KeyCode::End);
        h.type_text("2");
        assert_eq!(h.app.filter_dialog_draft().unwrap().0, "id BETWEEN 2 AND 10", "{accept:?}: {}", h.screen(120, 34));
    }
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

/// With the operator picker open only the picker takes the dialog's keys: `X`, `u`, `Ctrl+Z`,
/// `Q`, `#` and `Ctrl+R` leave the draft, the SQL section and the count as they were.
#[test]
fn the_operator_picker_open_keeps_the_dialogs_keys_from_the_dialog_under_it() {
    let mut h = users();
    h.keys("f");
    condition(&mut h, 0, "7");
    assert_eq!(h.app.filter_dialog_draft().unwrap().0, "id = 7");
    // From the value back to its operator, and the picker open.
    h.key(KeyCode::Left);
    h.key(KeyCode::Left);
    h.key(KeyCode::Char(' '));
    let picker = |h: &mut Harness| h.screen(120, 34).contains("between");
    assert!(picker(&mut h));
    h.keys("X");
    h.keys("u");
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    h.keys("Q");
    h.keys("#");
    h.key_mod(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(h.app.filter_dialog_draft().unwrap().0, "id = 7", "the draft is as it was");
    assert!(h.app.filter_dialog_sql_fields().is_none(), "the SQL section stays closed");
    assert!(!h.app.overlays.is_open(OverlayKind::Picker), "no history listed over it");
    assert!(h.sent().is_empty(), "nothing is counted");
    assert!(picker(&mut h), "the picker stays open");
    // `Esc` closes the picker; then `X` is the dialog's again.
    h.key(KeyCode::Esc);
    assert!(!picker(&mut h) && open(&h));
    h.keys("X");
    assert_eq!(h.app.filter_dialog_draft().unwrap().0, "");
}

/// `Space r x`, `:filter clear`, `:where` and `:orderby` on a busy tab are refused before
/// anything changes: the query its next run sends is the one it had.
#[test]
fn a_clear_on_a_busy_tab_changes_nothing() {
    let mut h = users();
    h.command("where id > 1");
    assert_eq!(runs(&mut h).len(), 1);
    // The run waits for its rows: the tab is busy.
    for clear in [" rx", ":filter clear", ":where id > 5", ":orderby name"] {
        match clear.strip_prefix(':') {
            Some(c) => h.command(c),
            None => h.keys(clear),
        }
        assert!(runs(&mut h).is_empty(), "{clear}: nothing is sent");
        assert!(h.screen(120, 34).contains("already running"), "{clear}: {}", h.screen(120, 34));
    }
    answer(&mut h);
    h.keys("f");
    assert_eq!(h.app.filter_dialog_draft().unwrap(), ("id > 1".to_string(), String::new()), "the draft is as it was");
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

/// The whole dialog is centred, 92% x 90% of the screen, each side never smaller than the
/// smallest screen's (80 x 24): on that screen it takes it all. Around it the screen stays,
/// dimmed (the `terminal` theme dims with the modifier); inside nothing is dimmed.
#[test]
fn the_dialog_is_centred_over_the_screen_dimmed() {
    use ratatui::style::Modifier;
    // (screen, the dialog's box)
    let cases = [((120, 34), (5, 2, 110, 30)), ((80, 24), (0, 0, 80, 24)), ((84, 26), (2, 1, 80, 24))];
    for terminal in [false, true] {
        let mut h = users();
        if terminal {
            h.app.theme = std::sync::Arc::new(datarig_tui::theme::TERMINAL.clone());
        }
        let before: Vec<_> = cases.iter().map(|&((w, hh), _)| h.draw(w, hh).backend().buffer().clone()).collect();
        h.keys("f");
        for (((w, hh), (x, y, bw, bh)), before) in cases.into_iter().zip(before) {
            let term = h.draw(w, hh);
            let buf = term.backend().buffer();
            let what = format!("{w}x{hh}, terminal theme {terminal}");
            assert_eq!(buf[(x, y)].symbol(), "\u{256d}", "{what}");
            assert_eq!(buf[(x + bw - 1, y)].symbol(), "\u{256e}", "{what}");
            assert_eq!(buf[(x, y + bh - 1)].symbol(), "\u{2570}", "{what}");
            assert_eq!(buf[(x + bw - 1, y + bh - 1)].symbol(), "\u{256f}", "{what}");
            for (cx, cy) in [(x + 2, y + 3), (x + bw / 2, y + bh / 2)] {
                assert!(!buf[(cx, cy)].modifier.contains(Modifier::DIM), "{what}: inside at {cx},{cy}");
            }
            if (bw, bh) == (w, hh) {
                continue;
            }
            // Outside: the screen as it was (its text), dimmed.
            let (ox, oy) = (x.saturating_sub(1), hh - 1);
            let (now, was) = (&buf[(ox, oy)], &before[(ox, oy)]);
            assert_eq!(now.symbol(), was.symbol(), "{what}: the screen stays behind");
            match terminal {
                true => assert!(now.modifier.contains(Modifier::DIM), "{what}"),
                false => assert_ne!((now.fg, now.bg), (was.fg, was.bg), "{what}: dimmed"),
            }
        }
    }
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
    // Past `+ add condition` and the column's three values, the sort line.
    for _ in 0..5 {
        h.key(KeyCode::Down);
    }
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

/// The table's catalogue read while the dialog is open: its columns become the catalogue's (in
/// the table's order, with their types), the selected column kept by its name.
#[test]
fn the_columns_follow_the_catalogue_read_while_the_dialog_is_open() {
    use datarig_core::driver::keys::{CatalogColumn, OrderKeys};
    let mut h = users();
    h.key(KeyCode::Right);
    h.keys("f");
    let screen = h.screen(120, 34);
    assert!(screen.contains("Columns 2") && screen.contains("from the loaded result"), "{screen}");
    let col = |name: &str, ty: &str| CatalogColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: name == "id",
        primary_key: name == "id",
        foreign_key: false,
        invisible: false,
    };
    let keys = OrderKeys {
        primary_key: vec!["id".into()],
        columns: Some(vec![col("id", "bigint"), col("email", "character varying(200)"), col("name", "text")]),
        ..Default::default()
    };
    keys_came_with(&mut h, "local-pg", keys);
    // A screen whose centred dialog is as wide as the whole one at 120 columns (room for the
    // whole type).
    let screen = h.screen(132, 34);
    assert!(screen.contains("Columns 3") && !screen.contains("from the loaded result"), "{screen}");
    assert!(screen.contains("email           varchar(200)"), "{screen}");
    // Still on `name` (now the third column, not the second).
    h.key(KeyCode::Right);
    h.key(KeyCode::Char(' '));
    h.type_text("x");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name = 'x')")]]);
}

#[test]
fn the_sort_line_steps_through_none_ascending_and_descending_both_ways() {
    let mut h = users();
    // `Space r o` with no order, then `A`: the first column's sort line.
    h.keys(" ro");
    h.keys("A");
    let order = |h: &Harness| h.app.filter_dialog_draft().unwrap().1;
    h.key(KeyCode::Right);
    assert_eq!(order(&h), "id ASC");
    h.key(KeyCode::Right);
    assert_eq!(order(&h), "id DESC");
    h.key(KeyCode::Right);
    assert_eq!(order(&h), "id DESC", "the last choice stays");
    h.key(KeyCode::Left);
    assert_eq!(order(&h), "id ASC", "back from descending to ascending");
    h.key(KeyCode::Left);
    assert_eq!(order(&h), "");
    h.key(KeyCode::Left);
    assert_eq!(order(&h), "");
}

/// In the open dialog, on column `name` (second in the list): the value field of its first
/// condition, the cursor at its end.
fn name_values(h: &mut Harness) {
    h.key(KeyCode::Char('j'));
    h.key(KeyCode::Right);
    h.key(KeyCode::Right);
    h.key(KeyCode::End);
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
}

#[test]
fn a_value_added_to_a_list_keeps_its_null_and_its_values_with_commas() {
    // A list with NULL among its values: adding one keeps the NULL, not the text 'NULL'.
    let mut h = users();
    h.command("where (name IN ('paid') OR name IS NULL)");
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    name_values(&mut h);
    h.type_text(", refunded");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE ((name IN ('paid', 'refunded') OR name IS NULL))")]]);
    // A value holding a comma stays one value when another is changed.
    let mut h = users();
    h.command("where name IN ('a,b', 'x')");
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    name_values(&mut h);
    h.type_text("y");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name IN ('a,b', 'xy'))")]]);
    // Empty text, blanks at the edges and the text 'NULL' stay what they were.
    let mut h = users();
    h.command("where name IN ('', ' a ', 'NULL', 'it''s')");
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    name_values(&mut h);
    h.type_text(", z");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (name IN ('', ' a ', 'NULL', 'it''s', 'z'))")]]);
}

/// The row with the keys (`▸`) on screen `s`, when one is drawn right of the column list.
fn focused_row(s: &str) -> Option<&str> {
    s.lines().find(|l| l.split('\u{2502}').nth(2).is_some_and(|r| r.contains('\u{25b8}')))
}

#[test]
fn many_conditions_scroll_to_the_one_with_the_keys() {
    let mut h = users();
    let terms: Vec<String> = (1..=20).map(|i| format!("id > {}", i * 100)).collect();
    h.command(&format!("where {}", terms.join(" AND ")));
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    // The last condition: on screen, with the keys.
    h.key(KeyCode::Right);
    for _ in 0..19 {
        h.key(KeyCode::Char('j'));
    }
    let s = h.screen(80, 24);
    assert!(focused_row(&s).is_some_and(|l| l.contains("2000")), "{s}");
    assert_screen!("filter_dialog_many_en_80x24", Lang::En, h.draw(80, 24));
    assert_screen!("filter_dialog_many_en_120x34", Lang::En, h.draw(120, 34));
    // Its operator picker opens where it is.
    h.key(KeyCode::Char(' '));
    let s = h.screen(80, 24);
    assert!(s.contains("between"), "{s}");
    h.key(KeyCode::Esc);
    // `+ add condition` under it.
    h.key(KeyCode::Char('j'));
    let s = h.screen(80, 24);
    assert!(focused_row(&s).is_some_and(|l| l.contains("+ add condition")), "{s}");
    // The SQL view takes the pane: its lines go out of reach (the keys to the `SQL` toggle), and
    // are back once it closes.
    h.key(KeyCode::Char('Q'));
    let s = h.screen(80, 24);
    assert!(!s.contains("+ add condition") && focused_row(&s).is_none(), "{s}");
    h.key(KeyCode::Right);
    h.key(KeyCode::Char('Q'));
    assert!(h.app.filter_dialog_sql_fields().is_some());
    h.key(KeyCode::Esc);
    assert!(h.app.filter_dialog_sql_fields().is_none());
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::FilterDialog);
    h.key(KeyCode::Right);
    for _ in 0..5 {
        h.key(KeyCode::Char('j'));
    }
    let s = h.screen(80, 24);
    assert!(focused_row(&s).is_some_and(|l| l.contains("600")), "{s}");
    h.key(KeyCode::Char(' '));
    assert!(h.screen(80, 24).contains("between"), "{}", h.screen(80, 24));
    h.key(KeyCode::Esc);
    for _ in 0..15 {
        h.key(KeyCode::Char('j'));
    }
    let s = h.screen(80, 24);
    assert!(focused_row(&s).is_some_and(|l| l.contains("+ add condition")), "{s}");
    assert_screen!("filter_dialog_many_end_en_80x24", Lang::En, h.draw(80, 24));
}

#[test]
fn the_dialog_takes_nothing_of_the_grid_under_it() {
    use ratatui::style::Modifier;
    let mut h = Harness::connected(Lang::En);
    open_users(&mut h);
    // NULL names: the grid writes NULL in italics.
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = Some(vec![meta("id", "int8", true, false), meta("name", "text", false, false)]);
    let rows = (0..20).map(|i| vec![Some(i.to_string()), None]).collect();
    h.tab_db(index, DbEvent::Page { id, columns, rows, more: false, elapsed: Duration::from_millis(2) });
    h.key(KeyCode::Tab);
    for (w, ht) in [(80, 24), (120, 34)] {
        let t = h.draw(w, ht);
        let italic =
            |b: &ratatui::buffer::Buffer| b.content().iter().filter(|c| c.modifier.contains(Modifier::ITALIC)).count();
        assert!(italic(t.backend().buffer()) > 0, "the grid shows its NULLs in italics");
        h.key(KeyCode::Char('f'));
        assert!(open(&h));
        let t = h.draw(w, ht);
        assert_eq!(italic(t.backend().buffer()), 0, "no italics come through the dialog at {w}x{ht}");
        h.key(KeyCode::Esc);
        assert!(!open(&h));
    }
}

/// PostgreSQL's long type names are listed by their short names, so the list does not cut
/// what tells them apart (with or without a time zone); the heading keeps the long name.
#[test]
fn the_column_list_names_postgresql_types_short() {
    use datarig_core::driver::keys::{CatalogColumn, OrderKeys};
    let mut h = users();
    h.keys("f");
    let col = |name: &str, ty: &str| CatalogColumn {
        name: name.into(),
        type_name: ty.into(),
        not_null: name == "id",
        primary_key: name == "id",
        foreign_key: false,
        invisible: false,
    };
    let keys = OrderKeys {
        primary_key: vec!["id".into()],
        columns: Some(vec![
            col("id", "bigint"),
            col("email", "character varying(200)"),
            col("seen", "timestamp with time zone"),
            col("made", "timestamp(3) without time zone"),
            col("at", "time with time zone[]"),
            col("score", "double precision"),
        ]),
        ..Default::default()
    };
    keys_came_with(&mut h, "local-pg", keys);
    // A screen whose centred dialog is as wide as the whole one at 120 columns (room for the
    // whole type).
    let screen = h.screen(132, 34);
    for (name, ty) in [
        ("email", "varchar(200)"),
        ("seen", "timestamptz"),
        ("made", "timestamp(3)"),
        ("at", "timetz[]"),
        ("score", "float8"),
    ] {
        let line = screen.lines().find(|l| l.contains(&format!(" {name} "))).unwrap_or_else(|| panic!("{screen}"));
        assert!(line.contains(&format!(" {ty} ")), "{name} as {ty}: {screen}");
    }
    for _ in 0..2 {
        h.key(KeyCode::Down);
    }
    assert!(h.screen(132, 34).contains("seen  timestamp with time zone"), "{}", h.screen(132, 34));
}

#[test]
fn undo_keeps_the_value_field_on_its_own_condition() {
    let mut h = users();
    h.command("where id > 77 AND id < 42");
    runs(&mut h);
    answer(&mut h);
    h.keys("f");
    // The first condition's operator: `x` takes it out, the next one's value has the keys.
    h.key(KeyCode::Right);
    h.key(KeyCode::Char('x'));
    h.key(KeyCode::Right);
    assert_eq!(h.app.key_context(), Ctx::FilterDialogText);
    assert_eq!(h.app.filter_dialog_draft().unwrap().0, "id < 42");
    // Undone, the condition taken out comes back before it: the keys stay in `42`.
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    assert_eq!(h.app.filter_dialog_draft().unwrap().0, "id > 77 AND id < 42");
    h.key(KeyCode::End);
    h.type_text("5");
    assert_eq!(h.app.filter_dialog_draft().unwrap().0, "id > 77 AND id < 425");
}

/// A PostgreSQL server whose version did not parse (`18beta1`, `17devel`) is judged for `LIKE`
/// on a nondeterministic collation by the number its metadata session read (180000 takes it,
/// 170000 refuses it), as before, never by the version's text: "starts with" is offered where
/// that read says the server takes it and written with `LIKE`, dimmed where it says not, even
/// on a server that calls itself `18beta1`; "contains" (`ILIKE`) is refused on each.
#[test]
fn a_pg_18_beta_server_still_takes_like_on_a_nondeterministic_collation() {
    use datarig_core::engine::{EngineFacts, Fact, PgFacts, ServerFacts, Topology, Why};
    // `shop.users` open on a server that says `line` and whose key-only read says whether it
    // takes `LIKE` on a nondeterministic collation (`takes`), `name` of such a collation; the
    // dialog open on `name`.
    let open_on = |line: &str, takes: bool| {
        let mut h = users();
        let facts = || {
            DbEvent::Server(std::sync::Arc::new(ServerFacts {
                version: Fact::Unknown(Why::VersionUnknown),
                version_text: Some(line.into()),
                provider: Fact::Unknown(Why::NotChecked),
                topology: Fact::Known(Topology::Direct),
                detail: EngineFacts::Postgres(PgFacts::default()),
            }))
        };
        h.meta_db("local-pg", facts());
        let index = h.app.tabs.active_index();
        h.tab_db(index, facts());
        let keys = datarig_core::driver::keys::OrderKeys {
            primary_key: vec!["id".into()],
            deterministic: [("name".to_string(), false)].into(),
            like_nondeterministic: Some(takes),
            ..Default::default()
        };
        keys_came_with(&mut h, "local-pg", keys);
        h.key(KeyCode::Right);
        h.keys("f");
        h
    };
    for (line, takes) in [("18beta1", true), ("17devel", false), ("18beta1", false)] {
        let mut h = open_on(line, takes);
        h.key(KeyCode::Right);
        h.key(KeyCode::Char(' '));
        let offers = h.app.filter_dialog_offers().expect("the operators of the condition");
        let why =
            |op: &str| offers.iter().find(|(o, _)| o == op).unwrap_or_else(|| panic!("{op}: {offers:?}")).1.clone();
        assert_eq!(
            why("contains").as_deref(),
            Some("PostgreSQL refuses ILIKE on the column's collation"),
            "{line} {takes}"
        );
        let refused = (!takes).then_some("The server refuses LIKE on the column's collation");
        assert_eq!(why("starts with").as_deref(), refused, "{line} {takes}");
        if takes {
            let mut h = open_on(line, takes);
            condition(&mut h, 8, "a_b");
            h.key(KeyCode::Enter);
            assert_eq!(
                runs(&mut h),
                vec![vec![format!("{ALL} WHERE (name LIKE 'a!_b%' ESCAPE '!')")]],
                "{line} {takes}"
            );
        }
    }
}

// ── Column names on MySQL and MariaDB ───────────────────────────────────────────────────────

/// `shop.users` with the columns `id` (a number) and `name` (`date` or `current_role`, of type
/// `ty`), open on a MySQL profile whose session is MariaDB (`maria`) or MySQL, a row of each
/// loaded, the grid focused, nothing sent since.
fn family_table(maria: bool, name: &str, ty: &str) -> Harness {
    use datarig_core::driver::SessionValues;
    use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
    let driver = FakeDriver::default();
    driver.mysql.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut p = datarig_core::profile::ConnectionConfig::test_db();
    (p.name, p.driver) = ("local-my".into(), "mysql".into());
    let cfg = datarig_core::config::Config { connections: vec![p], ..Default::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    let lang = Language::Sql(if maria {
        Dialect::MariaDb(MySqlMode::default())
    } else {
        Dialect::MySql(MySqlMode::default())
    });
    h.db(DbEvent::Language(lang));
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    open_users(&mut h);
    let index = h.app.tabs.active_index();
    h.tab_db(index, DbEvent::Language(lang));
    h.tab_db(index, DbEvent::Connected);
    h.tab_db(index, DbEvent::SessionValues(SessionValues { time_zone: "+00:00".into(), charset: "utf8mb4".into() }));
    family_page(&mut h, name, ty);
    h.key(KeyCode::Tab);
    h
}

/// The page [`family_table`] loads: `id` and `name` of type `ty`.
fn family_page(h: &mut Harness, name: &str, ty: &str) {
    use datarig_core::driver::{ColumnMeta, ValueKind};
    let kind = if ty == "date" { ValueKind::Date } else { ValueKind::Text };
    let id = h.app.tab().exec.query_id;
    let index = h.app.tabs.active_index();
    let columns = vec![
        ColumnMeta::new("id".into(), "int".into(), ValueKind::Integer, None),
        ColumnMeta::new(name.into(), ty.into(), kind, None),
    ];
    let rows = vec![vec![Some("1".into()), None], vec![Some("2".into()), Some("2024-01-01".into())]];
    h.tab_db(index, DbEvent::Page { id, columns: Some(columns), rows, more: false, elapsed: Duration::from_millis(2) });
    h.sent();
}

/// `:where text` run on [`family_table`] of column `name` (type `ty`), its rows loaded again.
fn family_where(h: &mut Harness, name: &str, ty: &str, text: &str) {
    h.command(&format!("where {text}"));
    assert_eq!(runs(h).len(), 1, "{text}");
    family_page(h, name, ty);
}

/// The dialog opened on column `col` of the grid: its count of the column's values (`#`), as
/// the statement sent.
fn family_count(h: &mut Harness, col: usize) -> String {
    h.app.tab_mut().grid.col = col;
    h.keys("f");
    assert!(open(h), "{}", h.screen(120, 34));
    h.keys("#");
    h.sent()
        .into_iter()
        .find_map(|c| match c {
            DbCommand::GroupCount { sql, .. } => Some(sql),
            _ => None,
        })
        .expect("a count is sent")
}

/// In the dialog opened on the grid's `id` column: the first value of its list ticked, the
/// draft's `WHERE` text then (the dialog stays open).
fn family_tick_id(h: &mut Harness, conditions: usize) -> String {
    h.app.tab_mut().grid.col = 0;
    h.keys("f");
    h.key(KeyCode::Right);
    for _ in 0..=conditions {
        h.key(KeyCode::Down);
    }
    h.key(KeyCode::Char(' '));
    h.app.filter_dialog_draft().expect("the dialog is open").0
}

/// MariaDB reads a bare `current_role` as its function: filter text naming it bare is kept as
/// written (a row of another column changed leaves it as it is, and a count of the column's
/// values keeps it among the other conditions); quoted, it is the column's condition.
#[test]
fn a_bare_name_the_server_may_call_stays_as_written() {
    let mut h = family_table(true, "current_role", "varchar(20)");
    family_where(&mut h, "current_role", "varchar(20)", "current_role IS NULL AND id > 0");
    let ticked = family_tick_id(&mut h, 1);
    assert_eq!(ticked, "current_role IS NULL AND id > 0 AND `id` = 1");
    let mut h = family_table(true, "current_role", "varchar(20)");
    family_where(&mut h, "current_role", "varchar(20)", "current_role IS NULL AND id > 0");
    let count = family_count(&mut h, 1);
    assert!(count.contains(" WHERE (current_role IS NULL AND id > 0) GROUP BY `current_role`"), "{count}");
    // Quoted: the column's condition, left out of its own count.
    let mut h = family_table(true, "current_role", "varchar(20)");
    family_where(&mut h, "current_role", "varchar(20)", "`current_role` IS NULL AND id > 0");
    let count = family_count(&mut h, 1);
    assert!(count.contains(" WHERE (id > 0) GROUP BY `current_role`"), "{count}");
}

/// The facts a session says whose greeting names MySQL 8.4.6 and whose `VERSION()` line names
/// MariaDB 11.4.13: its statements are judged on both engines.
fn two_engines_said() -> DbEvent {
    use datarig_core::engine::{EngineFacts, EngineId, Fact, MyFacts, ServerFacts, Version, Why};
    DbEvent::Server(std::sync::Arc::new(ServerFacts {
        version: Fact::Unknown(Why::EnginesDisagree),
        version_text: Some("11.4.13-MariaDB".into()),
        provider: Fact::Unknown(Why::NotChecked),
        topology: Fact::Unknown(Why::EnginesDisagree),
        detail: EngineFacts::MariaDb(MyFacts::new(Version::new(8, 4, 6), EngineId::MySql)),
    }))
}

/// On a MySQL session, a bare `current_role` reads as MariaDB reads it whatever the profile's
/// other sessions say of their server: kept as written when another session reports two engines
/// before the dialog opens, and when the report comes while it is open.
#[test]
fn a_bare_current_role_stays_as_written_when_another_session_reports_two_engines() {
    let kept = " WHERE (current_role IS NULL) GROUP BY `current_role`";
    // Reported before the dialog opens.
    let mut h = family_table(false, "current_role", "varchar(20)");
    h.meta_db("local-my", two_engines_said());
    family_where(&mut h, "current_role", "varchar(20)", "current_role IS NULL");
    let count = family_count(&mut h, 1);
    assert!(count.contains(kept), "{count}");
    // Reported while it is open.
    let mut h = family_table(false, "current_role", "varchar(20)");
    family_where(&mut h, "current_role", "varchar(20)", "current_role IS NULL");
    h.app.tab_mut().grid.col = 1;
    h.keys("f");
    assert!(open(&h));
    h.meta_db("local-my", two_engines_said());
    h.keys("#");
    let count = h
        .sent()
        .into_iter()
        .find_map(|c| match c {
            DbCommand::GroupCount { sql, .. } => Some(sql),
            _ => None,
        })
        .expect("a count is sent");
    assert!(count.contains(kept), "{count}");
    assert_eq!(h.app.filter_dialog_draft().expect("the dialog is open").0, "current_role IS NULL");
}

/// A bare `date` names the column on MySQL and MariaDB, as before: a condition on it is the
/// column's (left out of its own count), and a change of another column's row leaves its text
/// as typed.
#[test]
fn a_filter_on_a_bare_date_column_round_trips_as_today() {
    for maria in [false, true] {
        let mut h = family_table(maria, "date", "date");
        family_where(&mut h, "date", "date", "date = '2024-01-01' AND id > 0");
        let ticked = family_tick_id(&mut h, 1);
        assert_eq!(ticked, "date = '2024-01-01' AND id > 0 AND `id` = 1", "{maria}");
        let mut h = family_table(maria, "date", "date");
        family_where(&mut h, "date", "date", "date = '2024-01-01' AND id > 0");
        let count = family_count(&mut h, 1);
        assert!(count.contains(" WHERE (id > 0) GROUP BY `date`"), "{maria}: {count}");
    }
}
