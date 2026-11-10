//! The filter line of a table tab: `f` (or `Space r f`) and `Space r o` focus its condition and
//! its order, `Enter` or `Ctrl+E` apply both (the statement sent is the compiled query), `Esc`
//! puts back what is applied, `Tab` moves between the fields, `Space r x` clears both and runs
//! again, `Space r v` opens the statement in a console; the keys say why on a query's rows. The
//! line shows what is applied (the order as the app writes it, the key that makes it total
//! dimmed), marks edits with `●`, shrinks the order on narrow terminals, and the tab's name and
//! the tab list show the condition.

use crate::common;

use common::*;
use datarig_core::config::IconsSetting;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_i18n::Lang;
use datarig_tui::app::Results;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::table_query::Field;
use datarig_tui::keymap::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use std::time::Duration;

const ALL: &str = r#"SELECT * FROM "shop"."users""#;

fn lang_tag(l: Lang) -> &'static str {
    if l == Lang::En { "en" } else { "ko" }
}

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
fn users(lang: Lang) -> Harness {
    let mut h = Harness::connected(lang);
    open_users(&mut h);
    answer(&mut h);
    h.sent();
    // From the explorer to the grid.
    h.key(KeyCode::Tab);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    h
}

/// The keys of `shop.users` came (its primary key is `id`).
fn structure(h: &mut Harness) {
    keys_came(h, "local-pg", &["id"]);
}

/// `shop.users` filtered on `id > 1` and ordered by `name` descending (the structure read).
fn applied(lang: Lang) -> Harness {
    let mut h = users(lang);
    // The structure read first: the statement sent appends its key (a structure read after
    // the rows came changes only the next statement).
    structure(&mut h);
    h.keys("f");
    h.type_text("id > 1");
    h.key(KeyCode::Tab);
    h.type_text("NAME desc");
    h.key(KeyCode::Enter);
    answer(&mut h);
    h.sent();
    h
}

fn field_text(h: &Harness, f: Field) -> String {
    let line = &h.app.tab().doc.table_query.as_ref().expect("a table query").line;
    line.field(f).text().to_string()
}

fn focus(h: &Harness) -> Option<Field> {
    h.app.tab().doc.table_query.as_ref().and_then(|q| q.line.focus)
}

/// The screen line that holds `text`, and where `text` starts on it.
fn find(h: &mut Harness, w: u16, hh: u16, text: &str) -> (u16, u16) {
    let screen = h.screen(w, hh);
    for (y, line) in screen.lines().enumerate() {
        if let Some(i) = line.find(text) {
            return (line[..i].chars().count() as u16, y as u16);
        }
    }
    panic!("{text:?} is not on the screen:\n{screen}");
}

fn click(h: &mut Harness, (x, y): (u16, u16)) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// Pin the screen at 80 and at 120 columns.
fn pin(h: &mut Harness, name: &str, lang: Lang) {
    let tag = lang_tag(lang);
    assert_screen!(format!("filter_line_{name}_{tag}_80x24"), lang, h.draw(80, 24));
    assert_screen!(format!("filter_line_{name}_{tag}_120x30"), lang, h.draw(120, 30));
}

#[test]
fn f_then_enter_sends_the_condition_as_typed() {
    let mut h = users(Lang::En);
    h.keys("f");
    assert_eq!((h.app.key_context(), focus(&h)), (Ctx::FilterLine, Some(Field::Where)));
    // Letters are typed, not taken for grid keys.
    h.type_text("id > 1 and name <> 'jk'");
    assert!(runs(&mut h).is_empty());
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!("{ALL} WHERE (id > 1 and name <> 'jk')")]]);
    assert_eq!((h.app.key_context(), focus(&h)), (Ctx::Grid, None), "the grid has the keys again");
    answer(&mut h);
    assert_eq!(field_text(&h, Field::Where), "id > 1 and name <> 'jk'");
    // `Space r f` focuses it too.
    h.keys(" rf");
    assert_eq!(focus(&h), Some(Field::Where));
}

