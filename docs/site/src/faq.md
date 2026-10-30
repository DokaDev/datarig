# FAQ and troubleshooting

## macOS asks for my login password after every upgrade

datarig's release binaries are not signed, so macOS sees each upgrade as a new program and asks
again before it may read each saved password. Switch keychain access to Apple's `security` tool:
`:set secrets.macos_keychain_access=security_tool`. Read the trade-off first; see
[macOS: keychain access through `security`](guide/connections.md#macos-keychain-access-through-security).
datarig also offers the switch once when it notices such a prompt.

## The keychain does not work at all (headless Linux, SSH sessions)

When the OS keychain is unavailable, datarig asks for the password when connecting and keeps it
in memory only. To avoid the prompt, give the profile another source: `password_source = "file"`
(`secrets.toml` next to the config file), `"command"` or `"env"`. See
[Passwords](guide/connections.md#passwords).

## Icons look wrong

The explorer, tab bar and status bar can use Nerd Font icons. datarig asks once, with a preview,
whether your terminal shows them, because a terminal's name says nothing about its font. If you
answered wrongly, `:set icons=off` (or `on`) changes it, and `:set icons=auto` asks again. With
icons off, text marks are used instead (`SSH`, `RO`, `○`, `●`, `✕`).

## The screen says it must be larger

datarig needs a terminal of at least 80×24. A smaller window shows only that note until it is
resized.

## Colors look wrong, or the light/dark variant is wrong

- The default theme, `terminal`, uses your terminal's own 16 colors and background. Pick a
  truecolor theme with `:set theme=dark` (or any name on [Themes](guide/themes.md)).
- `catppuccin`, `tokyo-night` and `gruvbox` follow the terminal's background, which datarig asks
  once at startup (OSC 11). A terminal that does not answer counts as dark; the settings screen
  (`Space ,`) says what it found. Pick the variant directly (`gruvbox-light`, …) to override it.
- An error in a theme file is reported with the file and line, and the app draws with
  `terminal` until it is fixed.

## `Ctrl+Enter` does nothing

`Ctrl+Enter` reaches datarig only in terminals that support the kitty keyboard protocol.
`Ctrl+E` runs the statement under the cursor everywhere.

## Copying does not reach my clipboard over SSH

With `clipboard = "auto"` (the default), datarig uses OSC 52 in an SSH session and the system
clipboard otherwise. Your terminal must allow OSC 52; `osc52_max_bytes` caps how much is sent.
`"+p` in the editor cannot read the clipboard through OSC 52; use your terminal's paste.

## MySQL or MariaDB: something is missing

MySQL support is partial and MariaDB is a best effort (10.6 and newer). MySQL 5.7 and older are
refused. See [MySQL](guide/mysql.md) for what it supports and what not, including the plan
view (not on MariaDB) and a typed `USE` (use `:use`).

## `COPY … FROM STDIN` or `COPY … TO STDOUT` fails

They are not sent: their data would go through PostgreSQL's COPY sub-protocol, which the
session does not carry. Use [Export](guide/export.md) to write a result to a file.

## A profile does not connect and names its environment, preset or tunnel

A profile that names an environment, a Startup SQL preset or a tunnel preset that does not
exist is an error of that profile. It never connects with the default rules, without the
preset, or directly instead. Pick an existing one in the profile's form.

## A second datarig does not restore my tabs

Only one instance keeps the workspace (tabs, console buffers) at a time. A second instance
started while the first runs restores no tabs and writes none of the workspace files.

## Where does datarig keep its files?

| What | Where |
|---|---|
| Config file | `--config <path>`, else `$XDG_CONFIG_HOME/datarig/config.toml`, else `~/.config/datarig/config.toml` (on every OS) |
| Secrets file, theme files | Next to the config file: `secrets.toml`, `themes/<name>.toml` |
| Data (saved queries, datarig's own `known_hosts`) | `~/.local/share/datarig` on Linux, `~/Library/Application Support/datarig` on macOS, `%APPDATA%\datarig` on Windows |
| State (tabs, console buffers, `usage.toml`, `keychain.toml`) | `~/.local/state/datarig` on Linux, `~/Library/Application Support/datarig/state` on macOS, `%LOCALAPPDATA%\datarig` on Windows |

`$XDG_DATA_HOME` and `$XDG_STATE_HOME`, when set, take precedence on every OS (as
`<dir>/datarig`), and `DATARIG_DATA_DIR` / `DATARIG_STATE_DIR` override both. Windows paths are
untested.

`usage.toml` is what the [tips](getting-started.md#tips) remember: per action how often it ran
by key, from the action menu and from the command line, and which first steps are done. It
holds no times, statements or names of yours, is never sent anywhere, and is not written with
`[ui] tips = "off"`. Delete it while datarig is not running to see every tip again (with connections or a saved
workspace there, the first steps stay done). If you used datarig before tips existed, the first
steps count as done.

## The config file has an `[editor] mode` key

It is from an older version: the editor has vim keys only. It is ignored and dropped the next
time the app saves the file.

## Does datarig run on Windows?

CI builds it, lints it and runs the tests that need no database, but nobody has used it on
Windows yet. The Windows notes in this manual describe how it is meant to work.
