use super::*;

#[test]
fn splits_with_shell_rules_without_expanding() {
    assert_eq!(split("op read op://work/db/password").unwrap(), ["op", "read", "op://work/db/password"]);
    assert_eq!(split(r#"pass show "db/prod main""#).unwrap(), ["pass", "show", "db/prod main"]);
    assert_eq!(split(r"echo a\ b 'c d' $HOME ~ *").unwrap(), ["echo", "a b", "c d", "$HOME", "~", "*"]);
    assert_eq!(split("sh -c 'pass show db | head -n1'").unwrap(), ["sh", "-c", "pass show db | head -n1"]);
    assert_eq!(split("   "), Err(CommandError::Empty));
    assert!(matches!(split("echo 'unclosed"), Err(CommandError::Parse)));
}

#[test]
fn trims_one_trailing_newline() {
    assert_eq!(trim_newline("pw\n"), "pw");
    assert_eq!(trim_newline("pw\r\n"), "pw");
    assert_eq!(trim_newline("pw\n\n"), "pw\n", "only one");
    assert_eq!(trim_newline("pw "), "pw ", "spaces are part of the password");
    assert_eq!(trim_newline("pw"), "pw");
}

#[test]
fn a_missing_program_cannot_start() {
    let e = run("datarig-no-such-program-xyz --flag", TIMEOUT).unwrap_err();
    assert!(matches!(&e, CommandError::Start { program, .. } if program == "datarig-no-such-program-xyz"), "{e:?}");
}

#[cfg(unix)]
#[test]
fn output_status_and_stderr() {
    assert_eq!(run("printf 's3cret\\n'", TIMEOUT).unwrap(), "s3cret");
    assert_eq!(run("printf 'with space\\r\\n'", TIMEOUT).unwrap(), "with space");
    assert_eq!(run("sh -c 'printf pw; echo noise >&2'", TIMEOUT).unwrap(), "pw", "stderr is ignored on success");
    // No implicit shell: `$HOME` reaches the program as is.
    assert_eq!(run("printf %s $HOME", TIMEOUT).unwrap(), "$HOME");
    assert_eq!(run("true", TIMEOUT), Err(CommandError::NoOutput));
    assert_eq!(run("printf '\\n'", TIMEOUT), Err(CommandError::NoOutput));
    let e = run("sh -c 'printf leaked; echo \"item not found\" >&2; exit 3'", TIMEOUT).unwrap_err();
    assert_eq!(e, CommandError::Failed { status: "3".into(), stderr: "item not found".into() });
    // A long stderr is cut; stdout never shows up in the error.
    let e = run("sh -c 'printf secret-out; head -c 1000 /dev/zero | tr \"\\0\" x >&2; exit 1'", TIMEOUT).unwrap_err();
    let CommandError::Failed { stderr, .. } = &e else { panic!("{e:?}") };
    assert_eq!(stderr.chars().count(), STDERR_LIMIT + 1, "cut + …");
    assert!(stderr.ends_with('…') && !format!("{e:?}").contains("secret-out"));
    assert_eq!(run("printf '\\377'", TIMEOUT), Err(CommandError::NotUtf8));
}

#[cfg(unix)]
#[test]
fn a_slow_command_is_killed_after_the_timeout() {
    let t = Duration::from_millis(200);
    let started = Instant::now();
    assert_eq!(run("sleep 5", t), Err(CommandError::Timeout(t)));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
}
