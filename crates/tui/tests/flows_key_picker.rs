//! The SSH key file field of the profile form has a file picker (the tree dialog
//! of the saved queries over the file system). Every test runs on a scratch home folder with a
//! `.ssh` of its own; the user's home is never looked at (the app reads `HOME` through its
//! environment lookup, which the tests replace).

mod common;

use common::*;
use datarig_core::i18n::Lang;
use datarig_core::profile::ssh::{SshAuth, SshSettings};
use datarig_core::secret::MemoryStore;
use datarig_tui::app::Startup;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::profiles::Field;
use datarig_tui::app::script_tree::{TreeFocus, TreeMode, TreeRow};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A scratch home folder, removed when dropped.
struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn file(path: &Path, mode: u32) {
    std::fs::write(path, "not a key").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = mode;
}

/// A home folder with `.ssh`: a key, its public half, `known_hosts`, `config`, a key others
/// may read, a PuTTY key and a folder `keys` with one more key.
fn home(tag: &str) -> Home {
    let h = Home(std::env::temp_dir().join(format!("datarig-keys-{tag}-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&h.0);
    let ssh = h.0.join(".ssh");
    std::fs::create_dir_all(ssh.join("keys")).unwrap();
    file(&ssh.join("id_ed25519"), 0o600);
    file(&ssh.join("id_ed25519.pub"), 0o644);
    file(&ssh.join("known_hosts"), 0o644);
    file(&ssh.join("config"), 0o644);
    file(&ssh.join("work.pem"), 0o644);
    file(&ssh.join("old.ppk"), 0o600);
    file(&ssh.join("keys").join("deploy.key"), 0o600);
    h
}

/// The sample profiles, `local-pg` through a tunnel that logs in with a key file, its form
/// open on the key file field; `HOME` is `home`.
fn form(home: &Home, key_file: Option<&str>) -> Harness {
    let mut cfg = sample_config(None);
    cfg.connections[0].ssh = Some(SshSettings {
        enabled: true,
        host: "bastion.example.com".into(),
        user: "ec2-user".into(),
        auth: SshAuth::Key,
        key_file: key_file.map(str::to_string),
        ..SshSettings::default()
    });
    let mut h = Harness::launched(&cfg, Lang::En, Arc::new(MemoryStore::new()), Startup::Normal).with_fake_driver();
    let dir = home.0.to_string_lossy().to_string();
    h.app.set_env_lookup(Arc::new(move |k| (k == "HOME").then(|| dir.clone())));
    h.explore("local-pg");
    h.keys("e");
    h.app.overlays.form_mut().unwrap().focus_field(Field::SshKeyFile);
    h
}

fn key_field(h: &Harness) -> String {
    h.app.overlays.form().unwrap().ssh_key.text().to_string()
}

fn note(h: &Harness) -> Option<String> {
    h.app.overlays.form().unwrap().ssh_key_note.as_ref().map(|m| h.app.i18n.msg(m).to_string())
}

fn picker_root(h: &Harness) -> PathBuf {
    let t = h.app.overlays.script_tree().expect("the picker is open");
    assert_eq!(t.mode, TreeMode::KeyFile);
    t.root.clone()
}

/// The foreground color of the first cell of `text` on screen.
fn color_of(h: &mut Harness, text: &str) -> ratatui::style::Color {
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    for y in 0..40 {
        let row = row_text(buf, y);
        // Whole words only: `id_ed25519` is also the start of `id_ed25519.pub`.
        if let Some(i) = row.find(&format!("{text} ")) {
            let x = datarig_tui::text::width(&row[..i]) as u16;
            return buf[(x, y)].fg;
        }
    }
    panic!("{text} not on screen:\n{}", h.screen(120, 40));
}

#[test]
fn ctrl_o_opens_the_picker_in_ssh_and_enter_picks_a_key() {
    let home = home("pick");
    let mut h = form(&home, None);
    h.ctrl('o');
    assert_eq!(picker_root(&h), home.0.join(".ssh"), "the empty field opens at ~/.ssh");
    let screen = h.screen(120, 40);
    assert!(screen.contains("SSH key file") && screen.contains("~/.ssh/"), "{screen}");
    for name in ["id_ed25519", "id_ed25519.pub", "known_hosts", "config", "work.pem", "old.ppk", "keys/"] {
        assert!(screen.contains(name), "{name} listed:\n{screen}");
    }
    insta::assert_snapshot!("key_picker_en_120x40", h.draw(120, 40).backend());
    insta::assert_snapshot!("key_picker_en_80x24", h.draw(80, 24).backend());
    // Likely keys stand out; the public half, known_hosts and config are dimmed.
    let key = color_of(&mut h, "id_ed25519");
    let public = color_of(&mut h, "id_ed25519.pub");
    assert_eq!(public, color_of(&mut h, "known_hosts"));
    assert_eq!(public, datarig_tui::theme::DARK.fg_dim);
    assert_ne!(key, public);
    // Typing filters; Enter picks the first match.
    h.type_text("ed2");
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::File("id_ed25519".into()));
    h.key(KeyCode::Enter);
    assert!(h.app.overlays.script_tree().is_none());
    assert_eq!(key_field(&h), "~/.ssh/id_ed25519");
    assert_eq!(note(&h), None, "a key only its owner reads");
    assert_eq!(h.app.overlays.form().unwrap().focus, Field::SshKeyFile);
    // Opened again: at the file's folder, on the file.
    h.ctrl('o');
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::File("id_ed25519".into()));
    // Esc keeps the field as it was.
    h.key(KeyCode::Esc);
    assert!(h.app.overlays.script_tree().is_none() && h.app.overlays.form().is_some());
    assert_eq!(key_field(&h), "~/.ssh/id_ed25519");
    h.ctrl('s');
    assert!(h.app.overlays.form().is_none(), "saved");
    assert_eq!(h.app.profiles[0].ssh.as_ref().unwrap().key_file.as_deref(), Some("~/.ssh/id_ed25519"));
}

#[test]
fn a_key_others_may_read_or_a_putty_key_is_said_so_below_the_field() {
    let home = home("warn");
    let mut h = form(&home, None);
    h.ctrl('o');
    h.type_text("work");
    h.key(KeyCode::Enter);
    assert_eq!(key_field(&h), "~/.ssh/work.pem");
    // Unix modes only: Windows has no mode to warn about.
    if cfg!(unix) {
        let n = note(&h).expect("a note");
        assert!(n.contains("mode 644") && n.contains("chmod 600 ~/.ssh/work.pem"), "{n}");
        let screen = h.screen(120, 40);
        assert!(screen.contains("chmod 600"), "shown in the form:\n{screen}");
        // Editing the field clears it.
        h.key(KeyCode::Backspace);
    }
    assert_eq!(note(&h), None);
    h.ctrl('o');
    h.type_text("ppk");
    h.key(KeyCode::Enter);
    assert!(note(&h).is_some_and(|n| n.contains("PuTTY")), "{:?}", note(&h));
}

#[test]
fn the_arrows_open_and_close_a_folder_while_the_empty_filter_has_the_keyboard() {
    let home = home("arrows");
    let mut h = form(&home, None);
    h.ctrl('o');
    let focus = |h: &Harness| h.app.overlays.script_tree().unwrap().focus;
    let rows = |h: &Harness| -> Vec<TreeRow> {
        h.app.overlays.script_tree().unwrap().rows().into_iter().map(|(r, _)| r).collect()
    };
    assert_eq!(focus(&h), TreeFocus::Name, "the filter has the keyboard");
    let keys = TreeRow::Folder("keys".into());
    while h.app.overlays.script_tree().unwrap().row != keys {
        h.key(KeyCode::Down);
    }
    // `keys` was never listed: `→` reads it and opens it.
    h.key(KeyCode::Right);
    assert!(rows(&h).contains(&TreeRow::File("keys/deploy.key".into())), "{:?}", rows(&h));
    h.key(KeyCode::Left);
    assert!(!rows(&h).contains(&TreeRow::File("keys/deploy.key".into())), "`←` closed it");
    assert_eq!(focus(&h), TreeFocus::Name, "the filter keeps the keyboard");
    // With text in the filter they move its cursor.
    h.type_text("de");
    h.key(KeyCode::Left);
    assert_eq!(h.app.overlays.script_tree().unwrap().input.cursor(), 1);
    assert_eq!(picker_root(&h), home.0.join(".ssh"));
}

#[test]
fn folders_open_and_go_up_and_a_typed_path_is_taken() {
    let home = home("nav");
    let mut h = form(&home, None);
    h.ctrl('o');
    h.key(KeyCode::Tab); // the tree
    // `←` on the folder shown goes up to the home folder, `.ssh` selected.
    h.key(KeyCode::Up);
    h.key(KeyCode::Up);
    h.key(KeyCode::Left);
    assert_eq!(picker_root(&h), home.0);
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::Folder(".ssh".into()));
    assert!(h.screen(120, 40).contains("~/"), "the home folder as ~");
    // Into `.ssh/keys` by opening folders.
    h.key(KeyCode::Right);
    h.keys("j");
    let t = h.app.overlays.script_tree().unwrap();
    assert_eq!(t.row, TreeRow::Folder(".ssh/keys".into()), "folders first");
    h.key(KeyCode::Right);
    h.keys("j");
    h.key(KeyCode::Enter);
    assert_eq!(key_field(&h), "~/.ssh/keys/deploy.key");
    // `../` goes up too.
    h.ctrl('o');
    assert_eq!(picker_root(&h), home.0.join(".ssh/keys"));
    h.key(KeyCode::Tab);
    for _ in 0..3 {
        h.key(KeyCode::Up);
    }
    assert_eq!(h.app.overlays.script_tree().unwrap().row, TreeRow::Up);
    h.key(KeyCode::Enter);
    assert_eq!(picker_root(&h), home.0.join(".ssh"));
    // A typed path (with ~) is taken as typed.
    h.key(KeyCode::Tab);
    h.type_text("~/.ssh/id_ed25519");
    h.key(KeyCode::Enter);
    assert_eq!(key_field(&h), "~/.ssh/id_ed25519");
    assert_eq!(note(&h), None);
    // One that is no file says so, and is kept.
    h.ctrl('o');
    h.type_text("~/.ssh/nothing-here");
    h.key(KeyCode::Enter);
    assert_eq!(key_field(&h), "~/.ssh/nothing-here");
    assert!(note(&h).is_some_and(|n| n.contains("is not a file")));
}

