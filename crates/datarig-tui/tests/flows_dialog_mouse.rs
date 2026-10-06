//! The mouse on the dialogs: a click hits what was drawn there and does what the key for it
//! does; the pointer highlights a button without moving the focus (so `Enter` still presses the
//! safe one); a click outside a dialog changes nothing; moves that change nothing draw no frame.

mod common;

use common::*;
use datarig_core::driver::DbEvent;
use datarig_core::i18n::Lang;
use datarig_core::secret::SourceKind;
use datarig_tui::app::overlay::{ConfirmAction, OverlayKind};
use datarig_tui::app::profiles::{Field, FormHit, Section, SshChoice};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use ratatui::style::Modifier;

const W: u16 = 100;
const H: u16 = 30;

/// Where `text` starts on the screen drawn at `W`×`H` (the `n`th match, top to bottom), as
/// columns.
fn find_nth(h: &mut Harness, text: &str, n: usize) -> (u16, u16) {
    let t = h.draw(W, H);
    let buf = t.backend().buffer();
    (0..H)
        .flat_map(|y| {
            let row = row_text(buf, y);
            row.match_indices(text).map(|(i, _)| (datarig_tui::text::width(&row[..i]) as u16, y)).collect::<Vec<_>>()
        })
        .nth(n)
        .unwrap_or_else(|| panic!("no {text:?} on screen:\n{}", h.screen(W, H)))
}

fn find(h: &mut Harness, text: &str) -> (u16, u16) {
    find_nth(h, text, 0)
}

fn click(h: &mut Harness, (x, y): (u16, u16)) {
    h.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
}

/// Click on `text` (its `dx`th column).
fn click_on(h: &mut Harness, text: &str, dx: u16) {
    let (x, y) = find(h, text);
    click(h, (x + dx, y));
}

/// Move the pointer to (x, y); `true` when the move needs a frame.
fn hover(h: &mut Harness, x: u16, y: u16) -> bool {
    h.mouse(MouseEventKind::Moved, x, y);
    !h.app.take_idle_event()
}

fn wheel(h: &mut Harness, down: bool, (x, y): (u16, u16)) {
    h.mouse(if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp }, x, y);
}

/// The sample profiles at launch, the cursor on the first.
fn launched() -> Harness {
    Harness::launched(
        &sample_config(None),
        Lang::En,
        std::sync::Arc::new(datarig_core::secret::MemoryStore::new()),
        datarig_tui::app::Startup::Normal,
    )
}

fn focus(h: &Harness) -> Field {
    h.form().focus
}

/// The new-profile form, drawn.
fn new_form() -> Harness {
    let mut h = Harness::connected(Lang::En);
    h.command("new connection");
    assert!(h.form_open());
    h.draw(W, H);
    h
}

#[test]
fn a_click_focuses_a_field_and_puts_the_cursor_under_the_pointer() {
    let mut h = new_form();
    assert_eq!(focus(&h), Field::Name);
    // The label, and anywhere else on the field's line.
    click_on(&mut h, "Host ", 0);
    assert_eq!(focus(&h), Field::Host);
    click_on(&mut h, "User ", 1);
    assert_eq!(focus(&h), Field::User);
    // Inside a text: the cursor goes there, and typing goes in there.
    click_on(&mut h, "localhost", 3);
    assert_eq!(focus(&h), Field::Host);
    assert_eq!(h.form().host.cursor(), 3);
    h.type_text("X");
    assert_eq!(h.form().host.text(), "locXalhost");
    // Past the text: its end.
    let (x, y) = find(&mut h, "locXalhost");
    click(&mut h, (x + 20, y));
    assert_eq!(h.form().host.cursor(), 10);
    // A masked password: one column a letter.
    let (px, _) = find(&mut h, "locXalhost");
    let (_, py) = find(&mut h, "Password   ");
    click(&mut h, (px, py));
    assert_eq!(focus(&h), Field::Password);
    h.type_text("abcd");
    click(&mut h, (px + 1, py));
    h.type_text("Z");
    assert_eq!(h.form().password.text(), "aZbcd");
    // The DSN line, by its input.
    click_on(&mut h, "postgres://", 2);
    assert_eq!(focus(&h), Field::Dsn);
    assert_eq!(h.form().dsn.cursor(), 2);
}