#[test]
fn ctrl_e_applies_and_the_order_is_written_back_as_sent() {
    let mut h = users(Lang::En);
    h.keys(" ro");
    assert_eq!((h.app.key_context(), focus(&h)), (Ctx::FilterLine, Some(Field::Order)));
    h.type_text("NAME desc, id");
    h.ctrl('e');
    let sent = h.sent();
    let run: Vec<&Vec<String>> = sent
        .iter()
        .filter_map(|c| if let DbCommand::Execute { statements, .. } = c { Some(statements) } else { None })
        .collect();
    assert_eq!(run, vec![&vec![format!(r#"{ALL} ORDER BY "name" DESC, "id" ASC"#)]]);
    // The keys that make an order total are in the table's structure: asked for when the tab
    // opened, not again while it is being read.
    let asked =
        |c: &DbCommand| matches!(c, DbCommand::LoadStructure { schema, table } if schema == "shop" && table == "users");
    assert_eq!(sent.iter().filter(|c| asked(c)).count(), 0, "{sent:?}");
    assert_eq!(field_text(&h, Field::Order), "name DESC, id ASC");
    assert_eq!(focus(&h), None);
    answer(&mut h);
    // Read already: not asked again.
    structure(&mut h);
    h.keys(" ro");
    h.ctrl('l');
    h.type_text("name");
    h.key(KeyCode::Enter);
    let sent = h.sent();
    assert!(!sent.iter().any(asked), "{sent:?}");
}

#[test]
fn esc_puts_back_what_is_applied() {
    let mut h = applied(Lang::En);
    h.keys("f");
    h.ctrl('l');
    h.type_text("id = 7");
    h.key(KeyCode::Tab);
    h.ctrl('w');
    assert_eq!(field_text(&h, Field::Order), "name ");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.key_context(), Ctx::Grid);
    assert_eq!((field_text(&h, Field::Where), field_text(&h, Field::Order)), ("id > 1".into(), "name DESC".into()));
    assert!(runs(&mut h).is_empty());
}

#[test]
fn tab_and_shift_tab_move_between_the_fields() {
    let mut h = users(Lang::En);
    h.keys("f");
    h.key(KeyCode::Tab);
    assert_eq!(focus(&h), Some(Field::Order));
    h.type_text("id");
    h.key_mod(KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!((h.app.key_context(), focus(&h)), (Ctx::FilterLine, Some(Field::Where)), "not the previous pane");
    h.type_text("id > 0");
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (id > 0) ORDER BY "id" ASC"#)]]);
    // Home and End, `Ctrl+A` and `Ctrl+U` edit as the command line does.
    h.keys("f");
    h.key(KeyCode::Home);
    h.type_text("(");
    h.key(KeyCode::End);
    h.type_text(")");
    assert_eq!(field_text(&h, Field::Where), "(id > 0)");
    h.ctrl('a');
    h.key(KeyCode::Right);
    h.ctrl('u');
    assert_eq!(field_text(&h, Field::Where), "id > 0)");
}

#[test]
fn space_r_x_clears_the_filter_and_the_order_and_runs_again() {
    let mut h = applied(Lang::En);
    h.keys(" rx");
    assert_eq!(runs(&mut h), vec![vec![ALL.to_string()]]);
    answer(&mut h);
    // Not edited: the fields follow what is applied.
    h.draw(120, 30);
    assert_eq!((field_text(&h, Field::Where), field_text(&h, Field::Order)), (String::new(), String::new()));
    // `Ctrl+L` then `Enter` clears one field.
    let mut h = applied(Lang::En);
    h.keys("f");
    h.ctrl('l');
    h.key(KeyCode::Enter);
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} ORDER BY "name" DESC, "id" ASC"#)]]);
}

#[test]
fn pending_edits_are_asked_about_and_enter_keeps_them() {
    let mut h = users(Lang::En);
    if let Results::Rows(rs) = &mut h.app.tab_mut().results {
        assert!(rs.edit.batch.delete(&[0], |_| Vec::new()));
    }
    h.keys("f");
    h.type_text("id > 1");
    h.key(KeyCode::Enter);
    assert_eq!(
        h.app.overlays.confirm().map(|c| c.action),
        Some(ConfirmAction::LeaveEdits(datarig_tui::app::edit::Leave::Run(h.app.tab().id)))
    );
    // Enter keeps the edits: nothing runs, the condition stays typed (and marked as not applied).
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_eq!(field_text(&h, Field::Where), "id > 1");
    assert!(h.screen(120, 30).contains("\u{25cf}WHERE id > 1"), "{}", h.screen(120, 30));
    // `space r x` asks too.
    h.keys(" rx");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    h.key(KeyCode::Esc);
    assert!(runs(&mut h).is_empty());
}

#[test]
fn a_refused_condition_or_order_keeps_the_focus_on_its_field() {
    let mut h = users(Lang::En);
    h.keys("f");
    h.type_text("id = 1; DROP TABLE shop.users");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_eq!((h.app.key_context(), focus(&h)), (Ctx::FilterLine, Some(Field::Where)));
    h.ctrl('l');
    h.key(KeyCode::Tab);
    h.type_text("nosuch");
    h.key(KeyCode::Enter);
    assert!(runs(&mut h).is_empty());
    assert_eq!(focus(&h), Some(Field::Order));
    let r = h.app.tab().doc.table_query.as_ref().and_then(|q| q.refusal.clone()).expect("refusal");
    assert_eq!((r.field, r.at), (Field::Order, Some(0..6)));
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
        assert!(status.contains("Only a table tab has a filter line"), "{keys:?}: {status}");
    }
    assert!(runs(&mut h).is_empty());
    assert_eq!(h.app.tabs.len(), 1);
}

#[test]
fn space_r_v_opens_the_statement_in_a_console() {
    let mut h = applied(Lang::En);
    let tabs = h.app.tabs.len();
    h.keys(" rv");
    assert_eq!(h.app.tabs.len(), tabs + 1);
    assert!(!h.app.tab().is_table());
    assert_eq!(h.app.tab().editor.text(), format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" DESC, "id" ASC"#));
    assert!(runs(&mut h).is_empty(), "opened, not run");
}

#[test]
fn a_paste_is_one_line_and_has_a_cap() {
    let mut h = users(Lang::En);
    h.keys("f");
    h.event(ratatui::crossterm::event::Event::Paste("id > 1\nand\r\nname = 'a'".into()));
    assert_eq!(field_text(&h, Field::Where), "id > 1 and name = 'a'");
    h.ctrl('l');
    h.event(ratatui::crossterm::event::Event::Paste("x".repeat(20_000)));
    assert_eq!(field_text(&h, Field::Where).len(), 16 * 1024);
    assert!(h.status(120, 30).contains("Pasted text cut"), "{}", h.status(120, 30));
}

#[test]
fn a_click_focuses_a_field_and_the_refresh_mark_runs_again() {
    let mut h = applied(Lang::En);
    // On the condition, at the `>`: the cursor goes there.
    let (x, y) = find(&mut h, 120, 30, "id > 1");
    click(&mut h, (x + 4, y));
    assert_eq!((h.app.key_context(), focus(&h)), (Ctx::FilterLine, Some(Field::Where)));
    h.type_text("=");
    assert_eq!(field_text(&h, Field::Where), "id >= 1");
    h.key(KeyCode::Esc);
    let at = find(&mut h, 120, 30, "name DESC");
    click(&mut h, at);
    assert_eq!(focus(&h), Some(Field::Order));
    h.key(KeyCode::Esc);
    let at = find(&mut h, 120, 30, "\u{27f3}");
    click(&mut h, at);
    assert_eq!(runs(&mut h), vec![vec![format!(r#"{ALL} WHERE (id > 1) ORDER BY "name" DESC, "id" ASC"#)]]);
    assert_eq!(focus(&h), None);
}

#[test]
fn the_search_prompt_says_it_searches_the_held_rows_of_a_filter() {
    let mut h = users(Lang::En);
    h.keys("/");
    assert!(!h.screen(120, 30).contains("searching held rows"));
    h.key(KeyCode::Esc);
    let mut h = applied(Lang::En);
    h.keys("/");
    assert!(h.screen(120, 30).contains("searching held rows"), "{}", h.screen(120, 30));
}

#[test]
fn the_filter_line_screens() {
    for lang in [Lang::En, Lang::Ko] {
        // Nothing applied: which key filters.
        let mut h = users(lang);
        pin(&mut h, "empty", lang);
        // Applied: the condition highlighted, the order as sent, `+ id` dimmed (the inspector
        // hidden: its room is the line's); the name marked.
        let mut h = applied(lang);
        h.keys("i");
        pin(&mut h, "applied", lang);
        h.keys("i");
        // Edited, not applied: `●` and the keyword tinted.
        let (x, y) = find(&mut h, 120, 30, "id > 1");
        click(&mut h, (x + 6, y));
        h.type_text(" AND name <> 'x'");
        let at = find(&mut h, 120, 30, "n1");
        click(&mut h, at);
        assert_eq!(h.app.key_context(), Ctx::Grid);
        pin(&mut h, "modified", lang);
        // Focused.
        let mut h = applied(lang);
        h.keys("f");
        pin(&mut h, "focused_where", lang);
        h.key(KeyCode::Tab);
        pin(&mut h, "focused_order", lang);
        // A long condition at 80 columns: the order shrinks to `↓name`, the condition is cut.
        let mut h = applied(lang);
        h.keys("f");
        h.key(KeyCode::End);
        h.type_text(" AND name NOT IN ('alpha', 'beta', 'gamma', 'delta', 'epsilon', 'zeta', 'eta')");
        h.key(KeyCode::Enter);
        answer(&mut h);
        let tag = lang_tag(lang);
        assert_screen!(format!("filter_line_collapsed_{tag}_80x24"), lang, h.draw(80, 24));
        // Icons on: the refresh glyph, and the filter glyph after the tab's name.
        h.app.icons = IconsSetting::On;
        pin(&mut h, "icons", lang);
        h.app.icons = IconsSetting::Off;
        // The tab list shows the condition after the name.
        h.keys(" tt");
        assert_eq!(h.overlay_kind(), Some(OverlayKind::TabList));
        pin(&mut h, "tab_list", lang);
    }
}
