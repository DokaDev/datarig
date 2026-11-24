//! A MySQL profile in the app: what its sessions say about how their text is read reaches the
//! tab's editor and risk classifier.

use crate::common;

use common::*;
use datarig_core::config::Config;
use datarig_core::driver::{DbError, DbEvent};
use datarig_core::secret::PasswordSource;
use datarig_core::sql::dialect::{Dialect, DollarQuotes, Language, MySqlMode};
use datarig_i18n::Lang;
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
    let ansi = MySqlMode { ansi_quotes: true, dollar_quotes: DollarQuotes::Read, ..MySqlMode::default() };
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

/// The profile form of a profile that would connect directly to another machine with TLS off
/// says the connection is not encrypted, next to the host and in the SSH section; not with TLS
/// on (MySQL's default is PREFERRED), not once a tunnel is picked, not for a loopback host. The
/// same for PostgreSQL.
#[test]
fn the_form_warns_of_an_unencrypted_connection() {
    let mut h = launched(PasswordSource::File);
    h.command("conn.edit");
    assert_eq!(h.form().host.text(), "db.example.com");
    let short = "not encrypted: use TLS or SSH";
    let long = "This connection is not encrypted (SSL mode off)";
    // PREFERRED: it may be encrypted; the connection says what it got.
    assert_eq!(h.form().tls_mode(), datarig_core::tls::TlsMode::Prefer);
    assert!(!h.screen(120, 40).contains(short), "{}", h.screen(120, 40));
    let off = |h: &mut Harness| {
        let i = datarig_core::tls::TlsMode::Disable.index();
        h.app.overlays.form_mut().unwrap().pick(Field::SslMode, i);
    };
    off(&mut h);
    assert!(h.screen(120, 40).contains(short), "{}", h.screen(120, 40));
    h.ctrl('n');
    assert_eq!(h.form().section, Section::Ssh);
    assert!(h.screen(120, 40).contains(long), "{}", h.screen(120, 40));
    // Its own tunnel: encrypted to the bastion.
    h.key(KeyCode::Enter);
    h.type_text("only");
    h.key(KeyCode::Enter);
    assert_eq!(h.form().ssh_choice(), SshChoice::Inline);
    assert!(!h.screen(120, 40).contains(long));
    h.key(KeyCode::Enter);
    h.type_text("off");
    h.key(KeyCode::Enter);
    assert_eq!(h.form().ssh_choice(), SshChoice::Off);
    assert!(h.screen(120, 40).contains(long));
    // A server on this machine.
    h.ctrl('p');
    h.app.overlays.form_mut().unwrap().focus_field(Field::Host);
    h.ctrl('u');
    h.type_text("localhost");
    assert!(!h.screen(120, 40).contains(short));
    h.key(KeyCode::Esc);
    assert_eq!(h.overlay_kind(), Some(datarig_tui::app::overlay::OverlayKind::Confirm), "asked first");
    h.key(KeyCode::Char('y'));
    // PostgreSQL: a new profile prefers TLS; with it off, the same warning.
    h.command("conn.new");
    h.app.overlays.form_mut().unwrap().focus_field(Field::Host);
    h.ctrl('u');
    h.type_text("db.example.com");
    assert!(!h.form().is_mysql());
    assert!(!h.screen(120, 40).contains(short));
    off(&mut h);
    assert!(h.screen(120, 40).contains(short), "{}", h.screen(120, 40));
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
    // After the SSL mode and its files.
    assert_eq!(h.form().focus, Field::SslMode, "{:?}", h.form().fields());
    for _ in 0..4 {
        h.key(KeyCode::Tab);
    }
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

#[test]
fn a_profile_starts_its_tabs_in_the_servers_mode_not_in_one_tabs_set() {
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let tab = h.app.tab().id;
    // The metadata session says the mode a new session starts in: the tab's first run is
    // checked in it, before a query session of its own exists.
    let server = MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() };
    h.meta_db("local-my", DbEvent::Language(my(server)));
    assert_eq!(h.app.tab_language(tab), my(server));
    assert_eq!(h.app.tab().exec.prepared.language(), my(server));
    // The tab's session starts in it, then its `SET sql_mode` changes that session only.
    h.ctrl('e');
    h.tab_db(0, DbEvent::Language(my(server)));
    let set = MySqlMode { ansi_quotes: true, ..server };
    h.tab_db(0, DbEvent::Language(my(set)));
    assert_eq!(h.app.tab_language(tab), my(set));
    // Another tab of the profile starts as a new session does.
    h.ctrl('t');
    let fresh = h.app.tab().id;
    assert_ne!(fresh, tab);
    assert_eq!(h.app.tab_language(fresh), my(server));
    assert_eq!(h.app.tab().exec.prepared.language(), my(server));
    assert_eq!(h.app.tab_language(tab), my(set));
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
    let info = StatementInfo { insert_id: Some(5), warnings: 2, ..Default::default() };
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

/// An `EXPLAIN ANALYZE` of a write the driver did not send, as its tables keep changes past a
/// rollback, says which and what to run instead; one it let run names them in Messages.
#[test]
fn an_analyze_a_rollback_cannot_undo_says_which_tables() {
    use datarig_core::driver::{DbError, Outcome, StatementInfo};
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let tab = h.app.tab().id;
    let mut editor =
        datarig_tui::widgets::editor::Editor::new("EXPLAIN ANALYZE DELETE x FROM t x JOIN u ON u.id = x.id");
    editor.set_language(h.app.tab_language(tab));
    h.app.tab_mut().editor = editor;
    h.sent();
    h.ctrl('e');
    if h.app.overlays.run_confirm().is_some() {
        h.keys("y");
    }
    let id = sent_run(&mut h);
    let error = DbError::NotUndoable(vec!["shop.t (MyISAM)".into(), "shop.v (VIEW)".into()]);
    h.tab_db(0, DbEvent::Failed { id, error, cancelled: false });
    // The failure as the UI words it (the editor's hint shows its start, Messages all of it).
    let failed = |h: &Harness| match &h.app.tab().exec.run.statements[0].outcome {
        datarig_tui::app::runlog::StatementOutcome::Failed(text) => text.clone(),
        other => panic!("{other:?}"),
    };
    let text = failed(&h);
    assert!(text.contains("a rollback cannot undo what it would change in shop.t (MyISAM), shop.v (VIEW)"), "{text}");
    // Tables whose triggers the account cannot see: what to do.
    h.ctrl('e');
    if h.app.overlays.run_confirm().is_some() {
        h.keys("y");
    }
    let id = sent_run(&mut h);
    let error = DbError::NotUndoable(vec!["shop.t (trigger)".into(), "shop.u (triggers not visible)".into()]);
    h.tab_db(0, DbEvent::Failed { id, error, cancelled: false });
    let text = failed(&h);
    assert!(text.contains("shop.t (has triggers), shop.u (its triggers are not visible to this account)"), "{text}");
    assert!(text.contains("use an account granted the TRIGGER privilege on those tables directly"), "{text}");
    // Text whose tables cannot be read.
    h.ctrl('e');
    if h.app.overlays.run_confirm().is_some() {
        h.keys("y");
    }
    let id = sent_run(&mut h);
    h.tab_db(0, DbEvent::Failed { id, error: DbError::NotUndoable(Vec::new()), cancelled: false });
    assert!(failed(&h).contains("cannot be read from its text"), "{}", failed(&h));
    // Let run (the driver's other choice): Messages names the tables.
    h.ctrl('e');
    if h.app.overlays.run_confirm().is_some() {
        h.keys("y");
    }
    let id = sent_run(&mut h);
    let info = StatementInfo { not_undone: vec!["shop.t (MyISAM)".into()], ..Default::default() };
    h.tab_db(0, DbEvent::Info { id, index: 0, info });
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Command("EXPLAIN".into()), elapsed: Default::default() });
    let notes: Vec<String> = h.app.tab().exec.run.notes.iter().map(|n| n.render(&h.app.i18n).to_string()).collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("EXPLAIN ANALYZE ran this on shop.t (MyISAM): changes there were not rolled back")),
        "{notes:?}"
    );
}

