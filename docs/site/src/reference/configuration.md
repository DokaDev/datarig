# Configuration

<!-- Generated from crates/core/src/config/doc.rs and the `:set` settings. Do not edit;
     run `DATARIG_BLESS=1 cargo test -p datarig-tui --test config_doc` at the repository root. -->

The config file is `--config <path>`, else `$XDG_CONFIG_HOME/datarig/config.toml`, else `~/.config/datarig/config.toml` (on every OS). Without it datarig starts with no profiles and creates it on the first save. The app edits the file in place: comments and key order are kept, a setting is written only when it differs from its default, and passwords are never written. A file with an unknown key, or a value a key does not take, is not used at all: datarig says which key and runs with the defaults, saving nothing.

Keys marked `:set` can also be changed from the `:` command line (`:set <key>=<value>`) and on the settings screen, which save them here.

## Top level

Keys before the first table header.

| Key | Default | Values | Description |
|---|---|---|---|
| `version` | 1, the original format | `1`, `2` | The file's format version. datarig writes `2` (every profile has an `id`) and upgrades a version 1 file once at launch; older versions of datarig cannot read version 2. |
| `language` | `"en"` | `"en"`, `"ko"`, `"auto"` | The language of the whole UI; English unless set. Auto follows LC_ALL, LC_MESSAGES or LANG. Data and database messages are never translated. Also `:set language`. |
| `icons` | `"auto"` | `"auto"`, `"on"`, `"off"` | Nerd Font glyphs for connections, key columns and marks. Needs a Nerd Font as the terminal font. Auto: not decided yet — text marks until you answer the question with a preview (choosing auto asks again). Also `:set icons`. |
| `theme` | `"terminal"` | a built-in theme's name, or the name of a theme file | The colors of the whole UI. terminal uses your terminal's own colors. catppuccin, tokyo-night and gruvbox follow the terminal's background (light or dark); a variant name such as gruvbox-light pins one. Your own themes: themes/&lt;name>.toml next to the config file. Also `:set theme`. |
| `page_size` | `500` | 1 to 2147483647 | Rows a result fetches from the server at a time (one page of the grid). |
| `result_window_rows` | `10000` | 1000 to 10000000 | Rows of a result kept in memory; the rest go to a temporary file on disk. |
| `spill_limit` | `"1GB"` | bytes as a number, or `"512MB"`, `"2GB"` (KB, MB and GB are powers of 1024); `0` or `"off"` for no limit | The most a result's temporary file may take; fetching stops when it is reached. An environment may set its own. |
| `idle_disconnect` | `"off"` | a duration from `"1m"` to `"24h"` (`"300s"`, `"5m"`), or `0` or `"off"` | How long a connection may sit unused before the app closes it: each tab's session on its own, then the connection's other sessions and its SSH tunnel. The next run or explorer action connects again. A tab with an open transaction is warned a minute before; the server rolls the transaction back when its session closes. The server also gets a limit a minute longer for each query session (PostgreSQL 14 or later on a direct connection, MySQL). An environment can set its own. Also `:set idle_disconnect`. |
| `lock_wait` | `"off"` | a duration from `"1s"` to `"24h"` (`"5s"`, `"1m"`), or `0` or `"off"` | How long a statement of a query session waits for a lock another session holds before the server cancels it (PostgreSQL lock_timeout; MySQL lock_wait_timeout and innodb_lock_wait_timeout). Off keeps the server's setting. Explorer lookups keep their own two seconds. An environment can set its own, and a Startup SQL or a profile's own startup SQL its own for its sessions. Also `:set lock_wait`. |
| `folders` | `[]` | a list of folder paths (`"work/prod"`) | Every folder of the explorer, so that empty folders are kept. A profile's own `folder` adds its folder too. |
| `last_used` | none | a profile's `id` | Written by datarig: the profile connected to most recently, where the explorer's cursor starts. A version 1 file has the profile's name here. |
| `detail_view` | `"panel"` | `"panel"`, `"statusbar"` | How the result inspector (i in the grid) shows the selected cell: a panel with Cell and Row tabs, or a one-line preview in the status bar. Also `:set detail_view`. |
| `clipboard` | `"auto"` | `"auto"`, `"system"`, `"osc52"` | How copies reach the clipboard. OSC 52 goes through the terminal, so it also works over SSH; inside tmux it needs set -g set-clipboard on. Also `:set clipboard`. |
| `copy_header` | `"auto"` | `"auto"`, `"on"`, `"off"` | Whether y, Y and Copy as TSV or CSV start with a row of column names. Also `:set copy_header`. |
| `osc52_max_bytes` | `100000` | a positive number of bytes | The longest copy sent through OSC 52 (bytes of base64). A longer one is not sent: terminals and tmux drop long OSC 52 sequences without a word. |

