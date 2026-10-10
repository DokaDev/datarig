//! A MySQL profile in the app: what its sessions say about how their text is read reaches the
//! tab's editor and risk classifier.

mod common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbError, DbEvent};
use datarig_core::i18n::Lang;
use datarig_core::secret::PasswordSource;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_tui::app::profiles::{Field, Section, SshChoice};
use ratatui::crossterm::event::KeyCode;
use std::sync::atomic::Ordering;

/// Connecting to a MySQL profile (`local-my`, another one `other-my` too) with the first tab on
/// it (feed `Connected`).
fn mysql() -> Harness {
    let driver = FakeDriver::default();
    driver.mysql.store(true, Ordering::SeqCst);
    let profile = |name: &str| {
        let mut p = datarig_core::profile::ConnectionConfig::test_db();
        p.name = name.into();
        p.driver = "mysql".into();
        p
    };
    let cfg = Config { connections: vec![profile("local-my"), profile("other-my")], ..Config::default() };
    Harness::with_driver(&cfg, Lang::En, driver)
}

fn my(mode: MySqlMode) -> Language {
    Language::Sql(Dialect::MySql(mode))
}

#[test]
fn a_sessions_sql_mode_reaches_the_editor_and_the_classifier() {
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let tab = h.app.tab().id;
    assert_eq!(h.app.tab_language(tab), my(MySqlMode::default()), "the driver's until the session says");
    h.ctrl('e'); // a query session opens for the tab
    assert!(h.roles().contains(&datarig_core::driver::SessionRole::Query), "{:?}", h.roles());
    let ansi = MySqlMode { ansi_quotes: true, dollar_quotes: true, ..MySqlMode::default() };
    h.tab_db(0, DbEvent::Language(my(ansi)));
    assert_eq!(h.app.tab_language(tab), my(ansi));
    assert_eq!(h.app.tab().editor.language(), my(ansi));
    assert_eq!(h.app.tab().exec.prepared.language(), my(ansi));
}

#[test]
fn a_tab_on_another_profile_does_not_keep_the_old_sessions_mode() {
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let tab = h.app.tab().id;
    let id = |h: &Harness, name: &str| h.app.profiles.iter().find(|p| p.name == name).unwrap().id;
    h.ctrl('e');
    let ansi = MySqlMode { ansi_quotes: true, ..MySqlMode::default() };
    h.tab_db(0, DbEvent::Language(my(ansi)));
    assert_eq!(h.app.tab_language(tab), my(ansi));
    // The other profile connects (its own console opens), then the first tab moves to it: the
    // tab reads its text as that profile's sessions do (none said yet: the driver's default),
    // not in the mode of the session it had.
    h.command("conn other-my");
    h.db(DbEvent::Connected);
    h.command("conn local-my");
    assert_eq!(h.app.tab().id, tab);
    h.keys(" cs");
    if h.overlay_kind() == Some(datarig_tui::app::overlay::OverlayKind::Confirm) {
        h.keys("y");
    }
    h.type_text("other-my");
    h.key(ratatui::crossterm::event::KeyCode::Enter);
    assert_eq!(h.app.tab().profile, Some(id(&h, "other-my")));
    assert_eq!(h.app.tab_language(tab), my(MySqlMode::default()));
    assert_eq!(h.app.tab().editor.language(), my(MySqlMode::default()));
    // A new tab of the first profile reads its text in the mode that profile's session said,
    // before a session of its own says anything.
    h.command("conn local-my");
    h.ctrl('t');
    let fresh = h.app.tab().id;
    assert_ne!(fresh, tab);
    assert_eq!(h.app.tab().profile, Some(id(&h, "local-my")));
    assert_eq!(h.app.tab_language(fresh), my(ansi));
    assert_eq!(h.app.tab().editor.language(), my(ansi));
}

/// MySQL profiles `db-remote` (on another machine, its password from `first_source`) and
/// `lab-local` (on this one, from the secrets file), launched with the explorer's cursor on the
/// first; nothing connects until a test says so.
fn launched(first_source: PasswordSource) -> Harness {
    use datarig_core::profile::ConnectionConfig;
    let profile = |name: &str, host: &str| ConnectionConfig {
        name: name.into(),
        driver: "mysql".into(),
        host: host.into(),
        port: 3306,
        user: "app".into(),
        ..ConnectionConfig::default()
    };
    let mut remote = profile("db-remote", "db.example.com");
    remote.set_source(first_source);
    let mut local = profile("lab-local", "127.0.0.1");
    local.set_source(PasswordSource::File);
    let cfg = Config { connections: vec![remote, local], ..Config::default() };
    let store = std::sync::Arc::new(datarig_core::secret::MemoryStore::new());
    Harness::launched(&cfg, Lang::En, store, datarig_tui::app::Startup::Normal).with_fake_driver()
}