/// MySQL's databases are its schemas: the explorer lists them right under the profile (no
/// database level, the server's databases are not asked for), `:use <name>` moves the tab to
/// one, and the tab's line names it once.
#[test]
fn databases_are_the_schemas_under_the_profile() {
    use datarig_core::driver::{DbCommand, SessionContext};
    let mut h = mysql();
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["datarig".into(), "sales".into(), "shop".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    let rows = h.rows();
    let at = rows.iter().position(|r| r == "local-my").expect("the profile");
    assert_eq!(&rows[at + 1..at + 4], ["  datarig", "  sales", "  shop"], "{rows:?}");
    assert!(!rows.iter().any(|r| r.trim_start().starts_with("db:")), "no database level: {rows:?}");
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::LoadDatabases)));
    // `:use` names the schema (a database), and the tab works there.
    h.command("use sales");
    assert_eq!(h.app.tab().context, SessionContext { database: None, schema: Some("sales".into()) });
    let screen = h.screen(120, 30);
    assert!(screen.contains("local-my / sales "), "{screen}");
    assert!(!screen.contains("sales / sales"), "{screen}");
    assert_eq!(h.app.tab_path(h.app.tab()), ["sales"]);
    // Two levels are not a MySQL name.
    h.command("use a.b");
    assert!(h.screen(120, 30).contains(":use"), "a usage error");
}

#[test]
fn the_picker_lists_the_databases_with_nothing_below_them() {
    use datarig_tui::app::quick::QuickRow;
    let mut h = mysql();
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["datarig".into(), "sales".into(), "shop".into()])));
    let p = h.app.profiles[0].id;
    let rows = |h: &Harness| h.app.overlays.quick().map(|q| q.items.clone()).unwrap_or_default();
    let selected = |h: &Harness| h.app.overlays.quick().and_then(|q| q.items.get(q.selected).cloned());
    let want = |h: &Harness| {
        [QuickRow::Profile(p)]
            .into_iter()
            .chain(["datarig", "sales", "shop"].map(|d| QuickRow::Database(p, d.into())))
            .collect::<Vec<_>>()
            == rows(h)
    };
    let sessions = |h: &Harness| h.driver.sessions.lock().unwrap().len();
    h.keys(" cd");
    h.meta_db("local-my", DbEvent::Databases(Ok(vec!["datarig".into(), "sales".into(), "shop".into()])));
    assert!(want(&h), "{:?}", rows(&h));
    assert_eq!(selected(&h), Some(QuickRow::Database(p, "datarig".into())));
    h.key(ratatui::crossterm::event::KeyCode::Esc);
    // In another database: still one level, and no session of its own is opened for it.
    h.command("use sales");
    let before = sessions(&h);
    h.keys(" cd");
    assert!(want(&h), "{:?}", rows(&h));
    assert_eq!(selected(&h), Some(QuickRow::Database(p, "sales".into())));
    assert_eq!(sessions(&h), before);
}

#[test]
fn a_lost_sessions_mode_goes_with_it() {
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let server = MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() };
    h.meta_db("local-my", DbEvent::Language(my(server)));
    h.ctrl('e');
    h.tab_db(0, DbEvent::Language(my(server)));
    // The tab's session leaves the server's mode, then ends: the next run is checked as a new
    // session starts.
    h.tab_db(0, DbEvent::Language(my(MySqlMode::default())));
    assert_eq!(h.app.tab().exec.prepared.language(), my(MySqlMode::default()));
    h.tab_db(0, DbEvent::Lost { error: datarig_core::driver::DbError::Closed });
    let tab = h.app.tab().id;
    assert_eq!(h.app.tab_language(tab), my(server));
    assert_eq!(h.app.tab().exec.prepared.language(), my(server));
}

/// `shop.Items` as the MySQL driver reports its keys: `id` the primary key (`AUTO_INCREMENT`),
/// a blob, a bit, a text, a JSON, a `TIMESTAMP` and a `FLOAT` column; names compared as MySQL
/// compares them.
fn items_keys() -> datarig_core::driver::keys::KeyCatalog {
    use datarig_core::driver::keys::{Generated, KeyCatalog, KeyKind, NameRule};
    let mut k = KeyCatalog::default();
    k.set_names(NameRule::MySql { tables_ignore_case: false });
    k.set_server_zone("UTC");
    let names = ["id", "bin", "bt", "txt", "js", "at", "ratio"];
    k.add_table(1, "shop", "Items", names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())));
    k.mark(1, &[1], KeyKind::Primary);
    k.set_generated(1, 1, Generated::AutoIncrement);
    k
}

/// What a MySQL 8.4 session says it is (a real session says it before it connects).
fn mysql_8_4() -> Vec<DbEvent> {
    server_said(false, Some((8, 4, 6)))
}

/// The explorer's statement of `shop.<table>` run, and its one row back (`id`, `txt` as given),
/// the grid focused; `columns` of the table's columns (the first `n`). The origins name the
/// table as the server writes it in the result (`txt` in another case). The session says it is
/// MySQL 8.4 before the rows come.
fn items_result(h: &mut Harness, table: &str, id: &str, txt: &str, n: usize) {
    items_result_said(h, mysql_8_4(), table, id, txt, n);
}

