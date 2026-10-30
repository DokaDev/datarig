# Command line

<!-- Generated from `datarig --help` and ENV_VARS in crates/tui/src/lib.rs. Do not edit;
     run `DATARIG_BLESS=1 cargo test -p datarig-tui --test cli_doc` at the repository root. -->

`datarig --help` prints:

```text
datarig — dive into any database, right from your terminal (PostgreSQL for now)

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
```

`datarig --version` (or `-V`) prints `datarig` and the version, as `datarig 1.2.3`. An option it does not know prints what is wrong and the usage line, and exits with status 2.

## Environment variables

| Variable | What it does |
|---|---|
| `DATARIG_SECRET_STORE` | `memory` keeps passwords in memory only: the OS keychain and the secrets file are never touched. `keychain`, or unset (or empty): the profiles' own password stores. Any other value is an error and datarig does not start. |
| `XDG_CONFIG_HOME` | The config file is `$XDG_CONFIG_HOME/datarig/config.toml` when it is set (and `--config` is not given). |
| `HOME` | The home directory: the config file is `~/.config/datarig/config.toml` without `XDG_CONFIG_HOME`, the data and state directories default to places under it, and `~` in an SSH key file is it. |
| `USERPROFILE` | The home directory where `HOME` is not set (Windows). |
| `DATARIG_DATA_DIR` | The data directory (saved queries, datarig's own `known_hosts`), over every other rule. |
| `DATARIG_STATE_DIR` | The state directory (restored tabs, console buffers, one-time notices, the log of deleted passwords, the keychain offer's answer, what the tips remember), over every other rule. |
| `XDG_DATA_HOME` | The data directory is `$XDG_DATA_HOME/datarig` when it is set, on every OS. Otherwise `~/.local/share/datarig` (Linux and other Unix), `~/Library/Application Support/datarig` (macOS) or `%APPDATA%\datarig` (Windows). |
| `XDG_STATE_HOME` | The state directory is `$XDG_STATE_HOME/datarig` when it is set, on every OS. Otherwise `~/.local/state/datarig`, `~/Library/Application Support/datarig/state` or `%LOCALAPPDATA%\datarig`. |
| `APPDATA` | Windows: the data directory's default is `%APPDATA%\datarig`. |
| `LOCALAPPDATA` | Windows: the state directory's default is `%LOCALAPPDATA%\datarig`. |
| `LC_ALL` | With `language = "auto"`, the first of `LC_ALL`, `LC_MESSAGES` and `LANG` that is set picks the language: Korean when it starts with `ko`, else English. |
| `LC_MESSAGES` | As `LC_ALL`, when that is not set. |
| `LANG` | As `LC_ALL`, when neither it nor `LC_MESSAGES` is set. |
| `VISUAL` | The editor a query is opened in to edit it outside datarig, split into words as a shell would. |
| `EDITOR` | As `VISUAL`, when that is not set; without either, `vi` (`notepad` on Windows). |
| `SSH_CONNECTION` | Set (or `SSH_TTY`): datarig runs in an SSH session, so `clipboard = "auto"` copies through OSC 52 (the terminal), and the macOS keychain offer is never made. |
| `SSH_TTY` | As `SSH_CONNECTION`. |
| `SSH_CLIENT` | Set: the macOS keychain offer is never made (as in an SSH session). |
| `SSH_AUTH_SOCK` | The ssh-agent an SSH tunnel with `auth = "agent"` logs in with. |
| `TMUX` | Set: the status bar says that an OSC 52 copy needs `set -g set-clipboard on` in tmux. |

A profile with `password_source = "env"` reads its password from the variable its `password_env` names, and an SSH tunnel with `secret_source = "env"` its secret from the one its `secret_env` names.