#[test]
fn a_click_on_a_choice_picks_it_as_the_arrows_would() {
    let mut h = new_form();
    assert_eq!(h.form().source, SourceKind::Keychain);
    click_on(&mut h, "Command ", 1);
    assert_eq!((h.form().source, focus(&h)), (SourceKind::Command, Field::Source));
    click_on(&mut h, "File ", 0);
    assert_eq!(h.form().source, SourceKind::File);
    // A driver the app has no driver for is not picked.
    click_on(&mut h, "MySQL", 1);
    assert_eq!((h.form().driver, focus(&h)), (0, Field::Driver));
}

#[test]
fn section_tabs_selectors_and_the_ssh_choices_take_clicks() {
    let mut h = new_form();
    click_on(&mut h, " SSH ", 1);
    assert_eq!(h.form().section, Section::Ssh);
    assert_eq!(focus(&h), Field::SshEnabled);
    // `›` is the next value, `‹` the previous one.
    click_on(&mut h, "‹ off ›", 6);
    assert_eq!(h.form().ssh_choice(), SshChoice::Inline);
    click_on(&mut h, "password ", 1);
    assert_eq!((h.form().ssh_auth, focus(&h)), (datarig_core::profile::ssh::SshAuth::Password, Field::SshAuth));
    click_on(&mut h, "‹ this profile only ›", 0);
    assert_eq!(h.form().ssh_choice(), SshChoice::Off);
    // The value itself: the next one.
    click_on(&mut h, "‹ off ›", 3);
    assert_eq!(h.form().ssh_choice(), SshChoice::Inline);
    click_on(&mut h, " Advanced ", 2);
    assert_eq!(h.form().section, Section::Advanced);
    let sslmode = h.form().sslmode;
    click_on(&mut h, "‹ prefer ›", 9);
    assert_eq!(h.form().sslmode, sslmode + 1);
    assert_eq!(focus(&h), Field::SslMode);
    click_on(&mut h, "‹ on ›", 3);
    assert!(!h.form().statement_cache);
    click_on(&mut h, " Basic ", 2);
    assert_eq!(h.form().section, Section::Basic);
}

/// A picker's value opens its list; a click on a row picks it; the wheel moves the selection
/// and the pointer selects the row under it.
#[test]
fn a_pickers_list_takes_clicks_the_wheel_and_the_pointer() {
    let mut h = new_form();
    click_on(&mut h, " Advanced ", 2);
    click_on(&mut h, "auto (from the name)", 2);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Chooser));
    h.draw(W, H);
    let list = h.app.overlays.chooser().unwrap().list;
    let sel = |h: &Harness| h.app.overlays.chooser().unwrap().selected;
    assert_eq!(sel(&h), 0);
    wheel(&mut h, true, (list.x + 2, list.y));
    assert_eq!(sel(&h), 1);
    assert!(hover(&mut h, list.x + 2, list.y + 3), "another row: a frame");
    assert_eq!(sel(&h), 3);
    assert!(!hover(&mut h, list.x + 5, list.y + 3), "the same row: none");
    assert!(!hover(&mut h, 0, 0), "off the list: none");
    let value = h.app.overlays.chooser().unwrap().items[3].0.clone();
    click(&mut h, (list.x + 2, list.y + 3));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm));
    assert_eq!(h.form().color, value);
    assert!(value.is_some());
}