/// [`items_result`], the session saying `said` before the rows come.
fn items_result_said(h: &mut Harness, said: Vec<DbEvent>, table: &str, id: &str, txt: &str, n: usize) {
    use datarig_core::driver::{ColumnMeta, ColumnOrigin, ValueKind};
    h.app.run(vec![format!("SELECT * FROM `shop`.`{table}`")]);
    let run = sent_run(h);
    for ev in said {
        h.tab_db(0, ev);
    }
    let col = |name: &str, ty: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: table.into(), column: name.into() };
        ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
    };
    let columns = [
        col("id", "int", ValueKind::Integer),
        col("bin", "varbinary", ValueKind::Bytes),
        col("bt", "bit", ValueKind::Bit),
        col("TXT", "varchar", ValueKind::Text),
        col("js", "json", ValueKind::Json),
        col("at", "timestamp", ValueKind::TimestampTz),
        col("ratio", "float", ValueKind::Float),
    ];
    let row = [
        Some(id.to_string()),
        Some("0x00FF5C".to_string()),
        Some("b'101'".to_string()),
        Some(txt.to_string()),
        Some(r#"{"a": "b"}"#.to_string()),
        Some("2024-10-27 00:30:00".to_string()),
        Some("1.23457".to_string()),
    ];
    let columns = columns[..n].to_vec();
    let rows = vec![row[..n].to_vec()];
    h.tab_db(0, DbEvent::Page { id: run, columns: Some(columns), rows, more: false, elapsed: Default::default() });
    h.app.focus = datarig_tui::app::Focus::Results;
}

fn session(zone: &str, charset: &str) -> DbEvent {
    DbEvent::SessionValues(datarig_core::driver::SessionValues { time_zone: zone.into(), charset: charset.into() })
}

/// A MySQL result copied as SQL: backtick names, MySQL's literals (bytes `X'…'`, bits `b'…'`,
/// a string with a backslash as its UTF-8 bytes, so that it reads the same in every sql mode, a
/// `TIMESTAMP` with the offset of the session's fixed zone), the table's own column names; an
/// UPDATE keyed by the primary key. Refused, it says why in MySQL's terms: the keys still being
/// read, a table the key cache does not have, keys that could not be read, and for a column
/// whose values would not be written whole: a `FLOAT`, text read in another character set than
/// UTF-8, a `TIMESTAMP` in a zone that is not a fixed offset, an `AUTO_INCREMENT` 0.
#[test]
fn mysql_rows_copy_as_mysql_sql_or_say_why_not() {
    let mut h = mysql();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Connected);
    h.tab_db(0, session("SYSTEM", "utf8mb4"));
    // The keys are not read yet.
    items_result(&mut h, "Items", "7", "it's \\ here", 6);
    h.command("copy insert");
    let status = h.status(200, 45);
    assert!(status.contains("Not copied as INSERT: the tables' keys are still being read"), "{status}");
    assert!(!status.contains("not known yet, or could not be read"), "{status}");
    h.db(DbEvent::Keys(Ok(items_keys())));
    h.command("copy insert");
    assert_eq!(
        clip.last().as_deref(),
        Some(
            "INSERT INTO `shop`.`Items` (`id`, `bin`, `bt`, `txt`, `js`, `at`) \
             VALUES (7, X'00FF5C', b'101', _utf8mb4 X'69742773205C2068657265', '{\"a\": \"b\"}', \
             '2024-10-27 00:30:00+00:00');"
        ),
        "SYSTEM is the server's UTC: a fixed offset"
    );
    h.command("copy update");
    assert_eq!(
        clip.last().as_deref(),
        Some(
            "UPDATE `shop`.`Items` SET `bin` = X'00FF5C', `bt` = b'101', `txt` = _utf8mb4 X'69742773205C2068657265', \
             `js` = '{\"a\": \"b\"}', `at` = '2024-10-27 00:30:00+00:00' WHERE `id` = 7;"
        )
    );
    // A text that only looks like bytes is text.
    items_result(&mut h, "Items", "8", "0xE9", 4);
    h.command("copy insert");
    assert!(clip.last().is_some_and(|c| c.ends_with("VALUES (8, X'00FF5C', b'101', '0xE9');")), "{:?}", clip.last());
    let copies = || clip.texts.lock().unwrap().len();
    let refused = |h: &mut Harness, want: &str| {
        let before = copies();
        h.command("copy insert");
        let status = h.status(240, 45);
        assert!(status.contains(want), "{want:?}: {status}");
        assert_eq!(copies(), before, "nothing copied: {want}");
    };
    // An AUTO_INCREMENT 0 would be numbered anew.
    items_result(&mut h, "Items", "0", "x", 4);
    refused(&mut h, "Not copied as SQL: column “id” is AUTO_INCREMENT and a row has 0");
    // A FLOAT is sent rounded.
    items_result(&mut h, "Items", "9", "x", 7);
    refused(&mut h, "Not copied as SQL: column “ratio” is a FLOAT, which MySQL sends rounded");
    // A TIMESTAMP in a zone with daylight saving.
    h.tab_db(0, session("Europe/Berlin", "utf8mb4"));
    items_result(&mut h, "Items", "9", "x", 6);
    refused(&mut h, "column “at” is a TIMESTAMP shown in the time zone Europe/Berlin, which is not a fixed offset");
    // Text read in latin1 (SET NAMES latin1) is not the text.
    h.tab_db(0, session("+00:00", "latin1"));
    items_result(&mut h, "Items", "9", "x", 4);
    refused(&mut h, "column “TXT” is text the session reads in latin1, not utf8mb4 (SET NAMES utf8mb4 to copy it)");
    h.tab_db(0, session("+00:00", "utf8mb4"));
    // A table created after the keys were read.
    items_result(&mut h, "Later", "1", "x", 4);
    refused(&mut h, "Not copied as INSERT: its table or one of its columns is not in the key cache");
    // The keys could not be read: the server's reason.
    h.db(DbEvent::Keys(Err(DbError::Server("ERROR 1142 (42000): denied".into()))));
    items_result(&mut h, "Items", "1", "x", 4);
    refused(&mut h, "the tables' columns could not be read (ERROR 1142 (42000): denied)");
}

/// A MariaDB session's result is refused in the same words as a MySQL one's: its tables are
/// named in the result and looked up in the keys read at connect, so a table is missing while
/// they are still being read, or when it is not in the key cache.
#[test]
fn a_mariadb_session_words_the_key_cache_as_mysql() {
    let mut h = mysql();
    h.db(DbEvent::Connected);
    h.tab_db(0, session("+00:00", "utf8mb4"));
    let mariadb = || server_said(true, Some((11, 4, 13)));
    items_result_said(&mut h, mariadb(), "Items", "7", "x", 4);
    assert_eq!(h.app.tab().exec.prepared.language(), Language::Sql(Dialect::MariaDb(MySqlMode::default())));
    h.command("copy insert");
    let status = h.status(200, 45);
    assert!(status.contains("Not copied as INSERT: the tables' keys are still being read"), "{status}");
    assert!(!status.contains("not known yet, or could not be read"), "{status}");
    h.db(DbEvent::Keys(Ok(items_keys())));
    items_result_said(&mut h, mariadb(), "Later", "1", "x", 4);
    h.command("copy insert");
    let status = h.status(240, 45);
    assert!(
        status.contains("Not copied as INSERT: its table or one of its columns is not in the key cache"),
        "{status}"
    );
}