/// Typing the path in the field itself still works (no picker).
#[test]
fn a_path_typed_in_the_field_is_saved() {
    let home = home("typed");
    let mut h = form(&home, Some("~/.ssh/id_ed25519"));
    for _ in 0.."~/.ssh/id_ed25519".len() {
        h.key(KeyCode::Backspace);
    }
    h.type_text("~/.ssh/keys/deploy.key");
    h.ctrl('s');
    assert!(h.app.overlays.form().is_none(), "saved");
    assert_eq!(h.app.profiles[0].ssh.as_ref().unwrap().key_file.as_deref(), Some("~/.ssh/keys/deploy.key"));
}

/// The field's `[…]` button opens the picker; the action is in the registry (and so in the
/// command line and the help).
#[test]
fn the_button_opens_the_picker_and_the_action_is_registered() {
    let home = home("button");
    let mut h = form(&home, None);
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let (x, y) = (0..40u16)
        .find_map(|y| {
            let row = row_text(buf, y);
            row.find("[…]").map(|i| (datarig_tui::text::width(&row[..i]) as u16, y))
        })
        .expect("the button");
    // A button: a press and its release, past the arming delay after the form appeared.
    h.advance(std::time::Duration::from_millis(500));
    h.mouse(MouseEventKind::Down(MouseButton::Left), x + 1, y);
    h.mouse(MouseEventKind::Up(MouseButton::Left), x + 1, y);
    assert_eq!(h.app.overlays.top().map(|o| o.kind()), Some(OverlayKind::ScriptTree));
    assert_eq!(picker_root(&h), home.0.join(".ssh"));
    h.key(KeyCode::Esc);
    let spec = datarig_tui::app::action::by_id("form.pick_key_file").expect("registered");
    assert!((spec.when)(&h.app), "the form shows the key file field");
    // Not where the field is not shown (another way to log in).
    h.app.overlays.form_mut().unwrap().ssh_auth = SshAuth::Agent;
    assert!(!(spec.when)(&h.app));
    h.ctrl('o');
    assert!(h.app.overlays.script_tree().is_none(), "Ctrl+O does nothing then");
}