#[test]
fn the_buttons_press_test_save_and_cancel() {
    let mut h = new_form();
    // Save without a name: refused, the form stays and says why.
    click_on(&mut h, " Save ", 2);
    assert!(h.form_open() && h.form().attempted);
    assert_eq!(focus(&h), Field::Save);
    click_on(&mut h, "Name ", 0);
    h.type_text("tested");
    click_on(&mut h, "User ", 0);
    h.type_text("app");
    click_on(&mut h, " Test ", 2);
    assert!(h.app.conn_test.is_some(), "a test started");
    click_on(&mut h, " Cancel ", 2);
    assert!(!h.form_open());
    // Saved by a click once it is filled in.
    let mut h = new_form();
    h.type_text("clicked");
    click_on(&mut h, "User ", 0);
    h.type_text("app");
    click_on(&mut h, " Save ", 2);
    assert!(!h.form_open());
    assert!(h.app.profiles.iter().any(|p| p.name == "clicked"));
}

#[test]
fn a_click_outside_the_form_or_on_nothing_changes_nothing() {
    let mut h = new_form();
    h.type_text("kept");
    let before = h.screen(W, H);
    for p in [(0, 0), (1, 2), (W - 1, H - 1), (50, 1)] {
        click(&mut h, p);
        h.mouse(MouseEventKind::Down(MouseButton::Right), p.0, p.1);
        wheel(&mut h, true, p);
    }
    assert!(h.form_open());
    assert_eq!(h.form().name.text(), "kept");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ProfileForm), "no menu opened below");
    assert_eq!(h.screen(W, H), before);
}

/// The pointer underlines the button under it; the focus stays; a move along it, or off the
/// buttons, draws no frame unless the highlight goes.
#[test]
fn the_pointer_highlights_the_forms_buttons_only() {
    let mut h = new_form();
    let (x, y) = find(&mut h, " Save ");
    assert!(hover(&mut h, x + 1, y));
    assert_eq!(h.form().hover, Some(FormHit::Button(Field::Save)));
    assert_eq!(focus(&h), Field::Name, "the focus stays");
    let t = h.draw(W, H);
    assert!(t.backend().buffer()[(x + 2, y)].modifier.contains(Modifier::UNDERLINED));
    assert!(!hover(&mut h, x + 3, y), "along the same button: no frame");
    let (hx, hy) = find(&mut h, "Host ");
    assert!(hover(&mut h, hx, hy), "off the button: the highlight goes");
    assert_eq!(h.form().hover, None);
    assert!(!hover(&mut h, hx + 1, hy), "a field is no button: nothing");
    let t = h.draw(W, H);
    assert!(!t.backend().buffer()[(x + 2, y)].modifier.contains(Modifier::UNDERLINED));
}

/// The quit confirmation: Stay is focused, Quit is a button; the pointer on Quit does not move
/// the focus, so Enter still stays; a click on Quit quits, one on Stay stays.
#[test]
fn a_confirmation_acts_only_on_a_click_on_its_button() {
    let open = || {
        let mut h = Harness::connected(Lang::En);
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        h.ctrl('q');
        assert_eq!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::Quit));
        h
    };
    let mut h = open();
    assert!(h.screen(W, H).contains("[ Stay ]     Quit"), "{}", h.screen(W, H));
    let (x, y) = find(&mut h, "Quit  ");
    assert!(hover(&mut h, x, y));
    assert!(!hover(&mut h, x + 1, y));
    insta::assert_snapshot!("confirm_quit_hovered_en_100x30", h.draw(W, H).backend());
    let t = h.draw(W, H);
    let buf = t.backend().buffer();
    assert!(buf[(x, y)].modifier.contains(Modifier::UNDERLINED), "Quit is underlined");
    assert!(!buf[(x, y)].modifier.contains(Modifier::REVERSED), "and not focused");
    let (sx, sy) = find(&mut h, "[ Stay ]");
    assert!(t.backend().buffer()[(sx, sy)].modifier.contains(Modifier::REVERSED), "Stay keeps the focus");
    // Enter after hovering the destructive button still keeps.
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.confirm().is_none() && !h.app.quit);
    // A click on Stay stays; a click outside does nothing.
    let mut h = open();
    let (sx, sy) = find(&mut h, "[ Stay ]");
    click(&mut h, (0, 0));
    click(&mut h, (sx, sy - 1));
    click(&mut h, (sx + 9, sy));
    assert!(h.app.overlays.confirm().is_some(), "outside, above and between the buttons: nothing");
    click_on(&mut h, "[ Stay ]", 2);
    assert!(h.app.overlays.confirm().is_none() && !h.app.quit);
    // A click on Quit quits.
    let mut h = open();
    click_on(&mut h, "Quit  ", 1);
    assert!(h.app.quit);
}