/// `SELECT <cols> FROM shop.Items` run (columns of `items_result`'s types, by name), the session
/// saying `said` before its rows `rows` come; the grid focused.
fn items_rows(h: &mut Harness, said: Vec<DbEvent>, cols: &[&str], rows: &[&[Option<&str>]]) {
    use datarig_core::driver::{ColumnMeta, ColumnOrigin, ValueKind};
    h.app.run(vec![format!("SELECT {} FROM `shop`.`Items`", cols.join(", "))]);
    let run = sent_run(h);
    for ev in said {
        h.tab_db(0, ev);
    }
    let columns = cols
        .iter()
        .map(|&name| {
            let (ty, kind) = match name {
                "id" => ("int", ValueKind::Integer),
                "at" => ("timestamp", ValueKind::TimestampTz),
                other => panic!("no column {other}"),
            };
            let origin = ColumnOrigin::Named { schema: "shop".into(), table: "Items".into(), column: name.into() };
            ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
        })
        .collect();
    let rows = rows.iter().map(|r| r.iter().map(|v| v.map(str::to_string)).collect()).collect();
    h.tab_db(0, DbEvent::Page { id: run, columns: Some(columns), rows, more: false, elapsed: Default::default() });
    h.app.focus = datarig_tui::app::Focus::Results;
}

/// What the app flashed last, in words.
fn flashed(h: &Harness) -> String {
    h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default()
}

/// Copy the result as an SQL `IN` list.
fn copy_in(h: &mut Harness) {
    use datarig_tui::app::action::Action;
    use datarig_tui::app::copy::{CopyFormat, CopyScope};
    h.app.dispatch(Action::Copy(CopyScope::Fetched, CopyFormat::SqlIn));
}

const ENGINE_WORDS: &str = "Not copied as SQL: column “at” is a TIMESTAMP. This server does not read a time zone \
                            offset in a TIMESTAMP value (MariaDB, MySQL before 8.0.19), so it cannot be copied or \
                            exported as SQL. Export as CSV or JSON still works.";
const NOT_YET_WORDS: &str = "Not copied as SQL: column “at” is a TIMESTAMP. The server version was not known yet \
                             when these rows were read, so a time with a time zone offset cannot be written \
                             safely as SQL. Run the query again, or export as CSV or JSON.";
const UNKNOWN_WORDS: &str = "Not copied as SQL: column “at” is a TIMESTAMP. The server version is not known, so a \
                             time with a time zone offset cannot be written safely as SQL. Export as CSV or JSON \
                             still works, and so does copying from a session whose server version is known.";

/// A `TIMESTAMP` is copied as SQL with its offset only for a server that reads one (MySQL from
/// 8.0.19): for MariaDB, MySQL 8.0.18 and a MySQL whose version is not known, an `INSERT`, an
/// `UPDATE` and an `IN` list of it are refused with the reason (the `IN` list before "no value to
/// list"); NULLs and zero dates are still copied, an `IN` list of NULLs only still has no value.
/// MySQL 8.4 writes it with its offset, as before.
#[test]
fn a_timestamp_is_not_copied_as_sql_for_a_server_that_reads_no_offset() {
    let servers = [
        ("MariaDB 11.4", server_said(true, Some((11, 4, 13))), ENGINE_WORDS),
        ("MySQL 8.0.18", server_said(false, Some((8, 0, 18))), ENGINE_WORDS),
        ("MySQL of a version not known", server_said(false, None), UNKNOWN_WORDS),
    ];
    for (server, said, words) in servers {
        let mut h = mysql();
        let clip = FakeClipboard::attach(&mut h, false, &[]);
        h.db(DbEvent::Connected);
        h.db(DbEvent::Keys(Ok(items_keys())));
        h.tab_db(0, session("+00:00", "utf8mb4"));
        let copies = || clip.texts.lock().unwrap().len();
        items_rows(&mut h, said, &["id", "at"], &[&[Some("7"), Some("2024-10-27 00:30:00")]]);
        for copy in ["copy insert", "copy update"] {
            h.app.transient = None;
            h.command(copy);
            assert_eq!(flashed(&h), words, "{server}: {copy}");
            assert_eq!(copies(), 0, "{server}: {copy}: nothing copied");
        }
        // NULLs and zero dates are no instant: copied as before.
        let rows: &[&[Option<&str>]] = &[&[Some("8"), None], &[Some("9"), Some("0000-00-00 00:00:00")]];
        items_rows(&mut h, Vec::new(), &["id", "at"], rows);
        h.command("copy insert");
        assert_eq!(
            clip.last().as_deref(),
            Some(
                "INSERT INTO `shop`.`Items` (`id`, `at`) VALUES (8, NULL);\n\
                 INSERT INTO `shop`.`Items` (`id`, `at`) VALUES (9, '0000-00-00 00:00:00');"
            ),
            "{server}"
        );
        h.command("copy update");
        assert_eq!(
            clip.last().as_deref(),
            Some(
                "UPDATE `shop`.`Items` SET `at` = NULL WHERE `id` = 8;\n\
                 UPDATE `shop`.`Items` SET `at` = '0000-00-00 00:00:00' WHERE `id` = 9;"
            ),
            "{server}"
        );
        // An IN list of the value: its reason, not "no value to list".
        items_rows(&mut h, Vec::new(), &["at"], &[&[Some("2024-10-27 00:30:00")], &[None]]);
        let before = copies();
        h.app.transient = None;
        copy_in(&mut h);
        assert_eq!(flashed(&h), words, "{server}: IN");
        assert_eq!(copies(), before, "{server}: IN");
        // A zero date is listed; NULLs only have no value to list, as before.
        items_rows(&mut h, Vec::new(), &["at"], &[&[Some("0000-00-00 00:00:00")], &[None]]);
        copy_in(&mut h);
        assert_eq!(clip.last().as_deref(), Some("('0000-00-00 00:00:00')"), "{server}");
        items_rows(&mut h, Vec::new(), &["at"], &[&[None]]);
        h.app.transient = None;
        copy_in(&mut h);
        assert_eq!(flashed(&h), "Not copied as an IN list: there is no value to list (NULL is left out)", "{server}");
    }
    // MySQL 8.4 reads the offset: written with it.
    let mut h = mysql();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(items_keys())));
    h.tab_db(0, session("+00:00", "utf8mb4"));
    items_rows(&mut h, mysql_8_4(), &["id", "at"], &[&[Some("7"), Some("2024-10-27 00:30:00")]]);
    h.command("copy insert");
    assert_eq!(
        clip.last().as_deref(),
        Some("INSERT INTO `shop`.`Items` (`id`, `at`) VALUES (7, '2024-10-27 00:30:00+00:00');")
    );
    items_rows(&mut h, Vec::new(), &["at"], &[&[Some("2024-10-27 00:30:00")]]);
    copy_in(&mut h);
    assert_eq!(clip.last().as_deref(), Some("('2024-10-27 00:30:00+00:00')"));
}