/// Connecting directly to a MySQL server on another machine says once, in the status bar,
/// that the connection is not encrypted; a server on this machine just connects.
#[test]
fn a_direct_connection_to_another_machine_warns_in_the_status_bar() {
    let mut h = launched(PasswordSource::File);
    h.key(KeyCode::Enter);
    h.meta_db("db-remote", DbEvent::Connected);
    let status = h.status(200, 40);
    assert!(status.contains("Connected to db-remote without encryption"), "{status}");
    assert!(status.contains("use an SSH tunnel"), "{status}");
    h.command("conn lab-local");
    h.meta_db("lab-local", DbEvent::Connected);
    let status = h.status(200, 40);
    assert!(status.contains("Connected to lab-local") && !status.contains("without encryption"), "{status}");
}

/// The profile form of a MySQL profile that would connect directly to another machine says
/// the connection is not encrypted, next to the host and in the SSH section; not once a
/// tunnel is picked, not for a loopback host, and never for PostgreSQL.
#[test]
fn the_form_warns_of_an_unencrypted_mysql_connection() {
    let mut h = launched(PasswordSource::File);
    h.command("conn.edit");
    assert_eq!(h.form().host.text(), "db.example.com");
    let short = "not encrypted: use SSH";
    let long = "This MySQL connection is not encrypted";
    assert!(h.screen(120, 40).contains(short), "{}", h.screen(120, 40));
    h.ctrl('n');
    assert_eq!(h.form().section, Section::Ssh);
    assert!(h.screen(120, 40).contains(long), "{}", h.screen(120, 40));
    // Its own tunnel: encrypted to the bastion.
    h.key(KeyCode::Right);
    assert_eq!(h.form().ssh_choice(), SshChoice::Inline);
    assert!(!h.screen(120, 40).contains(long));
    h.key(KeyCode::Left);
    assert!(h.screen(120, 40).contains(long));
    // A server on this machine.
    h.ctrl('p');
    h.app.overlays.form_mut().unwrap().focus_field(Field::Host);
    h.ctrl('u');
    h.type_text("localhost");
    assert!(!h.screen(120, 40).contains(short));
    h.key(KeyCode::Esc);
    // PostgreSQL has its own TLS settings.
    h.command("conn.new");
    h.app.overlays.form_mut().unwrap().focus_field(Field::Host);
    h.ctrl('u');
    h.type_text("db.example.com");
    assert!(!h.form().is_mysql());
    assert!(!h.screen(120, 40).contains(short));
}

/// MySQL's server key file and key retrieval are in the Advanced section of a MySQL profile,
/// and saved with it.
#[test]
fn the_server_key_options_are_set_in_the_form() {
    let mut h = launched(PasswordSource::File);
    h.command("conn.edit");
    h.ctrl('n');
    h.ctrl('n');
    assert_eq!(h.form().section, Section::Advanced);
    assert_eq!(h.form().focus, Field::ServerKey, "{:?}", h.form().fields());
    let screen = h.screen(120, 40);
    assert!(screen.contains("Server key file") && screen.contains("Key retrieval"), "{screen}");
    assert!(screen.contains("not allowed"), "off by default: {screen}");
    h.type_text("/keys/prod.pem");
    h.key(KeyCode::Tab);
    assert_eq!(h.form().focus, Field::KeyRetrieval);
    h.key(KeyCode::Right);
    h.ctrl('s');
    let p = h.app.profiles.iter().find(|p| p.name == "db-remote").unwrap();
    assert_eq!((p.server_public_key_file.as_deref(), p.allow_public_key_retrieval), (Some("/keys/prod.pem"), true));
}

/// MySQL refuses a password typed in the prompt with ERROR 1045: the error shows (it may be
/// the account requiring TLS, which no password fixes, and says that connecting again asks
/// again) and the prompt does not open again. A stored password the server refuses still
/// opens the prompt once.
#[test]
fn a_typed_password_mysql_refuses_shows_the_error_instead_of_asking_again() {
    let denied = || DbEvent::ConnectFailed {
        error: DbError::AccessDenied(
            "ERROR 1045 (28000): Access denied for user 'app'@'10.0.0.9' (using password: YES)".into(),
        ),
        auth: true,
    };
    let mut h = launched(PasswordSource::File);
    let account = h.account("db-remote");
    h.app.secrets.stores().file.set(&account, "stored").unwrap();
    h.key(KeyCode::Enter);
    h.meta_db("db-remote", denied());
    assert!(h.prompt().is_some(), "the stored password may be wrong: asked once");
    h.type_text("typed");
    h.key(KeyCode::Enter);
    h.meta_db("db-remote", denied());
    assert!(h.prompt().is_none(), "not asked again");
    let error = h.node_error("db-remote").unwrap_or_default();
    assert!(error.contains("Access denied") && error.contains("REQUIRE SSL"), "{error}");
    assert!(error.contains("after a mistyped password, connecting again asks for it again"), "{error}");
    assert!(h.status(200, 40).contains("REQUIRE SSL"), "{}", h.status(200, 40));
    // A `prompt` profile: its first password is a typed one.
    let mut h = launched(PasswordSource::Prompt);
    h.key(KeyCode::Enter);
    h.type_text("typed");
    h.key(KeyCode::Enter);
    h.meta_db("db-remote", denied());
    assert!(h.prompt().is_none());
    // Connecting again asks again.
    h.command("conn db-remote");
    assert!(h.prompt().is_some());
}