#[test]
fn a_delete_confirmation_deletes_on_a_click_and_keeps_on_cancel() {
    let open = || {
        let mut h = launched();
        h.right_click_row("v6");
        h.menu_pick("Delete connection profile");
        assert!(matches!(h.app.overlays.confirm().map(|c| c.action), Some(ConfirmAction::DeleteProfile(_))));
        h
    };
    let mut h = open();
    let n = h.app.profiles.len();
    click_on(&mut h, "[ Cancel ]", 3);
    assert!(h.app.overlays.confirm().is_none());
    assert_eq!(h.app.profiles.len(), n, "kept");
    let mut h = open();
    click_on(&mut h, "Delete  ", 1);
    assert_eq!(h.app.profiles.len(), n - 1, "deleted");
    assert!(!h.app.profiles.iter().any(|p| p.name == "v6"));
}

fn prompt(h: &Harness) -> &datarig_tui::app::PasswordPrompt {
    h.prompt().expect("prompted")
}

/// The password prompt: the field takes the cursor (masked, one column a letter), the
/// checkbox toggles, Cancel closes and OK sends.
#[test]
fn the_password_prompt_takes_clicks() {
    let mut h = launched();
    h.key(KeyCode::Enter);
    h.db(DbEvent::ConnectFailed { error: "password missing".into(), auth: true });
    h.type_text("abc");
    let (x, y) = find(&mut h, "•••");
    click(&mut h, (x + 1, y));
    h.type_text("Z");
    assert_eq!(prompt(&h).input.text(), "aZbc");
    if prompt(&h).save_to.is_some() {
        let save = prompt(&h).save;
        let b = prompt(&h).checkbox;
        click(&mut h, (b.x + 4, b.y));
        assert_eq!(prompt(&h).save, !save);
        assert!(prompt(&h).save_focus);
        let at = find(&mut h, "••••");
        click(&mut h, at);
        assert!(!prompt(&h).save_focus, "back in the field");
    }
    let (ox, oy) = find(&mut h, "[ OK ]");
    assert!(hover(&mut h, ox + 10, oy), "Cancel under the pointer");
    assert_eq!(prompt(&h).buttons.hover, Some(1));
    click(&mut h, (0, 0));
    assert!(h.prompt().is_some(), "a click outside does nothing");
    click(&mut h, (ox + 10, oy));
    assert!(h.prompt().is_none(), "Cancel closed it");
}

/// The name input: the cursor under the pointer, OK and Cancel; the pointer underlines.
#[test]
fn the_name_input_takes_clicks() {
    let mut h = launched();
    h.keys("N");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::NameInput));
    h.type_text("wrk");
    let (x, y) = find(&mut h, "wrk");
    click(&mut h, (x + 1, y));
    h.type_text("o");
    assert_eq!(h.app.overlays.name_input().unwrap().input.text(), "work");
    let (ox, oy) = find(&mut h, "[ OK ]");
    assert!(hover(&mut h, ox + 10, oy));
    assert!(!hover(&mut h, ox + 11, oy));
    assert_eq!(h.app.overlays.name_input().unwrap().buttons.hover, Some(1));
    click(&mut h, (ox + 1, oy));
    assert!(h.overlay_kind().is_none());
    assert!(h.rows().contains(&"work/".to_string()), "OK made it: {:?}", h.rows());
    h.keys("N");
    let (ox, oy) = find(&mut h, "[ OK ]");
    click(&mut h, (ox + 10, oy));
    assert!(h.overlay_kind().is_none(), "Cancel closed it");
}