/// Rows are judged as SQL by the engine that read them: rows read on MariaDB, in a tab moved
/// since to a PostgreSQL profile, are not copied as SQL by PostgreSQL's rules.
#[test]
fn an_old_result_keeps_the_rules_of_the_server_that_read_it() {
    let driver = FakeDriver::default();
    driver.mysql.store(true, Ordering::SeqCst);
    let mysql = driver.mysql.clone();
    let mut my = datarig_core::profile::ConnectionConfig::test_db();
    (my.name, my.driver) = ("local-my".into(), "mysql".into());
    let mut pg = datarig_core::profile::ConnectionConfig::test_db();
    pg.name = "local-pg".into();
    let cfg = Config { connections: vec![my, pg], ..Config::default() };
    let mut h = Harness::with_driver(&cfg, Lang::En, driver);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(items_keys())));
    h.tab_db(0, session("+00:00", "utf8mb4"));
    items_rows(&mut h, server_said(true, Some((11, 4, 13))), &["id"], &[&[Some("7")]]);
    let tab = h.app.tab().id;
    h.command("copy insert");
    assert_eq!(clip.last().as_deref(), Some("INSERT INTO `shop`.`Items` (`id`) VALUES (7);"), "{}", flashed(&h));
    // The tab moves to the PostgreSQL profile (which the fake driver serves as PostgreSQL from
    // now on); its rows stay.
    mysql.store(false, Ordering::SeqCst);
    h.command("conn local-pg");
    h.db(DbEvent::Connected);
    h.command("conn local-my");
    assert_eq!(h.app.tab().id, tab);
    h.keys(" cs");
    if h.overlay_kind() == Some(datarig_tui::app::overlay::OverlayKind::Confirm) {
        h.keys("y");
    }
    h.type_text("local-pg");
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().id, tab);
    assert_eq!(h.app.tab_language(tab).dialect(), Dialect::Postgres);
    h.app.focus = datarig_tui::app::Focus::Results;
    let copies = clip.texts.lock().unwrap().len();
    for copy in ["copy insert", "copy update", "copy insert shop.Items"] {
        h.app.transient = None;
        h.command(copy);
        assert_eq!(
            flashed(&h),
            "Not copied as SQL. These rows were read on another database engine than the one the tab uses now; \
             run the query again.",
            "{copy}"
        );
    }
    copy_in(&mut h);
    assert!(flashed(&h).contains("read on another database engine"), "{}", flashed(&h));
    assert_eq!(clip.texts.lock().unwrap().len(), copies, "nothing copied as SQL");
    // As text it is still copied.
    h.command("copy csv all");
    assert_eq!(clip.last().as_deref(), Some("id\n7"));
}

/// A result read on MySQL 8.4 keeps what its server reads: copied with the offset right after
/// its session is lost (no session now), and again once a new session of the tab connected.
#[test]
fn a_result_read_on_mysql_8_4_still_copies_as_sql_after_the_session_is_lost() {
    let mut h = mysql();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(items_keys())));
    h.tab_db(0, session("+00:00", "utf8mb4"));
    items_rows(&mut h, mysql_8_4(), &["id", "at"], &[&[Some("7"), Some("2024-10-27 00:30:00")]]);
    let want = "INSERT INTO `shop`.`Items` (`id`, `at`) VALUES (7, '2024-10-27 00:30:00+00:00');";
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    h.command("copy insert");
    assert_eq!(clip.last().as_deref(), Some(want), "{}", flashed(&h));
    let copies = clip.texts.lock().unwrap().len();
    reconnect_keeping_the_result(&mut h, 0, "SELECT * FROM `shop`.`Gone`", mysql_8_4());
    let facts = h.app.tab().exec.current_facts().cloned().expect("facts");
    assert_eq!(facts.version, datarig_core::engine::Fact::Known(datarig_core::engine::Version::new(8, 4, 6)));
    h.app.focus = datarig_tui::app::Focus::Results;
    h.command("copy insert");
    assert_eq!(clip.last().as_deref(), Some(want), "{}", flashed(&h));
    assert_eq!(clip.texts.lock().unwrap().len(), copies + 1);
}

/// Whether a result's `TIMESTAMP`s are copied with their offset is what its session had said of
/// its server when its first page came: nothing said (never, or only after that page), refused
/// as a version not known yet; said before, written.
#[test]
fn a_result_read_before_the_server_said_its_facts_refuses_its_timestamps() {
    let start = || {
        let mut h = mysql();
        let clip = FakeClipboard::attach(&mut h, false, &[]);
        h.db(DbEvent::Connected);
        h.db(DbEvent::Keys(Ok(items_keys())));
        h.tab_db(0, session("+00:00", "utf8mb4"));
        (h, clip)
    };
    let row: &[&[Option<&str>]] = &[&[Some("7"), Some("2024-10-27 00:30:00")]];
    // Never said.
    let (mut h, clip) = start();
    items_rows(&mut h, Vec::new(), &["id", "at"], row);
    h.command("copy insert");
    assert_eq!(flashed(&h), NOT_YET_WORDS);
    assert!(clip.last().is_none());
    // Said after the first page: still refused, the result keeps what was known then.
    for ev in mysql_8_4() {
        h.tab_db(0, ev);
    }
    h.app.transient = None;
    h.command("copy insert");
    assert_eq!(flashed(&h), NOT_YET_WORDS);
    assert!(clip.last().is_none());
    // Said before: written. The session is open: the facts go to it before the run.
    let (mut h, clip) = start();
    items_rows(&mut h, Vec::new(), &["id"], &[&[Some("1")]]);
    for ev in mysql_8_4() {
        h.tab_db(0, ev);
    }
    items_rows(&mut h, Vec::new(), &["id", "at"], row);
    h.command("copy insert");
    assert_eq!(
        clip.last().as_deref(),
        Some("INSERT INTO `shop`.`Items` (`id`, `at`) VALUES (7, '2024-10-27 00:30:00+00:00');"),
        "{}",
        flashed(&h)
    );
}

