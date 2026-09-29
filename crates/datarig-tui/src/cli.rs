//! Command line: `datarig [--config <path>] [<profile>]`, `--help`, `--version`.

use std::path::PathBuf;

const USAGE: &str = "usage: datarig [--config <path>] [<profile>]";

pub(crate) const HELP: &str = "datarig — dive into any database, right from your terminal (PostgreSQL for now)

usage: datarig [--config <path>] [<profile>]

  <profile>          connect to this connection profile and open a console
                     (without it, nothing connects until you pick a profile)
  --config <path>    config file (default: $XDG_CONFIG_HOME/datarig/config.toml,
                     else ~/.config/datarig/config.toml)
  -h, --help         show this help
  -V, --version      show the version

environment:
  DATARIG_SECRET_STORE=memory   keep passwords in memory only, never touch the OS
                                  keychain (default: keychain)
";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Cli {
    Run { config: Option<PathBuf>, profile: Option<String> },
    Help,
    Version,
}

/// `datarig [--config <path>] [<profile>]`, `--help`, `--version`. `--` ends the options
/// (for a profile name that starts with `-`).
pub(crate) fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
    let mut args = args.into_iter();
    let (mut config, mut profile) = (None, None);
    let mut options = true;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" if options => return Ok(Cli::Help),
            "-V" | "--version" if options => return Ok(Cli::Version),
            "--config" if options => config = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--" if options => options = false,
            _ if options && a.starts_with("--config=") => config = Some(PathBuf::from(&a["--config=".len()..])),
            _ if options && a.starts_with('-') && a.len() > 1 => return Err(format!("unknown option {a}\n{USAGE}")),
            _ if profile.is_none() => profile = Some(a),
            _ => return Err(format!("only one profile name is allowed\n{USAGE}")),
        }
    }
    Ok(Cli::Run { config, profile })
}

#[cfg(test)]
mod tests;
