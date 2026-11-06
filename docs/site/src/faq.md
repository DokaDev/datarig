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

## Colours look wrong, or the light/dark variant is wrong

- The default theme, `terminal`, uses your terminal's own 16 colours and background. Pick a
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

## Why is my result read-only?

The edit key (`e`, `o`, `D`, …) says why in the status bar: "Read-only: …" or "Can't edit: …".
Once the table's details are read, the result tab strip also says `read-only · <reason>` for a
reason about the statement, the table or the rows; a reason about the connection (a read-only
environment, a server that can't edit, a changed connection, no connection) is said by the key
only. The usual causes:

- The statement is not one `SELECT` of one plain table: a join, `WITH`, `GROUP BY` or
  `DISTINCT`, a subquery, `UNION`, a view.
- The result has no key of the table and no column that compares exactly, so its rows can't be
  found again: add the primary key to the `SELECT`.
- The profile's environment is read-only, or the server cannot edit (MariaDB, MySQL before
  8.0.17).
- The tab's connection changed since the rows were read, or changes were already applied to
  them: run the query again.

Every reason is listed on [Editing data](guide/editing.md#what-can-be-edited).

## Why was my paste refused?

A range pasted into the grid is refused as a whole, never shifted or cut, and the status line
says why ("Not pasted: …"). The usual causes:

- It is wider than the columns from the cursor to the last column: put the cursor further left.
- A column in the range takes no value (computed, generated, an identity always generated), or
  a row in it is marked for deletion.
- It runs past the last row read while the server has more rows: fetch them first, or paste
  fewer rows (otherwise the rows meant to be overwritten would become new rows). Where the result's cursor
  is no longer open, paste fewer rows.
- It runs past the end where the table can't take new rows from this result.
- The text can't be read: rows of different widths, a quote never closed, a carriage return
  alone. `Space d V` reads the same text again as the next format (TSV, CSV, one value).
- Text with commas and no tab is not pasted until you answer: the status line asks whether to
  read it as CSV (`c`) or as one column (`1`), since the commas can be the values' own.
- It is larger than 5,000 rows or 1 MiB.
- The result can't be edited at all (see above).

A paste that went in but shows `!` in the gutter has values their columns don't take: they are
kept, listed first in the Changes tab, and block the Apply until fixed (`e`), dropped
(`Space d r`) or undone. An empty field is an empty value: paste `\N` for NULL. See
[Pasting a range](guide/editing.md#pasting-a-range).

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

## Does datarig contact the internet?

Only to check for a newer release: once a day at most, one HTTPS request to `api.github.com`
with nothing about you, your connections or your databases, through your `HTTPS_PROXY` when one
is set. Turn it off with `:set updates.check=off` or `DATARIG_NO_UPDATE_CHECK=1`; see
[Updates](guide/updates.md) for exactly what is sent. What's new comes with the program and needs
no network. Everything else datarig connects to is what you configure: your databases, and the
SSH hosts of your tunnels.

## Where does datarig keep its files?

| What | Where |
|---|---|
| Config file | `--config <path>`, else `$XDG_CONFIG_HOME/datarig/config.toml`, else `~/.config/datarig/config.toml` (on every OS) |
| Secrets file, theme files | Next to the config file: `secrets.toml`, `themes/<name>.toml` |
| Data (saved queries, datarig's own `known_hosts`) | `~/.local/share/datarig` on Linux, `~/Library/Application Support/datarig` on macOS, `%APPDATA%\datarig` on Windows |
| State (tabs, console buffers, `usage.toml`, `update.toml`, `keychain.toml`) | `~/.local/state/datarig` on Linux, `~/Library/Application Support/datarig/state` on macOS, `%LOCALAPPDATA%\datarig` on Windows |

`$XDG_DATA_HOME` and `$XDG_STATE_HOME`, when set, take precedence on every OS (as
`<dir>/datarig`), and `DATARIG_DATA_DIR` / `DATARIG_STATE_DIR` override both. Windows paths are
untested.

`usage.toml` is what the [tips](getting-started.md#tips) remember: per action how often it ran
by key, from the action menu and from the command line, and which first steps are done. It
holds no times, statements or names of yours, is never sent anywhere, and is not written with
`[ui] tips = "off"`. With `[ui] tips = "until_used"`, delete it while datarig is not running to see every tip
again (with connections or a saved workspace there, the first steps stay done). If you used datarig before tips existed, the first
steps count as done.

`update.toml` keeps the newest version of datarig that ran on this machine, so
[What's new](getting-started.md#whats-new) shows once after an update, and what the
[update check](guide/updates.md) learnt: when it last asked and how that ended, the latest
release, a version you skipped and when you last closed the notice. A second datarig open at the
same time reads it but never writes it.

## The config file has an `[editor] mode` key

It is from an older version: the editor has vim keys only. It is ignored and dropped the next
time the app saves the file.

## Does datarig run on Windows?

CI builds it, lints it and runs the tests that need no database, but nobody has used it on
Windows yet. The Windows notes in this manual describe how it is meant to work.