/// A result is copied by what its values were read in, not by what the session reads in now:
/// a `TIMESTAMP` read under Europe/Berlin stays refused after `SET time_zone = '+00:00'` (never
/// written with `+00:00` after Berlin's text), and a text read under latin1 stays refused after
/// `SET NAMES utf8mb4` or after the session is lost; a text read before the session said its
/// values is refused; a result whose pages came under different values is refused too.
#[test]
fn a_result_is_copied_by_the_values_it_was_read_in() {
    use datarig_core::driver::{ColumnMeta, ColumnOrigin, ValueKind};
    let mut h = mysql();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(items_keys())));
    let col = |name: &str, ty: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: "Items".into(), column: name.into() };
        ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
    };
    let cols = || vec![col("id", "int", ValueKind::Integer), col("at", "timestamp", ValueKind::TimestampTz)];
    let page = |h: &mut Harness, rows: usize, more: bool, first: bool| {
        let run = if first { sent_run(h) } else { h.app.tab().exec.query_id };
        if first {
            for ev in mysql_8_4() {
                h.tab_db(0, ev);
            }
        }
        let rows = (0..rows).map(|i| vec![Some((i + 1).to_string()), Some("2024-10-27 02:30:00".into())]).collect();
        let columns = first.then(cols);
        h.tab_db(0, DbEvent::Page { id: run, columns, rows, more, elapsed: Default::default() });
        h.app.focus = datarig_tui::app::Focus::Results;
    };
    // Read under Europe/Berlin, then the session's zone set to a fixed offset.
    h.tab_db(0, session("Europe/Berlin", "utf8mb4"));
    h.app.run(vec!["SELECT id, at FROM `shop`.`Items`".into()]);
    page(&mut h, 1, false, true);
    h.tab_db(0, session("+00:00", "utf8mb4"));
    h.command("copy insert");
    let status = h.status(240, 45);
    assert!(status.contains("column “at” is a TIMESTAMP shown in the time zone Europe/Berlin"), "{status}");
    assert!(clip.last().is_none(), "never written with +00:00: {:?}", clip.last());
    // Read under latin1, then SET NAMES utf8mb4.
    h.tab_db(0, session("+00:00", "latin1"));
    items_result(&mut h, "Items", "7", "x", 4);
    h.tab_db(0, session("+00:00", "utf8mb4"));
    h.command("copy insert");
    let status = h.status(240, 45);
    assert!(status.contains("column “TXT” is text the session reads in latin1"), "{status}");
    assert!(clip.last().is_none(), "{:?}", clip.last());
    // Read under latin1, then the session is lost: still refused.
    h.tab_db(0, session("+00:00", "latin1"));
    items_result(&mut h, "Items", "7", "x", 4);
    h.tab_db(0, DbEvent::Lost { error: DbError::Closed });
    h.command("copy insert");
    let status = h.status(240, 45);
    assert!(status.contains("column “TXT” is text the session reads in latin1"), "{status}");
    assert!(clip.last().is_none(), "{:?}", clip.last());
    // Read before the session said its values: not known, refused.
    h.tab_db(0, DbEvent::Connected);
    items_result(&mut h, "Items", "7", "x", 4);
    h.command("copy insert");
    let status = h.status(240, 45);
    assert!(
        status.contains("column “TXT” was read while the session's time zone and character set were not known"),
        "{status}"
    );
    assert!(clip.last().is_none(), "{:?}", clip.last());
    h.tab_db(0, session("+00:00", "utf8mb4"));
    // Read under +00:00, copied as such.
    h.app.run(vec!["SELECT id, at FROM `shop`.`Items`".into()]);
    page(&mut h, 2, true, true);
    h.command("copy insert");
    assert!(clip.last().is_some_and(|c| c.contains("'2024-10-27 02:30:00+00:00'")), "{:?}", clip.last());
    // Its next page comes after the zone changed: its rows are not all in one zone.
    h.tab_db(0, session("Europe/Berlin", "utf8mb4"));
    page(&mut h, 1, false, false);
    let copies = clip.texts.lock().unwrap().len();
    h.command("copy insert");
    let status = h.status(240, 45);
    assert!(status.contains("column “at” was read across a change of the session's time zone"), "{status}");
    assert_eq!(clip.texts.lock().unwrap().len(), copies);
}

/// A procedure, a prepared statement or a stored function may change the session's time zone or
/// character set while its result is read, and the server does not say: the session says the
/// result is read in values not known (`ValuesNotKnown`, before its first page), and it is not
/// copied with the zone the session had before; what the session says after it binds the next
/// result. A result whose session reports other values before it was read to its end is not
/// copied either.
#[test]
fn a_result_of_server_code_is_not_copied_with_the_old_values() {
    use datarig_core::driver::{ColumnMeta, ColumnOrigin, ValueKind};
    let mut h = mysql();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Connected);
    h.db(DbEvent::Keys(Ok(items_keys())));
    h.tab_db(0, session("+00:00", "utf8mb4"));
    let col = |name: &str, ty: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: "Items".into(), column: name.into() };
        ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
    };
    let columns = || vec![col("id", "int", ValueKind::Integer), col("at", "timestamp", ValueKind::TimestampTz)];
    let rows = |n: usize| (1..=n).map(|i| vec![Some(i.to_string()), Some("2024-10-27 09:30:00".into())]).collect();
    // Run `sql`; `before` its first page (more after it with `more`).
    let result = |h: &mut Harness, sql: &str, before: Option<DbEvent>, more: bool| -> u64 {
        h.app.run(vec![sql.to_string()]);
        // A procedure asks first.
        if h.app.overlays.run_confirm().is_some() {
            h.keys("y");
        }
        let run = sent_run(h);
        for ev in mysql_8_4() {
            h.tab_db(0, ev);
        }
        if let Some(ev) = before {
            h.tab_db(0, ev);
        }
        h.tab_db(
            0,
            DbEvent::Page { id: run, columns: Some(columns()), rows: rows(1), more, elapsed: Default::default() },
        );
        // Code may have changed the tables too: the keys are read again.
        h.db(DbEvent::Keys(Ok(items_keys())));
        h.app.focus = datarig_tui::app::Focus::Results;
        run
    };
    let refused = |h: &mut Harness, what: &str| {
        let before = clip.texts.lock().unwrap().len();
        h.command("copy insert shop.Items");
        let status = h.status(240, 45);
        assert!(status.contains("column “at” was read"), "{what}: {status}");
        assert_eq!(clip.texts.lock().unwrap().len(), before, "{what}: nothing copied");
    };
    for sql in ["CALL shop.moves_the_zone()", "EXECUTE s", "/*! CALL shop.moves_the_zone() */"] {
        let run = h.app.tab().exec.query_id + 1;
        result(&mut h, sql, Some(DbEvent::ValuesNotKnown { id: run }), false);
        refused(&mut h, sql);
    }
    // The session says what the code left: the next result is bound to it.
    h.tab_db(0, session("+09:00", "utf8mb4"));
    result(&mut h, "SELECT id, at FROM `shop`.`Items`", None, false);
    h.command("copy insert");
    assert!(
        clip.last().is_some_and(|c| c.contains("'2024-10-27 09:30:00+09:00'")),
        "{:?} {}",
        clip.last(),
        h.status(240, 45)
    );
    // A held result: its first page, then the session's values change (a stored function a view
    // calls), then its last page: read in two sets of values.
    let run = result(&mut h, "SELECT id, at FROM `shop`.`Items`", None, true);
    h.tab_db(0, session("+05:00", "utf8mb4"));
    h.tab_db(0, DbEvent::Page { id: run, columns: None, rows: rows(1), more: false, elapsed: Default::default() });
    refused(&mut h, "values changed before the result ended");
}

