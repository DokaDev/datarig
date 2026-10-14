use super::*;

#[test]
fn the_method_follows_the_setting_and_the_session() {
    use ClipboardSetting as S;
    assert_eq!(plan(S::Auto, true), Plan::Osc52, "over SSH the system clipboard is the remote one");
    assert_eq!(plan(S::Auto, false), Plan::SystemThenOsc52);
    assert_eq!(plan(S::System, true), Plan::System);
    assert_eq!(plan(S::System, false), Plan::System);
    assert_eq!(plan(S::Osc52, false), Plan::Osc52);
    assert_eq!(plan(S::Osc52, true), Plan::Osc52);
}

#[test]
fn an_ssh_session_is_seen_from_its_variables() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    };
    assert!(in_ssh(env(&[("SSH_TTY", "/dev/pts/1")])));
    assert!(in_ssh(env(&[("SSH_CONNECTION", "10.0.0.1 51000 10.0.0.2 22")])));
    assert!(!in_ssh(env(&[("SSH_TTY", "")])), "empty is unset");
    assert!(!in_ssh(env(&[("TMUX", "/tmp/tmux-501/default,1,0")])));
}

#[test]
fn osc52_is_base64_between_the_escape_codes() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    assert_eq!(base64("陳大文 🐘\t\n".as_bytes()), "6Zmz5aSn5paHIPCfkJgJCg==");
    assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
}