/// Browsing opens no file: a key nobody may read is listed and picked (only its kind and
/// mode are asked for).
#[cfg(unix)]
#[test]
fn browsing_reads_no_file() {
    let home = home("noread");
    let locked = home.0.join(".ssh").join("id_locked");
    file(&locked, 0o000);
    if std::fs::read(&locked).is_ok() {
        return; // root reads anything: nothing to prove
    }
    let mut h = form(&home, None);
    h.ctrl('o');
    assert!(h.screen(120, 40).contains("id_locked"));
    h.type_text("locked");
    h.key(KeyCode::Enter);
    assert_eq!(key_field(&h), "~/.ssh/id_locked");
    assert_eq!(note(&h), None, "its mode is 000: nobody else may read it");
}

/// A click on the picker's filter line puts the keyboard and the cursor there; one on a row
/// gives the keyboard back to the tree.
#[test]
fn a_click_on_the_filter_types_there() {
    use datarig_tui::app::script_tree::TreeFocus;
    let home = home("field");
    let mut h = form(&home, None);
    h.ctrl('o');
    h.key(KeyCode::Tab);
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Tree);
    let t = h.draw(120, 40);
    let buf = t.backend().buffer();
    let (x, y) = (0..40u16)
        .find_map(|y| {
            let row = row_text(buf, y);
            row.find("Filter or path: ").map(|i| (datarig_tui::text::width(&row[..i]) as u16, y))
        })
        .unwrap_or_else(|| panic!("the filter line:\n{}", h.screen(120, 40)));
    h.mouse(MouseEventKind::Down(MouseButton::Left), x + 20, y);
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Name);
    h.type_text("ed2");
    assert_eq!(h.app.overlays.script_tree().unwrap().input.text(), "ed2");
    let list = h.app.overlays.script_tree().unwrap().list;
    h.mouse(MouseEventKind::Down(MouseButton::Left), list.x + 3, list.y);
    assert_eq!(h.app.overlays.script_tree().unwrap().focus, TreeFocus::Tree);
}