/// A statement the server would not stop where reading on could last forever (the driver cuts
/// the connection): the run ends cancelled, the tab says why its connection was lost, in words
/// of its own for each cause, never that the statement stopped (it may run on and commit), and
/// what happens to an open transaction; the next run connects again.
#[test]
fn a_statement_the_server_would_not_stop_ends_the_tab_session_and_says_why() {
    use datarig_core::driver::CancelFailure;
    use datarig_core::driver::SessionRole::{Meta, Query};
    use datarig_tui::app::{Results, SessionState};
    let tail = "The statement may still run on the server, and outside a transaction block commit what it \
                changes; an open transaction is rolled back once the server ends the session";
    for (cause, why) in [
        (
            CancelFailure::Refused,
            "the server refused to stop the statement (it runs as another account, e.g. in a SQL SECURITY \
             DEFINER routine) and it kept sending rows: datarig closed the connection instead of reading them.",
        ),
        (
            CancelFailure::NotSent(Box::new(DbError::NoAnswer(std::time::Duration::from_secs(10)))),
            "the request to stop the statement could not reach the server (no answer within 10.0s): datarig \
             closed the connection.",
        ),
        (
            CancelFailure::KeptSending,
            "the server agreed to stop the statement and still kept sending rows: datarig closed the connection \
             instead of reading them.",
        ),
        (
            CancelFailure::NotAnswered(std::time::Duration::from_secs(10)),
            "the request to stop the statement got no answer from the server within 10.0s: datarig closed the \
             connection.",
        ),
    ] {
        let mut h = mysql();
        h.db(DbEvent::Connected);
        h.ctrl('e');
        let id = sent_run(&mut h);
        h.ctrl('c');
        assert!(h.session_cancelled(1), "the tab's session was asked to cancel");
        h.tab_db(0, DbEvent::Failed { id, error: DbError::Cancelled, cancelled: true });
        h.tab_db(0, DbEvent::Lost { error: DbError::CancelNotTaken(cause.clone()) });
        assert!(matches!(h.app.tab().results, Results::Cancelled));
        assert!(h.app.tab().exec.running.is_none());
        assert_eq!(h.app.tab().exec.state, SessionState::Lost);
        let status = h.status(600, 45);
        assert!(status.contains(&format!("This tab's connection was lost: {why}")), "{cause:?}: {status}");
        assert!(status.contains(tail), "{cause:?}: never says it stopped: {status}");
        assert!(!status.contains("cancel"), "{cause:?}: the words fit a stop the user did not ask for: {status}");
        h.ctrl('e');
        assert_eq!(h.roles(), [Meta, Query, Query], "the next run opens a new session");
    }
}

/// A statement that answered while a request to stop it got no answer: its outcome stays the
/// run's, and the tab says why its connection was lost without saying the statement may run on
/// (it ended) nor that it was cancelled.
#[test]
fn a_statement_that_answered_while_its_stop_went_unanswered_keeps_its_outcome() {
    use datarig_core::driver::{CancelFailure, Outcome};
    use datarig_tui::app::{Results, SessionState};
    let mut h = mysql();
    h.db(DbEvent::Connected);
    h.ctrl('e');
    let id = sent_run(&mut h);
    h.ctrl('c');
    h.tab_db(0, DbEvent::Done { id, outcome: Outcome::Affected(3), elapsed: Default::default() });
    let why = CancelFailure::StopPending(std::time::Duration::from_secs(10));
    h.tab_db(0, DbEvent::Lost { error: DbError::CancelNotTaken(why) });
    assert!(!matches!(h.app.tab().results, Results::Cancelled));
    assert_eq!(h.app.tab().exec.state, SessionState::Lost);
    let status = h.status(600, 45);
    assert!(
        status.contains(
            "This tab's connection was lost: a request to stop the statement got no answer from the server within \
             10.0s and could still stop whatever would run next: datarig closed the connection once the statement \
             had answered. An open transaction is rolled back once the server ends the session"
        ),
        "{status}"
    );
    assert!(!status.contains("may still run"), "{status}");
    assert!(!status.contains("cancel"), "{status}");
}

/// In MySQL a `"…"` string is a literal like `'…'`: its column's values are offered there
/// (an `ENUM`'s, as the driver reads them); under `ANSI_QUOTES` it is a name, and names are.
#[test]
fn values_complete_in_both_mysql_strings() {
    let mut h = mysql();
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["shop".into()])));
    h.db(DbEvent::Catalog(Ok(values_catalog())));
    h.keys("ggdGi");
    h.keys("SELECT * FROM shop.orders WHERE status = \"sh");
    h.settle();
    let labels = |h: &Harness| h.popup().map(|p| p.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>());
    assert_eq!(labels(&h), Some(vec!["shipped".to_string()]));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.tab().editor.text(), "SELECT * FROM shop.orders WHERE status = \"shipped\"");
    h.key(KeyCode::Esc);
    h.ctrl('e');
    let ansi = MySqlMode { ansi_quotes: true, ..MySqlMode::default() };
    h.tab_db(0, DbEvent::Language(my(ansi)));
    h.keys("ggdGi");
    h.keys("SELECT * FROM shop.orders WHERE status = \"st");
    h.settle();
    assert_eq!(labels(&h), Some(vec!["`status`".to_string()]), "a name, quoted as MySQL writes it");
}

