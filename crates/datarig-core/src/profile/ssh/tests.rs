use super::*;
use std::path::Path;

fn on() -> SshSettings {
    SshSettings {
        enabled: true,
        host: "bastion".into(),
        user: "ec2-user".into(),
        key_file: Some("~/.ssh/prod.pem".into()),
        ..SshSettings::default()
    }
}

#[test]
fn defaults_and_the_key_path() {
    let s = SshSettings::default();
    assert_eq!((s.port, s.auth, s.enabled), (22, SshAuth::Key, false));
    assert_eq!(s.keepalive(), Some(Duration::from_secs(15)));
    assert_eq!(SshSettings { keepalive: Some(0), ..s.clone() }.keepalive(), None);
    assert_eq!(s.timeout(), Duration::from_secs(10));
    let home = Path::new("/home/me");
    assert_eq!(on().key_path(Some(home)), Some(home.join(".ssh/prod.pem")));
    let abs = SshSettings { key_file: Some("/keys/a.pem".into()), ..on() };
    assert_eq!(abs.key_path(Some(home)), Some(PathBuf::from("/keys/a.pem")));
    assert_eq!(SshSettings { key_file: Some("  ".into()), ..on() }.key_path(Some(home)), None);
}

#[test]
fn problems_of_an_enabled_tunnel() {
    assert_eq!(on().problem(), None);
    assert_eq!(SshSettings { enabled: false, host: String::new(), ..on() }.problem(), None, "off: not checked");
    assert_eq!(SshSettings { host: " ".into(), ..on() }.problem(), Some(SshProblem::Missing("host")));
    assert_eq!(SshSettings { port: 0, ..on() }.problem(), Some(SshProblem::Invalid("port")));
    assert_eq!(SshSettings { user: String::new(), ..on() }.problem(), Some(SshProblem::Missing("user")));
    assert_eq!(SshSettings { key_file: None, ..on() }.problem(), Some(SshProblem::Missing("key_file")));
    let agent = SshSettings { auth: SshAuth::Agent, key_file: None, ..on() };
    assert_eq!(agent.problem(), None);
    let mut pw = SshSettings { auth: SshAuth::Password, key_file: None, ..on() };
    pw.set_source(PasswordSource::Env("NOT A NAME".into()));
    assert_eq!(pw.problem(), Some(SshProblem::Invalid("secret_env")));
    pw.set_source(PasswordSource::Command(" ".into()));
    assert_eq!(pw.problem(), Some(SshProblem::Missing("secret_command")));
}

#[test]
fn the_source_and_the_account() {
    let mut s = on();
    assert_eq!(s.source(), PasswordSource::Keychain);
    s.set_source(PasswordSource::Command("pass show bastion".into()));
    assert_eq!(s.source(), PasswordSource::Command("pass show bastion".into()));
    s.set_source(PasswordSource::Keychain);
    assert_eq!((s.secret_source, s.secret_command), (None, None));
    let id = crate::profile::ProfileId::new();
    assert_eq!(SshSettings::account(id), format!("profile:{id}:ssh"));
}
