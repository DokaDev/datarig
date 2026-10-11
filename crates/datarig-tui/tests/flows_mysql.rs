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

/// `shop.Items` as the MySQL driver reports its keys: `id` the primary key, a blob, a bit, a
/// text and a JSON column; names compared as MySQL compares them.
fn items_keys() -> datarig_core::driver::keys::KeyCatalog {
    use datarig_core::driver::keys::{KeyCatalog, KeyKind, NameRule};
    let mut k = KeyCatalog::default();
    k.set_names(NameRule::MySql { tables_ignore_case: false });
    let names = ["id", "bin", "bt", "txt", "js"];
    k.add_table(1, "shop", "Items", names.iter().enumerate().map(|(i, n)| (i as i16 + 1, n.to_string())));
    k.mark(1, &[1], KeyKind::Primary);
    k
}

/// The explorer's statement of `shop.Items` run, and its one row back, the grid focused. The
/// origins name the table as the server writes it in the result (`txt` in another case).
fn items_result(h: &mut Harness, table: &str, txt: &str) {
    use datarig_core::driver::{ColumnMeta, ColumnOrigin, ValueKind};
    h.app.run(vec![format!("SELECT * FROM `shop`.`{table}`")]);
    let id = sent_run(h);
    let col = |name: &str, ty: &str, kind: ValueKind| {
        let origin = ColumnOrigin::Named { schema: "shop".into(), table: table.into(), column: name.into() };
        ColumnMeta::new(name.into(), ty.into(), kind, Some(origin))
    };
    let columns = vec![
        col("id", "int", ValueKind::Integer),
        col("bin", "varbinary", ValueKind::Bytes),
        col("bt", "bit", ValueKind::Bit),
        col("TXT", "varchar", ValueKind::Text),
        col("js", "json", ValueKind::Json),
    ];
    let row = vec![
        Some("7".to_string()),
        Some("0x00FF5C".to_string()),
        Some("b'101'".to_string()),
        Some(txt.to_string()),
        Some(r#"{"a": "b\\c"}"#.to_string()),
    ];
    h.tab_db(
        0,
        DbEvent::Page { id, columns: Some(columns), rows: vec![row], more: false, elapsed: Default::default() },
    );
    h.app.focus = datarig_tui::app::Focus::Results;
}

/// A MySQL result copied as SQL: backtick names, MySQL's literals (bytes `X'…'`, bits `b'…'`,
/// strings escaped as the session's mode reads them), the table's own column names; an UPDATE
/// keyed by the primary key. Refused, it says why in MySQL's terms: the keys still being read,
/// a table the key cache does not have, keys that could not be read, a value not in hand whole.
#[test]
fn mysql_rows_copy_as_mysql_sql_or_say_why_not() {
    let mut h = mysql();
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Connected);
    // The keys are not read yet.
    items_result(&mut h, "Items", "it's \\ here");
    h.command("copy insert");
    let status = h.status(200, 45);
    assert!(status.contains("Not copied as INSERT: the tables' keys are still being read"), "{status}");
    assert!(!status.contains("not known yet, or could not be read"), "{status}");
    h.db(DbEvent::Keys(Ok(items_keys())));
    h.command("copy insert");
    assert_eq!(
        clip.last().as_deref(),
        Some(
            "INSERT INTO `shop`.`Items` (`id`, `bin`, `bt`, `txt`, `js`) \
             VALUES (7, X'00FF5C', b'101', 'it''s \\\\ here', '{\"a\": \"b\\\\\\\\c\"}');"
        )
    );
    h.command("copy update");
    assert_eq!(
        clip.last().as_deref(),
        Some(
            "UPDATE `shop`.`Items` SET `bin` = X'00FF5C', `bt` = b'101', `txt` = 'it''s \\\\ here', \
             `js` = '{\"a\": \"b\\\\\\\\c\"}' WHERE `id` = 7;"
        )
    );
    // Under NO_BACKSLASH_ESCAPES a backslash is written once.
    let plain = MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() };
    h.tab_db(0, DbEvent::Language(my(plain)));
    h.command("copy insert");
    assert!(clip.last().is_some_and(|c| c.contains("'it''s \\ here'")), "{:?}", clip.last());
    // A table created after the keys were read.
    items_result(&mut h, "Later", "x");
    h.command("copy insert");
    let status = h.status(220, 45);
    assert!(status.contains("Not copied as INSERT: its table is not in the key cache"), "{status}");
    // Text the session could only show as its bytes.
    items_result(&mut h, "Items", "0xE9");
    let copies = clip.texts.lock().unwrap().len();
    h.command("copy insert");
    assert!(h.status(200, 45).contains("Not copied as SQL: a value is not in hand whole"), "{}", h.status(200, 45));
    assert_eq!(clip.texts.lock().unwrap().len(), copies, "nothing copied");
    // The keys could not be read: the server's reason.
    h.db(DbEvent::Keys(Err(DbError::Server("ERROR 1142 (42000): denied".into()))));
    items_result(&mut h, "Items", "x");
    h.command("copy insert");
    let status = h.status(220, 45);
    assert!(status.contains("the tables' columns could not be read (ERROR 1142 (42000): denied)"), "{status}");
}