/// Quick connect: a click on `▸` lists the profile's databases, one on a row picks it; the
/// wheel and the pointer move the selection.
#[test]
fn quick_connect_takes_clicks_the_wheel_and_the_pointer() {
    let mut h = launched();
    h.ctrl('o');
    assert_eq!(h.overlay_kind(), Some(OverlayKind::QuickConnect));
    h.draw(W, H);
    let q = h.app.overlays.quick().unwrap();
    let (list, n) = (q.list, q.items.len());
    assert!(n >= 3, "{:?}", q.items);
    let sel = |h: &Harness| h.app.overlays.quick().unwrap().selected;
    wheel(&mut h, true, (list.x + 4, list.y));
    assert_eq!(sel(&h), 1);
    assert!(hover(&mut h, list.x + 6, list.y + 2));
    assert_eq!(sel(&h), 2);
    assert!(!hover(&mut h, list.x + 7, list.y + 2));
    // The arrow of the first profile: its databases are asked for (it opens).
    let (arrow, row) = h.app.overlays.quick().unwrap().arrows[0];
    click(&mut h, (arrow.x, arrow.y));
    let q = h.app.overlays.quick().unwrap();
    assert_eq!(q.selected, row);
    assert_eq!(q.open.len(), 1, "opened");
    // A click on the third profile's row picks it: it connects.
    let i = h.app.overlays.quick().unwrap().items.iter().position(|r| {
        matches!(r, datarig_tui::app::quick::QuickRow::Profile(p) if h.app.profile(*p).is_some_and(|p| p.name == "v6"))
    });
    let i = i.expect("v6 listed");
    h.draw(W, H);
    let q = h.app.overlays.quick().unwrap();
    let at = (q.list.x + 8, q.list.y + (i - q.scroll) as u16);
    click(&mut h, at);
    assert!(h.overlay_kind().is_none(), "picked");
    // (The `▸` above asked for local-pg's databases: it connects too.)
    let v6 = h.app.profiles.iter().find(|p| p.name == "v6").unwrap().id;
    assert_eq!(h.app.conns.state(v6), datarig_tui::app::NodeState::Connecting);
}

/// The settings: a click on `›` changes the value, on a row selects it; the wheel and the
/// pointer move the selection.
#[test]
fn the_settings_take_clicks_the_wheel_and_the_pointer() {
    let mut h = Harness::connected(Lang::En);
    h.command("settings");
    h.draw(W, H);
    let rows = h.app.overlays.settings().unwrap().rows.clone();
    let sel = |h: &Harness| h.app.overlays.settings().unwrap().selected;
    // A row with fixed values (not the theme's).
    let fixed = |i: usize| h.app.setting_value(datarig_tui::app::settings::order()[i]).is_some();
    let (line, i, value) = *rows.iter().skip(1).find(|(_, i, _)| fixed(*i)).unwrap();
    click(&mut h, (line.x + 3, line.y));
    assert_eq!(sel(&h), i);
    let k = datarig_tui::app::settings::order()[i];
    let before = h.app.setting_value(k);
    click(&mut h, (value.x + value.width - 1, value.y));
    assert_ne!(h.app.setting_value(k), before, "› changed it");
    click(&mut h, (value.x, value.y));
    assert_eq!(h.app.setting_value(k), before, "‹ changed it back");
    wheel(&mut h, true, (line.x + 3, line.y));
    assert_eq!(sel(&h), i + 1);
    let (line4, i4, _) = *rows.iter().find(|(_, r, _)| *r == i + 3).unwrap();
    assert!(hover(&mut h, line4.x + 3, line4.y));
    assert_eq!(sel(&h), i4);
    assert!(!hover(&mut h, line4.x + 9, line4.y));
    click(&mut h, (0, 0));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Settings), "a click outside closes nothing");
}