/// The Execute the app just sent: its id.
fn sent_run(h: &mut Harness) -> u64 {
    let sent = h.sent();
    sent.iter()
        .rev()
        .find_map(|c| match c {
            datarig_core::driver::DbCommand::Execute { id, .. } => Some(*id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no run sent: {sent:?}"))
}

#[test]
fn a_held_result_is_read_on_at_once_on_mysql() {
    use datarig_core::driver::{ColumnMeta, DbCommand, ValueKind};
    let mut h = mysql();
    h.db(DbEvent::Connected);
    h.ctrl('e');
    let id = sent_run(&mut h);
    let cols = vec![ColumnMeta::new("id".into(), "int".into(), ValueKind::Integer, None)];
    let rows = |n: usize| (0..n).map(|i| vec![Some(i.to_string())]).collect::<Vec<_>>();
    // Held (no `Released` before it): the next page is asked for at once, without a key.
    h.tab_db(0, DbEvent::Page { id, columns: Some(cols), rows: rows(500), more: true, elapsed: Default::default() });
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::FetchMore { id: f } if *f == id)), "read on");
    h.tab_db(0, DbEvent::Page { id, columns: None, rows: rows(500), more: true, elapsed: Default::default() });
    assert!(h.sent().iter().any(|c| matches!(c, DbCommand::FetchMore { .. })), "and on");
    h.tab_db(0, DbEvent::Page { id, columns: None, rows: rows(20), more: false, elapsed: Default::default() });
    assert!(h.sent().is_empty(), "read to its end: nothing more is asked");
    assert!(h.app.tab().exec.running.is_none());
    assert!(matches!(&h.app.tab().results, datarig_tui::app::Results::Rows(rs) if rs.rows.len() == 1020 && !rs.more));
}

#[test]
fn what_the_server_said_of_a_statement_goes_to_messages() {
    use datarig_core::driver::{Outcome, StatementInfo};
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let tab = h.app.tab().id;
    let mut editor =
        datarig_tui::widgets::editor::Editor::new("INSERT INTO t VALUES (1);\nUPDATE t SET a = 2 WHERE id = 1;");
    editor.set_language(h.app.tab_language(tab));
    h.app.tab_mut().editor = editor;
    h.keys("ggvG$");
    h.sent();
    h.ctrl('e');
    if h.app.overlays.run_confirm().is_some() {
        h.keys("y");
    }
    let id = sent_run(&mut h);
    let info = StatementInfo { insert_id: Some(5), warnings: 2, message: None, more_results: 0 };
    h.tab_db(0, DbEvent::Started { id, index: 0 });
    h.tab_db(0, DbEvent::Info { id, index: 0, info });
    h.tab_db(0, DbEvent::Finished { id, index: 0, outcome: Outcome::Affected(1), elapsed: Default::default() });
    h.tab_db(0, DbEvent::Started { id, index: 1 });
    let info = StatementInfo { message: Some("Rows matched: 1  Changed: 1  Warnings: 0".into()), ..Default::default() };
    h.tab_db(0, DbEvent::Info { id, index: 1, info });
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(1), elapsed: Default::default() });
    let notes: Vec<String> = h.app.tab().exec.run.notes.iter().map(|n| n.render(&h.app.i18n).to_string()).collect();
    assert_eq!(
        notes,
        [
            "statement 1: last insert id: 5",
            "statement 1: 2 warnings: SHOW WARNINGS shows them",
            "statement 2: the server says: Rows matched: 1  Changed: 1  Warnings: 0",
        ]
    );
    // An answer of an earlier run says nothing here.
    h.tab_db(0, DbEvent::Info { id: id - 1, index: 0, info: StatementInfo { warnings: 1, ..Default::default() } });
    assert_eq!(h.app.tab().exec.run.notes.len(), 3);
}