/// The folder shown cannot be listed: its row says so, and still does after a key.
#[cfg(unix)]
#[test]
fn an_unreadable_folder_shown_stays_marked() {
    use std::os::unix::fs::PermissionsExt;
    let home = home("locked-root");
    let locked = home.0.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
        return;
    }
    let mut h = form(&home, Some("~/locked/id_x"));
    h.ctrl('o');
    assert_eq!(picker_root(&h), locked);
    h.key(KeyCode::Tab);
    h.key(KeyCode::Down);
    h.key(KeyCode::Up);
    let screen = h.screen(120, 40);
    let ok = screen.lines().any(|l| l.contains("~/locked/") && l.contains("(can't be read)"));
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    assert!(ok, "{screen}");
}

/// The folder the picker opens at does not answer: the picker opens at once, says it is being
/// read, keeps answering keys, and closes on `Esc`; the answer that comes later is dropped.
#[test]
fn the_picker_opens_and_answers_keys_while_its_folder_hangs() {
    use ratatui::crossterm::event::{Event, KeyEvent, KeyModifiers};
    let home = home("hang");
    let mut h = form(&home, None);
    let fake = Arc::new(FakeFs::default());
    fake.hang.lock().unwrap().push(home.0.join(".ssh"));
    h.app.set_file_system(fake.clone());
    let press = |h: &mut Harness, code: KeyCode, m: KeyModifiers| {
        let start = std::time::Instant::now();
        h.app.handle_event(Event::Key(KeyEvent::new(code, m)));
        assert!(start.elapsed() < std::time::Duration::from_secs(2), "waited for the disk");
    };
    press(&mut h, KeyCode::Char('o'), KeyModifiers::CONTROL);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ScriptTree), "open at once");
    let screen = h.screen(120, 40);
    assert!(screen.contains("(reading…)"), "{screen}");
    assert!(!screen.contains("id_ed25519"), "nothing listed yet:\n{screen}");
    // The filter takes keys meanwhile.
    press(&mut h, KeyCode::Char('i'), KeyModifiers::NONE);
    assert_eq!(h.app.overlays.script_tree().unwrap().input.text(), "i");
    press(&mut h, KeyCode::Esc, KeyModifiers::NONE);
    assert!(h.app.overlays.script_tree().is_none(), "closed while the read hangs");
    fake.release();
    h.app.pump_folder_reads(std::time::Duration::from_secs(10));
    assert!(h.app.overlays.script_tree().is_none(), "the late answer opened nothing");
    assert_eq!(key_field(&h), "", "and put nothing in the field");
    // Asked again, it answers.
    h.ctrl('o');
    assert!(h.screen(120, 40).contains("id_ed25519"));
}

/// Closed and opened again while the first read hangs: the first answer is dropped, the second
/// fills the new picker once (nothing twice).
#[test]
fn closed_and_opened_again_while_its_folder_hangs() {
    use ratatui::crossterm::event::{Event, KeyEvent, KeyModifiers};
    let home = home("reopen");
    let mut h = form(&home, None);
    let fake = Arc::new(FakeFs::default());
    fake.hang.lock().unwrap().push(home.0.join(".ssh"));
    h.app.set_file_system(fake.clone());
    let press =
        |h: &mut Harness, code: KeyCode, m: KeyModifiers| h.app.handle_event(Event::Key(KeyEvent::new(code, m)));
    press(&mut h, KeyCode::Char('o'), KeyModifiers::CONTROL);
    press(&mut h, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut h, KeyCode::Char('o'), KeyModifiers::CONTROL);
    fake.release();
    h.app.pump_folder_reads(std::time::Duration::from_secs(10));
    assert_eq!(picker_root(&h), home.0.join(".ssh"));
    let t = h.app.overlays.script_tree().unwrap();
    assert_eq!(t.entries.iter().filter(|e| e.path == "id_ed25519").count(), 1, "{:?}", t.entries);
    assert!(t.disk.waiting.is_empty());
}