/// The command line: a click on an entry runs it; the wheel moves the selection; a click on
/// the input puts the cursor there.
#[test]
fn the_command_line_takes_clicks_and_the_wheel() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::Esc);
    h.keys(":");
    h.type_text("settngs");
    let (x, y) = find(&mut h, "settngs");
    click(&mut h, (x + 4, y));
    h.type_text("i");
    assert_eq!(h.cmdline().unwrap().input.text(), "settings");
    h.draw(W, H);
    let c = h.cmdline().unwrap();
    let (list, selected) = (c.list, c.selected);
    if c.items.len() > 1 {
        wheel(&mut h, true, (list.x + 2, list.y));
        assert_eq!(h.cmdline().unwrap().selected, selected + 1);
        wheel(&mut h, false, (list.x + 2, list.y));
    }
    click(&mut h, (list.x + 2, list.y));
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Settings), "the entry ran");
}

/// The keyboard help: a click on its search line types the filter there.
#[test]
fn a_click_on_the_helps_search_line_starts_the_filter() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::BackTab);
    h.key(KeyCode::F(1));
    h.draw(W, H);
    let list = h.app.overlays.help().unwrap().list;
    click(&mut h, (list.x + 6, list.y - 2));
    assert!(h.app.overlays.help().unwrap().filtering);
    h.type_text("quit");
    assert_eq!(h.app.overlays.help().unwrap().filter.text(), "quit");
}

/// At the smallest size every dialog's parts are hit where they are drawn; on a screen too
/// small to draw anything a click hits nothing.
#[test]
fn a_small_screen_hits_what_is_drawn_and_a_too_small_one_nothing() {
    let mut h = new_form();
    h.draw(80, 24);
    let f = h.form();
    let inner_bottom = f.hits.iter().map(|(r, _)| r.y + r.height).max().unwrap();
    assert!(inner_bottom <= 24);
    let (r, hit) = *f.hits.iter().find(|(_, hit)| *hit == FormHit::Button(Field::Cancel)).unwrap();
    assert_eq!(hit, FormHit::Button(Field::Cancel));
    let t = h.draw(80, 24);
    assert!(row_text(t.backend().buffer(), r.y).contains("Cancel"));
    // Too small: nothing is drawn, and the last frame's places hit nothing.
    h.draw(40, 10);
    click(&mut h, (r.x + 1, r.y));
    assert!(h.form_open(), "not cancelled");
    h.draw(80, 24);
    click(&mut h, (r.x + 1, r.y));
    assert!(!h.form_open());
}

/// Moves over a dialog off its buttons draw no frame and change nothing on screen.
#[test]
fn moves_off_the_buttons_draw_no_frame() {
    let mut h = new_form();
    let before = h.screen(W, H);
    let buttons: Vec<_> = h.form().hits.iter().filter(|(_, x)| x.is_button()).map(|(r, _)| *r).collect();
    let mut frames = 0;
    for y in 0..H {
        for x in 0..W {
            if buttons.iter().any(|r| r.contains(ratatui::layout::Position::new(x, y))) {
                continue;
            }
            frames += usize::from(hover(&mut h, x, y));
        }
    }
    assert_eq!(frames, 0);
    assert_eq!(h.screen(W, H), before);
    // Across the buttons: one frame per button entered and one per button left.
    let (r, _) = *h.form().hits.iter().find(|(_, x)| *x == FormHit::Button(Field::Test)).unwrap();
    let mut frames = 0;
    for x in r.x - 1..r.x + 30 {
        frames += usize::from(hover(&mut h, x, r.y));
    }
    assert_eq!(frames, 6, "Test, Save, Cancel: in and out of each");
}
