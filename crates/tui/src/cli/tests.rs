use super::*;

fn parse(args: &[&str]) -> Result<Cli, String> {
    parse_args(args.iter().map(|s| s.to_string()))
}

#[test]
fn cli_arguments() {
    assert_eq!(parse(&[]), Ok(Cli::Run { config: None, profile: None }));
    assert_eq!(parse(&["prod"]), Ok(Cli::Run { config: None, profile: Some("prod".into()) }));
    assert_eq!(
        parse(&["--config", "/x.toml", "本番"]),
        Ok(Cli::Run { config: Some(PathBuf::from("/x.toml")), profile: Some("本番".into()) })
    );
    assert_eq!(parse(&["--config=/y.toml"]), Ok(Cli::Run { config: Some(PathBuf::from("/y.toml")), profile: None }));
    assert_eq!(parse(&["-h"]), Ok(Cli::Help));
    assert_eq!(parse(&["prod", "--help"]), Ok(Cli::Help));
    assert_eq!(parse(&["--version"]), Ok(Cli::Version));
    assert_eq!(parse(&["-V"]), Ok(Cli::Version));
    assert_eq!(parse(&["--", "-dash"]), Ok(Cli::Run { config: None, profile: Some("-dash".into()) }));
    assert!(parse(&["--nope"]).unwrap_err().starts_with("unknown option --nope"));
    assert!(parse(&["a", "b"]).is_err());
    assert!(parse(&["--config"]).is_err());
}

/// The summary says what datarig is and stays general: database engines are named on their
/// own pages of the docs, not in a line that every new engine would have to change.
#[test]
fn help_summary_names_no_database_engine() {
    let first = HELP.lines().next().unwrap();
    assert!(first.contains("terminal database client"), "{first}");
    for engine in ["PostgreSQL", "MySQL", "MariaDB", "SQLite"] {
        assert!(!first.contains(engine), "{first}");
    }
    assert!(!HELP.contains("for now"), "{HELP}");
}