/// A copy as SQL INSERT flashes only the columns it wrote: a generated column left out of it
/// does not flash; a copy of every column flashes them all.
#[test]
fn an_insert_copy_flashes_only_the_columns_it_wrote() {
    use datarig_core::driver::keys::Generated;
    let mut h = mysql();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Connected);
    h.tab_db(0, session("SYSTEM", "utf8mb4"));
    let mut keys = items_keys();
    keys.set_generated(1, 4, Generated::Expression);
    h.db(DbEvent::Keys(Ok(keys)));
    items_result(&mut h, "Items", "7", "x", 4);
    h.command("copy insert");
    let copied = clip.last().unwrap_or_default();
    assert!(copied.starts_with("INSERT INTO `shop`.`Items` (`id`, `bin`, `bt`) VALUES"), "{copied}");
    let flash = |h: &Harness| h.app.copy_flash.as_ref().map(|f| (f.rows.clone(), f.cols.clone()));
    assert_eq!(flash(&h), Some((0..1, vec![0, 1, 2])), "not the generated `txt`");
    // A range from the second column: the columns of the result it wrote.
    h.keys("lvll");
    h.keys(" ryi");
    let copied = clip.last().unwrap_or_default();
    assert!(copied.starts_with("INSERT INTO `shop`.`Items` (`bin`, `bt`) VALUES"), "{copied}");
    assert_eq!(flash(&h), Some((0..1, vec![1, 2])));
    h.command("copy csv all");
    assert_eq!(flash(&h), Some((0..1, vec![0, 1, 2, 3])));
}

/// A filter on a `TIMESTAMP` reads the session's time zone (`CONVERT_TZ(…, '+00:00',
/// @@session.time_zone)`): not held, its next page runs it again and it is counted, while the
/// session's zone is the one its rows were read in. Once the zone changed, neither runs, and
/// the reason is said; back in that zone, both run again.
#[test]
fn a_timestamp_filter_pages_again_only_in_the_zone_it_was_read_in() {
    use datarig_core::driver::{ColumnMeta, DbCommand, PagingMode, ValueKind};
    let sql = "SELECT * FROM `shop`.`orders` WHERE `placed_at` = \
               CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone) ORDER BY `id`";
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let lang = h.app.tab().editor.language();
    h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new(sql);
    h.app.tab_mut().editor.set_language(lang);
    h.ctrl('e');
    let id = sent_run(&mut h);
    h.tab_db(0, session("+00:00", "utf8mb4"));
    let cols = || Some(vec![ColumnMeta::new("id".into(), "int".into(), ValueKind::Integer, None)]);
    let rows = |from: usize| (from..from + 500).map(|i| vec![Some(i.to_string())]).collect::<Vec<_>>();
    h.tab_db(0, DbEvent::Released { id });
    h.tab_db(0, DbEvent::Page { id, columns: cols(), rows: rows(0), more: true, elapsed: Default::default() });
    let status = h.status(400, 45);
    assert!(!status.contains("First 500 rows only"), "{status}");
    assert!(h.app.can_run_again());
    h.app.focus = datarig_tui::app::Focus::Results;
    let resumes = |sent: &[DbCommand]| {
        sent.iter()
            .filter_map(|c| match c {
                DbCommand::Resume { id, sql, skip, paging: PagingMode::NoHold, .. } => Some((*id, sql.clone(), *skip)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let counts = |sent: &[DbCommand]| sent.iter().filter(|c| matches!(c, DbCommand::Count { .. })).count();
    h.keys("n");
    let sent = h.sent();
    let [(rid, resumed, 500)] = &resumes(&sent)[..] else { panic!("{sent:?}") };
    assert_eq!(resumed, sql);
    h.tab_db(0, DbEvent::Released { id: *rid });
    h.tab_db(0, DbEvent::Page { id: *rid, columns: cols(), rows: rows(500), more: true, elapsed: Default::default() });
    assert!(matches!(&h.app.tab().results, datarig_tui::app::Results::Rows(rs) if rs.rows.len() == 1000 && rs.more));
    // The session's zone changes (a `SET time_zone` the user ran): the rows were read in
    // another one.
    h.tab_db(0, session("+09:00", "utf8mb4"));
    h.keys("n");
    let sent = h.sent();
    assert!(resumes(&sent).is_empty(), "{sent:?}");
    let status = h.status(400, 45);
    assert!(
        status.contains("is not run again for you (it reads the session's time zone, which is not known to be the one its rows were read in)"),
        "{status}"
    );
    h.keys("#");
    let sent = h.sent();
    assert_eq!(counts(&sent), 0, "{sent:?}");
    let status = h.status(400, 45);
    assert!(status.contains("Rows are not counted for you: it reads the session's time zone"), "{status}");
    // Back in the zone the rows were read in.
    h.tab_db(0, session("+00:00", "utf8mb4"));
    h.keys("#");
    assert_eq!(counts(&h.sent()), 1);
}

/// A result read while the session's time zone is not known (the driver reports
/// [`SessionValues::UNKNOWN`](datarig_core::driver::SessionValues::UNKNOWN), as after a stored
/// routine that may have changed it): a filter on a `TIMESTAMP` is not paged again nor counted,
/// though the session still reports the same, since the zone its rows were read in was never
/// known.
#[test]
fn a_timestamp_filter_read_in_an_unknown_zone_is_not_paged_again() {
    use datarig_core::driver::{ColumnMeta, DbCommand, SessionValues, ValueKind};
    let sql = "SELECT * FROM `shop`.`orders` WHERE `placed_at` = \
               CONVERT_TZ('2026-11-10 09:30:00', '+00:00', @@session.time_zone) ORDER BY `id`";
    let mut h = mysql();
    h.db(DbEvent::Connected);
    let lang = h.app.tab().editor.language();
    h.app.tab_mut().editor = datarig_tui::widgets::editor::Editor::new(sql);
    h.app.tab_mut().editor.set_language(lang);
    h.ctrl('e');
    let id = sent_run(&mut h);
    h.tab_db(0, session(SessionValues::UNKNOWN, SessionValues::UNKNOWN));
    let cols = Some(vec![ColumnMeta::new("id".into(), "int".into(), ValueKind::Integer, None)]);
    let rows = (0..500).map(|i| vec![Some(i.to_string())]).collect::<Vec<_>>();
    h.tab_db(0, DbEvent::Released { id });
    h.tab_db(0, DbEvent::Page { id, columns: cols, rows, more: true, elapsed: Default::default() });
    h.app.focus = datarig_tui::app::Focus::Results;
    h.keys("n");
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::Resume { .. })), "{sent:?}");
    let status = h.status(400, 45);
    assert!(
        status.contains("is not run again for you (it reads the session's time zone, which is not known to be the one its rows were read in)"),
        "{status}"
    );
    h.keys("#");
    let sent = h.sent();
    assert!(!sent.iter().any(|c| matches!(c, DbCommand::Count { .. })), "{sent:?}");
    let status = h.status(400, 45);
    assert!(status.contains("Rows are not counted for you: it reads the session's time zone"), "{status}");
}