## `[editor]`

The query editor.

| Key | Default | Values | Description |
|---|---|---|---|
| `mode` |  | any | Retired (the editor has vim keys only): accepted with any value, ignored, and dropped the next time the app saves the file. |
| `cursor_shape` | `"on"` | `"on"`, `"off"` | The cursor shows the editor's mode: a block in Normal and Visual, a bar in Insert and while typing into any text field. Off leaves the terminal's own cursor alone. Also `:set editor.cursor_shape`. |
| `clipboard` | `"on"` | `"on"`, `"off"` | Whether y, d, c, x and the other yanks and deletes that name no register also go to the system clipboard, as in Vim with clipboard=unnamedplus (the clipboard setting decides how they get there). "+ and "* always do, "a to "z and "_ never. "+p reads the clipboard only when you type it. Also `:set editor.clipboard`. |
| `format_keyword_case` | `"preserve"` | `"preserve"`, `"upper"`, `"lower"` | How the formatter (Space e f, :format) writes the words the editor highlights as keywords. Names, strings and comments are never changed. Also `:set editor.format_keyword_case`. |
| `format_indent` | `4` | `4`, `2` | How many spaces the formatter indents each level with. Also `:set editor.format_indent`. |
| `auto_pairs` | `"off"` | `"off"`, `"on"` | Whether ( [ { ' " and ` typed in Insert mode also put their closing character after the cursor (typing it there steps over it, Backspace deletes the empty pair). Never inside strings or comments, before a word, or for a paste. Off is how Vim types. Also `:set editor.auto_pairs`. |
| `soft_tab_backspace` | `"on"` | `"on"`, `"off"` | Whether Backspace in Insert mode deletes the spaces before the cursor back to the tab stop (every 4 columns) at once, as Tab types them, when nothing but spaces lies between (as Vim's softtabstop=4 does then). With anything else between, such as a letter or a tab character, Backspace deletes one character. Also `:set editor.soft_tab_backspace`. |
| `run_hints` | `"on"` | `"on"`, `"off"` | Whether a dim hint after a statement's last line says what its last run did (rows, rows affected, the error, rolled back or cancelled, how long it took and when). The hint is not part of the text: it is never yanked, saved or searched, and it goes when the statement is edited. Also `:set editor.run_hints`. |

## `[explorer]`

The explorer.

| Key | Default | Values | Description |
|---|---|---|---|
| `size_heat` | `3` | `3`, `5`, `"off"` (`"on"`, its old value, reads as `3`) | Whether, and in how many levels, the explorer colors the estimates of the largest tables of each schema: the largest tenth by size (one at least). 3 levels: amber from 1 MiB, red from 1 GiB. 5 levels: yellow from 1 MiB, amber from 100 MiB, orange from 1 GiB, red from 10 GiB. Only the estimates already read are used; nothing more is asked of the server. Also `:set explorer.size_heat`. |

## `[ui]`

Things the whole screen shows.

| Key | Default | Values | Description |
|---|---|---|---|
| `yank_flash` | `"on"` | `"on"`, `"off"` | Whether the cells a copy took from the results grid flash briefly once the copy was sent to the clipboard. A copy that fails or is refused does not flash. Also `:set ui.yank_flash`. |
| `tips` | `"on"` | `"on"`, `"off"` | Whether empty places show a tip about a key (an empty editor or result, the workspace without tabs, spare room in the status bar), the welcome panel its first steps, and an action run from a menu a few times names its key once. Tips follow the key bindings, remapped keys included. Also `:set ui.tips`. |

## `[secrets]`

Passwords and the OS keychain.

| Key | Default | Values | Description |
|---|---|---|---|
| `default_source` | `"auto"` | `"auto"`, `"keychain"`, `"file"`, `"command"`, `"env"`, `"prompt"` | Where a new profile keeps its password. Auto uses the OS keychain when it works, else asks on every connect. Also `:set secrets.default_source`. |
| `macos_keychain_access` | `"direct"` | `"direct"`, `"security_tool"` | macOS only (no effect elsewhere). Direct: macOS ties Always Allow to the datarig binary, and releases are unsigned, so it asks for your login password again after each upgrade. Through /usr/bin/security: Always Allow given to Apple's tool lasts across upgrades, but any program running as you can then read these passwords with it, without being asked. Also `:set secrets.macos_keychain_access`. |

## `[commands]`

The `:` command line.

| Key | Default | Values | Description |
|---|---|---|---|
| `position` | `"popup"` | `"popup"`, `"bottom"` | Where the : command line opens: a box near the top with the matches below the input, or the last line with the matches above it. Also `:set commands.position`. |

## `[[connections]]`

One table per connection profile, in the explorer's order. Passwords are never written here: `password_source` says where a profile's password comes from.

| Key | Default | Values | Description |
|---|---|---|---|
| `id` | a new one, written by the launch-time upgrade | a UUID | The profile's stable id: its keychain entries and `last_used` refer to it, so a rename keeps them. datarig assigns it; two profiles with one id are an error. |
| `name` | `"local-pg"` | text | The name the explorer shows and `datarig <profile>` takes. |
| `driver` | `"postgres"` | `"postgres"` (also `"postgresql"`, `"pg"`), `"mysql"` (also `"mariadb"`) | The kind of database. |
| `host` | `"127.0.0.1"` | a host name or address | The server's host. |
| `port` | `5432` | a port number | The server's port. Left out it is 5432 whatever the driver, so a MySQL profile says `3306`. |
| `user` | `""` | text | The database user. |
| `password` | none | text | A plaintext password from an older file. At launch it is moved to the profile's password store (the keychain or the secrets file) and removed from the file; datarig never writes it. |
| `database` | `""` | text | The database to connect to. |
| `sslmode` | PostgreSQL `"disable"`; MySQL `"PREFERRED"` (`"DISABLED"` with `server_public_key_file`) | PostgreSQL: `"disable"`, `"prefer"`, `"require"`, `"verify-ca"`, `"verify-full"`; MySQL: `"DISABLED"`, `"PREFERRED"`, `"REQUIRED"`, `"VERIFY_CA"`, `"VERIFY_IDENTITY"` | How the connection uses TLS, spelled as the database's own client spells it. |
| `sslrootcert` | none | a file path | The CA file the server's certificate chain is checked against. |
| `sslcert` | none | a file path | The client certificate file, used with `sslkey`. |
| `sslkey` | none | a file path | The client certificate's private key file. |
| `dsn` | none | a connection string | A connection string kept as written because it cannot be turned into fields (not a URL, or parameters other than `sslmode`, `sslrootcert`, `sslcert` and `sslkey`). A URL of the profile's driver with only those becomes fields when the file is read. |
| `folder` | the top level | a folder path (`"work/prod"`) | The explorer folder the profile is in. Folders only organize profiles. |
| `color` | a color picked from the name | `"red"`, `"orange"`, `"yellow"`, `"green"`, `"teal"`, `"cyan"`, `"blue"`, `"indigo"`, `"purple"`, `"pink"`, `"brown"`, `"gray"`, or `"#rrggbb"` | The profile's color in the explorer and on its tabs. |
| `icon` | the driver's icon | the name of one of datarig's icons | The Nerd Font icon next to the profile (an unknown name shows the driver's icon). |
| `policy` | the default rules | the name of a `[policy.<env>]` table | The profile's environment. A name no environment has is an error of the profile: it does not connect. |
| `password_source` | `"keychain"` | `"keychain"`, `"file"` (the secrets file), `"command"`, `"env"`, `"prompt"` | Where the profile's password comes from. |
| `password_command` | none | a command | With `password_source = "command"` (required then): the command whose output is the password. |
| `password_env` | none | an environment variable's name | With `password_source = "env"` (required then): the variable that holds the password. |
| `statement_cache` | `true` | `true`, `false` | Keep prepared statements on the server between transactions. Turn it off for a server behind a pooler in transaction mode that does not support them (PgBouncer before 1.22, or with `max_prepared_statements = 0`); the driver also turns it off for a connection when the server keeps losing them. |
| `server_public_key_file` | none | a PEM file path | MySQL: the server's RSA public key. A `caching_sha2_password` login over an unencrypted connection encrypts the password with it and never asks the server for a key. |
| `allow_public_key_retrieval` | `false` | `true`, `false` | MySQL: whether a direct, unencrypted connection to a server that is not on this machine may ask the server for its public key. Whoever sits in between could send theirs and read the password, so such a login is refused when off. A loopback server and one reached through an SSH tunnel are asked either way. |
| `tunnel` | a direct connection | the name of a `[tunnels.<name>]` table | The tunnel preset the profile connects through. A preset that is not there, or a preset and an enabled `[connections.ssh]` at once, is an error of the profile. |
| `startup` | the preset its environment maps for its database, if any | the name of a `[startup.<name>]` table, or `"none"` | The Startup SQL preset its query sessions start with; `"none"`: none, whatever its environment maps. A preset that is not there, or one for the other database, is an error of the profile. |
| `startup_sql` | `[]` | a list of statements | The profile's own session settings, run after the preset's. Each must be one setting of the session; they are checked when the profile connects. |
| `session` |  |  | Old name of `startup`, read the same; a table may not have both. |
| `session_sql` |  |  | Old name of `startup_sql`, read the same; a table may not have both. |

## `[connections.ssh]`

The SSH tunnel of the profile above it only (a preset several profiles share is a `[tunnels.<name>]` table). One bastion, never read from `~/.ssh/config`. The table stays when the tunnel is turned off.

| Key | Default | Values | Description |
|---|---|---|---|
| `enabled` | `false` | `true`, `false` | Whether the profile connects through this tunnel. |
| `host` | none (required when enabled) | a host name or address | The bastion's host. |
| `port` | `22` | a port number (`0` is refused when enabled) | The bastion's SSH port. |
| `user` | none (required when enabled) | text | The user on the bastion. |
| `auth` | `"key"` | `"key"`, `"password"`, `"agent"`, `"keyboard-interactive"` | How the tunnel logs in: a private key file (OpenSSH or PEM, maybe with a passphrase), a password, the keys of the ssh-agent, or the bastion's own questions (a one-time code). |
| `key_file` | none (required with `auth = "key"`) | a file path (`~` is the home directory) | The private key file. |
| `secret_source` | `"keychain"` | as `password_source` | Where the key's passphrase or the password comes from. It is kept apart from the database password. |
| `secret_command` | none | a command | With `secret_source = "command"` (required then). |
| `secret_env` | none | an environment variable's name | With `secret_source = "env"` (required then). |
| `keepalive` | `15` | seconds; `0` for none | Seconds between keepalives. |
| `timeout` | `10` | seconds (`0` counts as 1) | Seconds each step of opening the tunnel may take. |

## `[policy.<env>]`

An environment that profiles name with `policy = "<env>"`. Every key is optional; a key left out has its built-in default (not the one of `[policy.default]`). `[policy.default]` is not an environment: it holds the rules of every profile without one (its `color` and `badge` are not shown anywhere). A profile naming an environment that is not defined does not connect.

| Key | Default | Values | Description |
|---|---|---|---|
| `paging` | `"no_hold"` | `"no_hold"`, `"hold"` | What a result with more rows than a page keeps on the server, outside your own `BEGIN` block. `"no_hold"` keeps nothing: the next page runs the statement again (only a plain `SELECT`; any other statement shows its first page only). `"hold"` keeps the cursor, its transaction and its locks open until the result is read to its end or closed after `paging_idle_timeout`. |
| `paging_idle_timeout` | `"30s"` | seconds as a number, or `"90s"`, `"5m"`, `"1h"`; `0` or `"off"` never closes it | With `paging = "hold"`: how long a result that has more rows may keep its cursor open without a fetch. |
| `spill_limit` | the top-level `spill_limit` | as the top-level `spill_limit` | The most a result's temporary file may take for its profiles. |
| `read_only` | `false` | `true`, `false` | Run only reads, transaction control and safe session settings, and refuse the rest before sending anything; the server also makes every transaction of the query sessions read-only. |
| `confirm` | `"destructive"` | `"destructive"`, `"writes"` | Which statements ask before they run: destructive ones (`DROP`, `TRUNCATE`, an `UPDATE` or `DELETE` of every row, …), or also every other statement that may write. Dangerous statements always ask. |
| `commit` | `"auto"` | `"auto"`, `"manual"` | `"auto"`: each statement outside your own `BEGIN` block commits by itself. `"manual"`: the first statement that may write opens a transaction that stays open until you commit or roll it back. A driver that cannot runs it as `"auto"`. |
| `tx_idle_warning` | `"5m"` | a duration as `paging_idle_timeout` takes, at least 10 seconds, or `0` or `"off"` | How long an open transaction may sit idle before the app warns that it holds locks. |
| `tx_idle_rollback` | `"30m"` | a duration as `paging_idle_timeout` takes, at least 10 seconds, or `0` or `"off"` | How long a transaction opened for manual commit may sit idle before the app rolls it back. PostgreSQL's server also gets the limit, a little later, so it ends there when the app is gone. |
| `tx_idle_timeout` |  |  | Old name of `tx_idle_rollback`, read the same; a table may not have both. |
| `idle_disconnect` | the top-level `idle_disconnect` | as the top-level `idle_disconnect` | How long its profiles' connections may sit unused before the app closes their sessions. |
| `lock_wait` | the top-level `lock_wait` | as the top-level `lock_wait` | How long a statement of its profiles' query sessions waits for another session's lock. A Startup SQL statement that sets the server's own limit overrides it for those sessions. |
| `startup` | none | `{ postgres = "<name>", mysql = "<name>" }`, either left out | The Startup SQL preset its profiles' query sessions start with, for each database. A profile may name its own, or `"none"`. |
| `session` |  |  | Old name of `startup`, read the same; a table may not have both. |
| `color` | none | as a profile's `color` | The environment's color: its profiles' names take it in the explorer, the tab list and the quick switcher, and its tabs' titles in the tab bar. |
| `badge` | none | text of at most 8 columns, no blanks around it | A short text its profiles' tabs and the status bar show (`"PROD"`). |

## `[tunnels.<name>]`

An SSH tunnel preset that profiles name with `tunnel = "<name>"`: the keys of `[connections.ssh]` without `enabled` (a preset is always on), and an `id`.

| Key | Default | Values | Description |
|---|---|---|---|
| `id` | a new one, written by datarig | a UUID | The preset's stable id: its secret is kept under it, so a rename keeps it. |
| `host` | none (required) | a host name or address | The bastion's host. |
| `port` | `22` | 1 to 65535 | The bastion's SSH port. |
| `user` | none (required) | text | The user on the bastion. |
| `auth` | `"key"` | `"key"`, `"password"`, `"agent"`, `"keyboard-interactive"` | As in `[connections.ssh]`. |
| `key_file` | none (required with `auth = "key"`) | a file path (`~` is the home directory) | The private key file. |
| `secret_source` | `"keychain"` | as `password_source` | Where the key's passphrase or the password comes from. |
| `secret_command` | none | a command | With `secret_source = "command"` (required then). |
| `secret_env` | none | an environment variable's name | With `secret_source = "env"` (required then). |
| `keepalive` | `15` | seconds; `0` for none | Seconds between keepalives. |
| `timeout` | `10` | seconds (`0` counts as 1) | Seconds each step of opening the tunnel may take. |

## `[startup.<name>]`

A Startup SQL preset: settings a query session of one kind of database starts with. Profiles name it with `startup = "<name>"`, environments with `startup = { postgres = "<name>", mysql = "<name>" }`. The name `none` is reserved.

| Key | Default | Values | Description |
|---|---|---|---|
| `dbms` | none (required) | `"postgres"`, `"mysql"` | The kind of database the preset is for. |
| `sql` | `[]` | a list of statements | The statements a query session starts with, in order. Each must be one setting of the session (in a read-only environment one a read-only session may change); they are checked when a profile connects. |

## `[sessions.<name>]`

The old name of `[startup.<name>]`, read the same; a file may not have both.

## `[keymap.<context>]`

Key remapping for one context (see Keybindings for the context names and action ids). Invalid entries are skipped and reported at startup.

| Key | Default | Values | Description |
|---|---|---|---|
| `"<keys>"` | the default bindings | an action id, or `"none"` to unbind | Binds the keys (`"ctrl+e"`, `"space c n"`: a space separates the keys of a sequence) in that context. |
