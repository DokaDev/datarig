use super::*;

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn typ(f: &mut ProfileForm, s: &str) {
    for c in s.chars() {
        f.key(&key(KeyCode::Char(c)));
    }
}

/// Tab to `target`, or switch to its section first.
fn focus(f: &mut ProfileForm, target: Field) {
    if target.section().is_some_and(|s| s != f.section) {
        f.focus_field(target);
    }
    while f.focus != target {
        f.key(&key(KeyCode::Tab));
    }
}

fn clear(f: &mut ProfileForm) {
    f.key(&KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    f.key(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
}

#[test]
fn fields_update_dsn_without_password() {
    let mut f = ProfileForm::from_profile(&ConnectionConfig::test_db(), "datarig".into(), Some(0));
    assert_eq!(f.dsn.text(), "postgres://datarig@127.0.0.1:55432/datarig?sslmode=disable");
    focus(&mut f, Field::Host);
    clear(&mut f);
    typ(&mut f, "::1");
    assert_eq!(f.dsn.text(), "postgres://datarig@[::1]:55432/datarig?sslmode=disable");
    focus(&mut f, Field::SslMode);
    f.key(&key(KeyCode::Right));
    assert!(f.dsn.text().ends_with("sslmode=prefer"));
    assert!(!f.dsn.text().contains("datarig:"), "password never in the DSN");
}

#[test]
fn dsn_updates_fields_and_moves_password() {
    let mut f = ProfileForm::new_profile();
    focus(&mut f, Field::Dsn);
    clear(&mut f);
    f.paste("postgresql://app%40corp:p%40ss@db.internal:6543/sales?sslmode=require");
    assert_eq!(f.dsn_problem, None);
    assert_eq!(f.host.text(), "db.internal");
    assert_eq!(f.port.text(), "6543");
    assert_eq!(f.user.text(), "app@corp");
    assert_eq!(f.database.text(), "sales");
    assert_eq!(SSL_MODES[f.sslmode], "require");
    assert_eq!(f.password.text(), "p@ss");
    assert_eq!(f.dsn.text(), "postgres://app%40corp@db.internal:6543/sales?sslmode=require");
}

#[test]
fn invalid_dsn_keeps_fields() {
    let mut f = ProfileForm::from_profile(&ConnectionConfig::test_db(), String::new(), Some(0));
    focus(&mut f, Field::Dsn);
    typ(&mut f, "x"); // ...?sslmode=disablex
    assert_eq!(f.dsn_problem, Some(DsnProblem::SslMode("disablex".into())));
    assert_eq!(f.port.text(), "55432");
    clear(&mut f);
    typ(&mut f, "mysql://h/db");
    assert_eq!(f.dsn_problem, Some(DsnProblem::Parse(DsnError::Scheme)));
    assert_eq!(f.host.text(), "127.0.0.1", "fields untouched on error");
    clear(&mut f);
    typ(&mut f, "postgres://h:99999/db");
    assert!(matches!(f.dsn_problem, Some(DsnProblem::Parse(DsnError::Port(_)))));
    clear(&mut f);
    typ(&mut f, "postgres://h/db?application_name=x");
    assert_eq!(f.dsn_problem, Some(DsnProblem::Param("application_name".into())));
    // While typing, every prefix that parses (`postgres://h/db`) already synced the fields.
    assert_eq!((f.host.text(), f.database.text(), f.user.text()), ("h", "db", ""));
    // Editing a field regenerates a valid DSN and clears the problem.
    focus(&mut f, Field::Database);
    typ(&mut f, "2");
    assert_eq!(f.dsn_problem, None);
    assert_eq!(f.dsn.text(), "postgres://h:5432/db2?sslmode=disable");
}

#[test]
fn validation() {
    let mut f = ProfileForm::new_profile();
    assert!(f.errors(|_| false).is_empty(), "no required errors before saving");
    assert_eq!(f.key(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)), FormOutcome::Save);
    f.attempted = true;
    let errs = f.errors(|_| false);
    assert!(errs.contains(&(Field::Name, FieldError::Required)));
    assert!(errs.contains(&(Field::User, FieldError::Required)));
    assert!(!errs.iter().any(|(f, _)| *f == Field::Host), "host has a default");
    typ(&mut f, "prod");
    assert!(f.errors(|n| n == "prod").contains(&(Field::Name, FieldError::NameTaken)));
    focus(&mut f, Field::Port);
    clear(&mut f);
    typ(&mut f, "70000");
    assert!(f.errors(|_| false).contains(&(Field::Port, FieldError::Port)));
    clear(&mut f);
    typ(&mut f, "0");
    assert!(f.errors(|_| false).contains(&(Field::Port, FieldError::Port)));
    clear(&mut f);
    assert!(f.errors(|_| false).contains(&(Field::Port, FieldError::Required)));
}

#[test]
fn buttons_and_cjk_input() {
    let mut f = ProfileForm::new_profile();
    typ(&mut f, "本番DB🐘");
    f.key(&key(KeyCode::Backspace));
    assert_eq!(f.name.text(), "本番DB");
    focus(&mut f, Field::Test);
    assert_eq!(f.key(&key(KeyCode::Enter)), FormOutcome::Test);
    f.key(&key(KeyCode::Right));
    assert_eq!(f.focus, Field::Save);
    assert_eq!(f.key(&key(KeyCode::Enter)), FormOutcome::Save);
    assert_eq!(f.key(&key(KeyCode::Esc)), FormOutcome::Cancel);
    assert_eq!(f.to_profile().name, "本番DB");
}

#[test]
fn copy_names() {
    assert_eq!(copy_name("a", |_| false), "a-copy");
    assert_eq!(copy_name("a", |n| n == "a-copy"), "a-copy2");
}

#[test]
fn a_pasted_url_fills_a_new_profile_and_names_it() {
    let mut f = ProfileForm::new_profile();
    f.set_dsn("  postgres://report:p%40ss@warehouse.internal:6432/sales?sslmode=require\n", |n| n == "sales");
    assert_eq!(f.name.text(), "sales-copy", "the database name, made unique");
    assert_eq!((f.host.text(), f.port.text(), f.user.text()), ("warehouse.internal", "6432", "report"));
    assert_eq!((f.password.text(), SSL_MODES[f.sslmode]), ("p@ss", "require"));
    assert!(!f.dsn.text().contains("p%40ss"), "the password leaves the DSN");
    // No database: the host names it.
    let mut f = ProfileForm::new_profile();
    f.set_dsn("postgresql://me@db.local", |_| false);
    assert_eq!(f.name.text(), "db.local");
}

#[test]
fn sections_split_the_fields_and_pickers_cycle() {
    let mut f = ProfileForm::from_profile(&ConnectionConfig::test_db(), String::new(), Some(0));
    assert_eq!(f.section, Section::Basic);
    assert!(f.fields().starts_with(&[Field::Driver, Field::Name]) && f.fields().contains(&Field::Dsn));
    assert!(!f.fields().contains(&Field::SslMode));
    f.key(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!((f.section, f.focus), (Section::Ssh, Field::SshEnabled));
    f.key(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!((f.section, f.focus), (Section::Advanced, Field::SslMode));
    // MySQL's own are not shown for PostgreSQL.
    let pg: Vec<Field> =
        ADVANCED.into_iter().filter(|f| !matches!(f, Field::ServerKey | Field::KeyRetrieval)).collect();
    assert_eq!(&f.fields()[..pg.len()], &pg[..]);
    // Color: auto, then the named colors in order; Left wraps back to auto.
    focus(&mut f, Field::Color);
    f.key(&key(KeyCode::Right));
    assert_eq!(f.color.as_deref(), Some("red"));
    f.key(&key(KeyCode::Left));
    f.key(&key(KeyCode::Left));
    assert_eq!(f.color.as_deref(), Some("gray"));
    assert_eq!(f.key(&key(KeyCode::Enter)), FormOutcome::Choose(Field::Color));
    // Folder: the known folders.
    f.folders = vec!["work".into(), "work/prod".into()];
    focus(&mut f, Field::Folder);
    f.key(&key(KeyCode::Right));
    f.key(&key(KeyCode::Right));
    assert_eq!(f.folder.as_deref(), Some("work/prod"));
    let p = f.to_profile();
    assert_eq!((p.color.as_deref(), p.folder.as_deref()), (Some("gray"), Some("work/prod")));
    // The driver: only enabled ones can be picked; the profile's own spelling stays.
    let mut c = ConnectionConfig::test_db();
    c.driver = "pg".into();
    let mut f = ProfileForm::from_profile(&c, String::new(), Some(0));
    focus(&mut f, Field::Driver);
    f.key(&key(KeyCode::Right));
    assert_eq!(f.driver, 0);
    assert_eq!(f.to_profile().driver, "pg");
    f.drivers_enabled[1] = true;
    f.key(&key(KeyCode::Right));
    assert_eq!(f.to_profile().driver, "mysql");
}

/// The server-side statement cache (Advanced): the profile's value, toggled with
/// ←/→ or Space, and saved with the profile; nothing else of the profile changes.
#[test]
fn statement_cache_toggles_and_is_saved() {
    let c = ConnectionConfig { statement_cache: false, ..ConnectionConfig::test_db() };
    let mut f = ProfileForm::from_profile(&c, String::new(), Some(0));
    assert!(!f.statement_cache);
    assert!(f.fields().iter().all(|x| *x != Field::StatementCache), "Basic does not show it");
    focus(&mut f, Field::StatementCache);
    assert_eq!(f.section, Section::Advanced);
    f.key(&key(KeyCode::Right));
    assert!(f.to_profile().statement_cache);
    f.key(&key(KeyCode::Char(' ')));
    assert!(!f.to_profile().statement_cache);
    f.key(&key(KeyCode::Left));
    let saved = f.to_profile();
    assert!(saved.statement_cache);
    assert_eq!((saved.host.as_str(), saved.port, saved.sslmode.as_str()), ("127.0.0.1", 55432, "disable"));
    assert!(ProfileForm::new_profile().statement_cache, "on for a new profile");
}

/// MySQL's server key file and key retrieval (Advanced, MySQL only): the profile's values,
/// edited and saved; an empty key file is none. A PostgreSQL profile does not show them.
#[test]
fn mysql_key_options_are_edited_and_saved() {
    let my = ConnectionConfig {
        driver: "mysql".into(),
        port: 3306,
        server_public_key_file: Some("~/db.pem".into()),
        ..ConnectionConfig::test_db()
    };
    let mut f = ProfileForm::from_profile(&my, String::new(), Some(0));
    f.open_section(Section::Advanced);
    assert_eq!(f.fields()[..2], [Field::ServerKey, Field::KeyRetrieval]);
    assert_eq!(f.server_key.text(), "~/db.pem");
    focus(&mut f, Field::ServerKey);
    clear(&mut f);
    typ(&mut f, " /keys/prod.pem ");
    focus(&mut f, Field::KeyRetrieval);
    f.key(&key(KeyCode::Right));
    assert!(f.to_profile().allow_public_key_retrieval);
    f.key(&key(KeyCode::Char(' ')));
    assert!(!f.to_profile().allow_public_key_retrieval);
    f.key(&key(KeyCode::Left));
    let saved = f.to_profile();
    assert_eq!(
        (saved.server_public_key_file.as_deref(), saved.allow_public_key_retrieval),
        (Some("/keys/prod.pem"), true)
    );
    focus(&mut f, Field::ServerKey);
    clear(&mut f);
    assert_eq!(f.to_profile().server_public_key_file, None);
    let mut pg = ProfileForm::from_profile(&ConnectionConfig::test_db(), String::new(), Some(0));
    pg.open_section(Section::Advanced);
    assert!(pg.fields().iter().all(|f| !matches!(f, Field::ServerKey | Field::KeyRetrieval)));
    let new = ProfileForm::new_profile().to_profile();
    assert_eq!((new.server_public_key_file, new.allow_public_key_retrieval), (None, false));
}

/// The server key file must be absolute or under `~/` (a relative one would depend on the
/// directory datarig was started in): another path is an error at once, next to the field.
#[test]
fn the_server_key_file_must_be_absolute_or_under_home() {
    let my = ConnectionConfig { driver: "mysql".into(), port: 3306, ..ConnectionConfig::test_db() };
    let mut f = ProfileForm::from_profile(&my, String::new(), Some(0));
    focus(&mut f, Field::ServerKey);
    let key_errors = |f: &ProfileForm| -> Vec<FieldError> {
        f.errors(|_| false).into_iter().filter(|(field, _)| *field == Field::ServerKey).map(|(_, e)| e).collect()
    };
    for (path, ok) in
        [("keys/x.pem", false), ("./k.pem", false), ("~k.pem", false), ("/k.pem", true), ("~/k.pem", true)]
    {
        clear(&mut f);
        typ(&mut f, path);
        let expected = if ok { vec![] } else { vec![FieldError::AbsolutePath] };
        assert_eq!(key_errors(&f), expected, "{path}");
    }
    clear(&mut f);
    assert!(key_errors(&f).is_empty(), "none is fine");
}

/// The SSH section: off shows the switch only; on, the fields follow the way
/// to log in and the secret's source; a profile gets tunnel settings only once turned on, and
/// keeps them when turned off again.
#[test]
fn the_ssh_section_follows_the_tunnel_settings() {
    let mut f = ProfileForm::from_profile(&ConnectionConfig::test_db(), String::new(), Some(0));
    focus(&mut f, Field::SshEnabled);
    assert_eq!(f.section, Section::Ssh);
    assert_eq!(f.fields(), [&[Field::SshEnabled][..], &BUTTONS].concat());
    assert_eq!(f.to_profile().ssh, None, "never set up");
    f.key(&key(KeyCode::Char(' ')));
    assert!(f.ssh_enabled);
    let with = |f: &ProfileForm| f.ssh_fields();
    assert_eq!(
        with(&f),
        [
            Field::SshEnabled,
            Field::SshHost,
            Field::SshPort,
            Field::SshUser,
            Field::SshAuth,
            Field::SshKeyFile,
            Field::SshSource,
            Field::SshSecret,
            Field::SshKeepalive,
            Field::SshTimeout
        ]
    );
    // The agent has no secret and no key file.
    focus(&mut f, Field::SshAuth);
    f.key(&key(KeyCode::Right));
    f.key(&key(KeyCode::Right));
    assert_eq!(f.ssh_auth, SshAuth::Agent);
    assert!(!with(&f).contains(&Field::SshKeyFile) && !with(&f).contains(&Field::SshSource));
    // A password from a command.
    f.key(&key(KeyCode::Left));
    assert_eq!(f.ssh_auth, SshAuth::Password);
    focus(&mut f, Field::SshSource);
    f.key(&key(KeyCode::Right));
    f.key(&key(KeyCode::Right));
    assert_eq!(f.ssh_source, SourceKind::Command);
    assert!(with(&f).contains(&Field::SshCommand) && !with(&f).contains(&Field::SshSecret));
    // Required once saving was tried; bad numbers at once.
    f.attempted = true;
    let errors = f.errors(|_| false);
    for field in [Field::SshHost, Field::SshUser, Field::SshCommand] {
        assert!(errors.contains(&(field, FieldError::Required)), "{field:?}: {errors:?}");
    }
    focus(&mut f, Field::SshTimeout);
    typ(&mut f, "ten");
    assert!(f.errors(|_| false).contains(&(Field::SshTimeout, FieldError::Seconds)));
    clear(&mut f);
    focus(&mut f, Field::SshHost);
    typ(&mut f, "bastion.example.com");
    focus(&mut f, Field::SshUser);
    typ(&mut f, "ec2-user");
    focus(&mut f, Field::SshCommand);
    typ(&mut f, "pass show bastion");
    assert!(f.errors(|_| false).is_empty(), "{:?}", f.errors(|_| false));
    let ssh = f.to_profile().ssh.expect("on");
    assert_eq!((ssh.host.as_str(), ssh.user.as_str(), ssh.port), ("bastion.example.com", "ec2-user", 22));
    assert_eq!(ssh.source(), PasswordSource::Command("pass show bastion".into()));
    // Saved, then turned off in the form: kept, off. (Turned on and off before a save: none.)
    let saved = f.to_profile();
    focus(&mut f, Field::SshEnabled);
    f.key(&key(KeyCode::Char(' ')));
    assert_eq!(f.to_profile().ssh, None);
    let mut again = ProfileForm::from_profile(&saved, String::new(), Some(0));
    focus(&mut again, Field::SshEnabled);
    again.key(&key(KeyCode::Char(' ')));
    let off = again.to_profile().ssh.expect("kept");
    assert!(!off.enabled && off.host == "bastion.example.com");
}

/// A secret typed in the SSH section is only for a source that stores it.
#[test]
fn only_a_storing_source_takes_a_typed_tunnel_secret() {
    let c = ConnectionConfig {
        ssh: Some(datarig_core::profile::ssh::SshSettings {
            enabled: true,
            host: "b".into(),
            user: "u".into(),
            key_file: Some("~/.ssh/k.pem".into()),
            ..Default::default()
        }),
        ..ConnectionConfig::test_db()
    };
    let mut f = ProfileForm::from_profile(&c, String::new(), Some(0));
    assert_eq!(f.ssh_typed_secret(), None);
    focus(&mut f, Field::SshSecret);
    typ(&mut f, "passphrase");
    assert_eq!(f.ssh_typed_secret().as_deref(), Some("passphrase"));
    focus(&mut f, Field::SshSource);
    for _ in 0..4 {
        f.key(&key(KeyCode::Right));
    }
    assert_eq!(f.ssh_source, SourceKind::Prompt);
    assert_eq!(f.ssh_typed_secret(), None);
}
