# datarig

[![CI](https://github.com/DokaDev/datarig/actions/workflows/ci.yml/badge.svg)](https://github.com/DokaDev/datarig/actions/workflows/ci.yml)

A terminal database client: a DataGrip-style workspace for your databases, in the terminal.

datarig is written in Rust with [Ratatui](https://ratatui.rs). It keeps several connections
open side by side, each with its own tabs, a schema explorer, a SQL editor with vim keys and a
results grid that pages through large results explicitly instead of loading them whole. It is
careful by default: dangerous statements ask first, and read-only profiles are enforced by the
server.

## Status

**Early, pre-1.0.** Releases have binaries for macOS, Linux and Windows and a Homebrew
formula ([Install](#install)); the latest is 0.3.0. Only **PostgreSQL** is implemented today. The configuration format, key bindings and behavior may still change
between releases (see [Versioning](#versioning)).

Platforms: developed and tested on macOS; CI runs the full test suite on Linux. On Windows CI
builds datarig, lints it and runs the tests that need no database, but running datarig there
is untested: the Windows notes below are how it is meant to work, not something anyone has
tried yet.

## Features

Everything below works today, with PostgreSQL.

**Workspace**
- Several connections at once, each with its own metadata session, schema tree and completion
  catalog. A tab belongs to one connection; tabs of different connections sit side by side.
  A tab's number shows its connection: in the profile's color while connected, muted while not,
  and a spinner in its place while a statement runs. `◆` after the name marks your open
  transaction, `!` an aborted transaction or a lost connection.
- The explorer lists every connection profile, the server's databases, their schemas, tables and
  views (materialized views with an icon of their own). Each table and materialized view shows
  the server's estimates of its rows and its size on disk on its own line, dim and on the right
  (`orders   ~50k rows · 19 MB`), read with the schema's list in the same round trip from the
  statistics that `VACUUM` and `ANALYZE` keep, without opening it and without a lock on any
  table (`r` lists them again); a table that has grown to more than twice the rows its last
  `ANALYZE` saw shows the live rows the server has counted since, its size grown with them. The name always wins: on a short line the size goes first, then
  the rows; a table with no statistics yet shows nothing there, and the status bar says "rows and
  size unknown (no statistics yet)". An open table shows its structure, read
  from the catalog in one round trip the first time it opens (`r` reads it again, and its
  estimates with it; the table itself is never read): Columns (key
  marks, type, `not null`, default), Primary Key, Foreign Keys (`Enter` goes to the referenced
  table), Indexes (columns or expressions with their order, operator class and collation, as in
  `(created DESC)`, `UNIQUE`, method, partial predicate, whether a key or constraint owns it),
  Unique and Check Constraints and Triggers (timing, events with the columns of `UPDATE OF`,
  row or statement, function, disabled; a trigger with a `WHEN` condition opens to it). Each key, index and check (one that
  reads columns) opens to `Columns (n)`, which lists the columns it covers in its order, each as
  the Columns group shows it: an index key with its order, operator class and collation, an
  expression key as its text, the `INCLUDE` columns last and marked, a foreign key's columns
  with the column each references (`user_id → users.id`). Views show their columns and
  triggers, materialized views their columns and indexes. The groups start closed, one line
  each (`Columns (27)`, `Primary Key`, `Foreign Keys (5)`); what you open in a table stays open
  when it closes and opens again, for the session. A group with nothing in it is dim,
  without a count. The explorer is narrow, so the status bar shows the whole line under the
  cursor (a line too long even for the status bar is cut in its middle, so its end stays: what to
  do, what a trigger calls). A
  lookup never waits on another session: a table another session locks (an `ALTER TABLE`, a
  `VACUUM FULL`, a migration waiting for its lock) says "structure unavailable: the table is
  locked by another session (try again)" without asking for a lock that would queue behind it:
  at once when the lock is already there, after at most 2 s when it is taken while the
  structure is being read.
  Nerd Font icons are optional (asked once, `:set icons=on|off`).
- Connection profiles with colors, icons and nested folders; a quick-connect list (`Ctrl+O`)
  with fuzzy search; a database and schema per tab (`:use db.schema`).
- Saved queries as plain `.sql` files in folders, autosave of every tab, and the workspace
  (tabs, cursors, open folders) restored at the next start. Closed consoles go to a trash
  (`:recover`). A second instance opens read-only instead of fighting over the files.
- English and Korean UI (`:set language=en|ko|auto`).
- Themes: by default the terminal's own colors; built-in truecolor themes and your own theme
  files ([Themes](#themes)).

**Passwords and connections**
- Password sources per profile: the OS keychain (every call off the UI thread, with a time
  limit, so a locked or remote keychain cannot freeze the app), a `0600` secrets file, a command
  (`password_command`, for example a password manager's CLI), an environment variable, or a
  prompt on every connect. Plaintext passwords found in the config file are moved to the
  keychain at launch.
- Connection URLs (`postgres://user@host:port/db?...`) can be pasted into the profile form.
- **SSH tunnels** through one bastion per profile: key files (OpenSSH, PEM including AWS `.pem`,
  PKCS#8, encrypted or not, with certificates), password, ssh-agent, or keyboard-interactive
  (one-time codes). Host keys are checked against `~/.ssh/known_hosts` (read only; hashed
  names, wildcards, `@revoked`) and datarig's own `known_hosts`; an unknown or changed key is
  always asked about, never accepted silently. Every session, cancel request and test
  connection of the profile goes through the tunnel.
- **Tunnel presets**: named SSH tunnels in the explorer's **Tunnels** section (make, edit, copy,
  delete, test), which profiles pick in their form. Profiles that use the same preset at the
  same time share one SSH connection: one login, one host key question, one secret prompt; it
  closes when the last of them disconnects. A tunnel of a profile's own still works, and its
  form can save it as a preset (see [SSH tunnels](#ssh-tunnels)).
- **PgBouncer and other transaction poolers**: nothing outside the query session relies on
  prepared statements, statement names are unique per process, and a profile can turn the
  server-side statement cache off (`statement_cache = false`). When the server keeps losing
  prepared statements, the session turns the cache off by itself and says so.
- A test connection from the profile form, with the server version and the latency (and the
  tunnel's time).

**SQL editor**
- Vim keys: Normal, Insert and Visual modes, with counts. The motions `h` `j` `k` `l`, `w` `b` `e`,
  `W` `B` `E`, `ge` `gE`, `0` `^` `$`, `gg` `G`, `f` `F` `t` `T` `;` `,`, `%` (brackets in
  strings and comments do not count), `{` `}`, `H` `M` `L`; the operators `d` `c` `y`, `gu` `gU`
  `g~` and `>` `<` with any motion or text object (`iw` `aw` `iW` `aW`, `i(` `a(`, `i[`, `i{`,
  `i<`, `i"` `i'` `` i` ``, `ip` `ap`, inner and around), doubled for lines (`dd` `cc` `yy`
  `>>`); `x` `X` `s` `S` `D` `C` `Y`, `p` `P`, `r`, `J` `gJ`, `~`, `u` and `Ctrl+R`; `.` repeats
  the last change, what was typed included; `i` `a` `I` `A` `o` `O`; `Ctrl+D` `Ctrl+U` `Ctrl+F`
  `Ctrl+B` and `zz` `zt` `zb` scroll; Visual mode by character (`v`) or by line (`V`), where `p`
  and `P` put a register in place of the selection, and by block (`Ctrl+V`): screen columns, so
  wide characters (Hangul, CJK) line up, `$` to each line's end, `o` `O`, `y` `d` `c` `D` `C`,
  `I` and `A` (what you type goes on every line of the block), `r` `~` `u` `U` `>` `<` `J` and
  `p` `P`, as Vim does them; `.` repeats a block operator on a block of the same size. One
  command is one undo step. The status bar
  says `i` starts typing and `Esc` stops. Insert mode keeps the usual editing keys (arrows,
  `Home`/`End`, `Backspace`/`Delete`), plus `Ctrl+W`, `Ctrl+U` and `Ctrl+R {register}`, and
  `Enter` keeps the line's indent; a paste from the terminal goes in at the cursor in every
  mode. Registers work as in Vim (`"a`-`"z` and `"A`-`"Z` to append, `"0`, the `"1`-`"9` delete
  ring, `"-`, `"_`, `".`, `"/`, `"+`/`"*`; `":` `"%` `"#` are not kept), and like Vim with
  `clipboard=unnamedplus` every yank, delete or change without a register also goes to the
  system clipboard (or through OSC 52 over SSH; `[editor] clipboard = "off"` turns that off).
  `"+p` reads the system clipboard only when you type it; over SSH use the terminal's paste.
  Search with `/` and `?` (a prompt on the editor's last line; the cursor shows the match as
  you type), `n` `N`, `*` `#` and `g*` `g#`, with counts and after an operator (`d/from`);
  matches on screen stay highlighted until `:noh`. Search patterns are
  [Rust regular expressions](https://docs.rs/regex/latest/regex/#syntax), not Vim's: case
  sensitive unless the pattern starts with `(?i)`, `\b` for a word boundary, and a match never
  spans two lines. Marks: `m{a-z}`, `'a` and `` `a `` (also after an operator, `d'a`), `''`
  back to where the last jump left from, `'<` `'>` for the last Visual selection and `'.` for the
  last change; marks follow their lines through edits and come back on undo. `gc` comments lines
  out with `-- ` or back in as Neovim does (`gcc`, `gc{motion}`, `gc` in Visual mode; also
  `Space e c`).
  On the `:` line the editor takes a line number (`:12`, `:$`, `:'a`) and `:s`
  (`:%s/old/new/g`, `:'<,'>s/^/-- /`, `:&&`, and `&` / `g&` in Normal mode): the pattern is a
  Rust regular expression as in search, the replacement Vim's (`&`, `\1`, `\r`, `\u`, `~`; `$`
  is plain text), flags `g` `i` `I` `e` (`c`, confirm, is not supported). One `:s` is one undo
  step.
  Macros are not there yet (their keys are reserved). Commands keep working with a
  Korean (2-Set) input source: the jamo are read as the QWERTY keys they sit on, and the
  character `f`, `t` or `r` waits for is taken as typed. The supported keys are listed in
  [docs/keybindings.md](docs/keybindings.md).
- Syntax highlighting, completion of schemas, tables and columns (aliases included), and the
  statement under the cursor marked in the gutter. Completion inserts names as SQL: a name that
  is not a plain lower-case word (`MixedCase`, `Order Lines`) or is a reserved word goes in
  quoted, also when you started it with `"`. The statement's `WITH` queries are offered as
  tables, with their columns when the query lists them or names them in its select list. Names
  that start with what you typed come first, then names that contain its letters in order
  (`oi` finds `order_items`); keywords only by their start.
- A formatter you run yourself (`Space e f`, `:format`, `:'<,'>format`; never automatic): it
  lays out the statement under the cursor or the selection, as one undo step. It changes only
  the layout (line breaks, indent of 4 or 2 spaces, and keyword case if you ask for it with
  `[editor] format_keyword_case = "upper"` or `"lower"`); names, strings, comments and
  dollar-quoted bodies stay exactly as written. Every result is checked token by token against
  the original, and when anything else would change (operator characters split, strings
  joined across a line break, `U&'...'`, psql meta-commands) it refuses, says on which line, and
  leaves the text alone.
- Run the statement under the cursor (`Ctrl+E`), a selection, or several statements in a row:
  each statement's outcome is listed in a Messages tab, and every row result gets a result tab
  of its own. Queries can be cancelled (`Ctrl+C`).
- `Ctrl+G` opens the query in your own editor (`$VISUAL`, else `$EDITOR`, else `vi`; for
  example `EDITOR="code -w"`), run directly without a shell on a private temporary file; what you
  save replaces the text as one undo step, and quitting without saving (or Vim's `:cq`) changes
  nothing. A table tab's query opens as a copy that comes back in a new console. `Ctrl+Z` (or
  `:suspend`) suspends datarig like Vim, and `fg` brings it back (not on Windows). A running
  query keeps running on the server meanwhile.

**Results**
- A grid that shows one page at a time (`n`/`p`), fetched from a server-side portal; the rows
  already fetched stay in a bounded memory window and spill to a private temporary file past it.
  An idle portal is closed after a policy's timeout, and a closed result can be re-run from the
  page you were on when the statement is safe to repeat.
- A row count on demand, with a label that says whether it matches the pages you saw.
- An inspector for the selected cell or row (JSON pretty-printed), a cell viewer, and cell,
  row, column and range selection with the keyboard or the mouse.
- Copy as TSV (with or without headers), CSV, JSON, indented JSON, Markdown, HTML, XML, an SQL
  `IN` list, or SQL `INSERT`/`UPDATE` statements (only when the target table is certain), to the
  system clipboard or through OSC 52 over SSH.
- Transaction indicators in the tab bar and the status bar (open, aborted, "rollback required"),
  and questions before closing, quitting or disconnecting would roll back your work.

**Safety**
- Statements are classified with PostgreSQL's own parser (libpg_query), not with a regular
  expression: `DROP`, `TRUNCATE`, `UPDATE`/`DELETE` without a real `WHERE`, `ALTER ... TYPE`,
  `DO`/`CALL`, `COPY` to or from server files or programs, server-side built-ins that act on
  files or backends, and text the parser rejects all ask before they run.
- Safety policies per profile (`[policy.<name>]`): `read_only`, which statements need a
  confirmation (`confirm = "destructive"` or `"writes"`), the idle portal timeout and the spill
  limit. **Read-only is enforced by the server**: every transaction of a read-only profile is
  opened `READ ONLY`, so even a function called from a `SELECT` cannot write.
- `EXPLAIN ANALYZE` of a write is rolled back.

**Keys**
- A context keymap with a which-key popup after `Space`, keyboard help (`F1` or `Space ?`) with
  search, a `:` command line (`Ctrl+K` everywhere) that runs commands and finds every action by
  name, and remapping in the config file. See [docs/keybindings.md](docs/keybindings.md).

**Performance**, held by budgets that CI enforces (`crates/datarig-bench/budgets.toml`,
[docs/perf.md](docs/perf.md)):
- One round trip for the first page of a prepared `SELECT`, one for each later page and one
  for an `INSERT`, counted exactly through a latency proxy, also through an SSH tunnel.
- Paging through all 4,000,000 rows of the test table stays under 96 MiB of resident memory,
  growing less than 16 MiB after the in-memory window is full.
- Keystroke to frame under 25 ms (p95) in a 5 MB SQL file.
- Idle: under 48 MiB and 1% CPU, and fewer than 0.2 wakeups a second when nothing is waiting.
- First frame under 250 ms (p95); the release binary is under 16 MiB.

## Not yet (roadmap)

Planned, in no particular order and with no dates:

- Drivers for MySQL/MariaDB, Valkey/Redis and Elasticsearch
- TLS connections (today every connection is plain TCP; use an SSH tunnel across untrusted
  networks, and servers that require TLS cannot be reached yet)
- A server monitor (sessions, locks, activity)
- More of vim: `gv`, macros, Ex commands other than `:{n}` and `:s` (`:d`, `:g`, …)
- DDL view of tables and other objects
- Query profiling and charts
- Multi-hop SSH and importing hosts from `~/.ssh/config`

## Install

- **Homebrew** (macOS and Linux, arm64 and x86_64), in one line:

  ```sh
  brew install dokadev/tap/datarig
  ```

  or add the tap once and use the short name from then on:

  ```sh
  brew tap dokadev/tap
  brew install datarig
  brew upgrade datarig   # later, for a new release
  ```

  datarig may be submitted to homebrew-core later, so that `brew install datarig` works
  without the tap. The tap installs stable releases only.
- **GitHub Releases**: download the archive for your platform from
  [Releases](https://github.com/DokaDev/datarig/releases) and put the `datarig` binary on your
  `PATH`. There are archives for macOS and Linux (arm64 and x86_64) and Windows (x86_64,
  untested); `SHA256SUMS` lists their checksums. Each archive also has the licenses and the
  third-party notices.

### From source

datarig builds from source with the Rust toolchain. You need:

- Rust **1.93** or newer (`rust-toolchain.toml` pins 1.93.0; rustup installs it
  automatically).
- A C compiler and **libclang**, for the `pg_query` crate (it compiles PostgreSQL's parser and
  generates bindings with bindgen). On macOS the Xcode Command Line Tools have both; on
  Debian/Ubuntu `apt install clang libclang-dev`; on Windows (untested outside CI) install
  LLVM and set `LIBCLANG_PATH` if bindgen does not find it.

```sh
git clone https://github.com/DokaDev/datarig
cd datarig
cargo install --locked --path crates/datarig-tui   # installs the `datarig` binary
# or: cargo build --release   (the binary is target/release/datarig)
```

A terminal of at least 80×24 is required. A Nerd Font is optional (for the icons).

## Versioning

Releases follow [Semantic Versioning](https://semver.org), as `0.MINOR.PATCH` until 1.0. A
patch release only fixes things. A minor release before 1.0 may change the configuration
formats: the release notes say so, and datarig migrates the files automatically, keeping a
backup of the old ones. A release candidate (`vX.Y.0-rc.N`), when there is one, is a GitHub
pre-release only; the Homebrew tap installs stable releases. 1.0 comes with several databases supported and a stable configuration.

## Quick start

```sh
datarig                # open the workspace; the explorer offers "＋ New connection"
datarig my-profile     # connect to a profile and open a console
datarig --config path/to/config.toml
```

In the workspace, press `Space` and wait to see what follows, or `F1` for the keyboard help.
`Ctrl+O` picks a connection, `Ctrl+E` runs the statement under the cursor, `Ctrl+C` cancels,
`:q` closes a tab and `:qa` quits.

To try it against a local test database, start the development PostgreSQL from `dev/` (see
[Development](#development)) and add a profile with host `127.0.0.1`, port `55432`, user,
password and database `datarig`, or paste `postgres://datarig@127.0.0.1:55432/datarig` into
the new-connection form.

## Configuration

The config file is `--config <path>`, else `$XDG_CONFIG_HOME/datarig/config.toml`, else
`~/.config/datarig/config.toml` (on every OS). The app creates and edits it (comments and key
order are kept); you can also write it by hand. Passwords are never written to it.

```toml
version = 2
language = "en"            # en | ko | auto
theme = "catppuccin"       # see Themes below; terminal when left out

[[connections]]
name = "local"
driver = "postgres"
host = "127.0.0.1"
port = 5432
user = "me"
database = "app"
color = "green"
folder = "dev"
password_source = "keychain"   # keychain | file | command | env | prompt

[[connections]]
name = "prod-replica"
driver = "postgres"
host = "db.internal"
port = 5432
user = "readonly"
database = "app"
color = "red"
folder = "work/prod"
policy = "prod"
password_source = "command"
password_command = "op read op://work/prod-db/password"
tunnel = "office"              # the tunnel preset below

[[connections]]
name = "lab"
driver = "postgres"
host = "10.0.3.7"
user = "me"
database = "lab"

[connections.ssh]              # a tunnel of this profile only
enabled = true
host = "lab-bastion.example.com"
user = "me"
auth = "agent"                 # key | password | agent | keyboard-interactive

[tunnels.office]               # profiles that name it share one SSH connection
host = "bastion.example.com"
user = "ec2-user"
auth = "key"
key_file = "~/.ssh/prod.pem"

[policy.prod]
read_only = true
confirm = "writes"
paging_idle_timeout = "10s"
```

Data (saved queries, datarig's own `known_hosts`) and state (tabs, console buffers) live in the
platform's data and state directories (`~/.local/share/datarig` and `~/.local/state/datarig`
on Linux, `~/Library/Application Support/datarig` on macOS; on Windows, untested,
`%APPDATA%\datarig` and `%LOCALAPPDATA%\datarig`), or under `$XDG_DATA_HOME` / `$XDG_STATE_HOME` when set.
`DATARIG_SECRET_STORE=memory` keeps passwords in memory only and never touches the OS keychain.
An `[editor] mode` key from an older version is ignored (the editor has vim keys only) and
dropped the next time the app saves the file.

## Themes

One setting, `theme` (`:set theme=<name>`, or the settings screen, `Space ,`, where moving over
the names previews them: `Enter` keeps one, `Esc` goes back):

| `theme` | What it does |
|---|---|
| `terminal` | The default: your terminal's own colors and background (its 16 ANSI colors by role) |
| `dark`, `light`, `high-contrast` | Truecolor themes of datarig |
| `catppuccin`, `tokyo-night`, `gruvbox` | The light or the dark variant, following the terminal's background |
| `catppuccin-latte`, `catppuccin-mocha`, `tokyo-night-day`, `tokyo-night-night`, `gruvbox-light`, `gruvbox-dark` | That variant, whatever the background |
| `nord`, `dracula` | Those palettes (dark) |
| any other name | Your theme file `themes/<name>.toml`, next to the config file |

The terminal's background is asked once at startup (OSC 11); a terminal that does not answer
counts as dark, and the settings screen says which it found. A theme file sets some tokens and
takes the rest from `extends` (a built-in theme; `terminal` without it). Colors are `#rrggbb`,
an ANSI name (`red`, `bright-black`, …) or `default`:

```toml
# ~/.config/datarig/themes/mine.toml, used with theme = "mine"
extends = "dark"

[colors]
accent = "#89b4fa"
warning = "yellow"

[styles]
selection = { bg = "#45475a", modifiers = ["bold"] }
```

The token names are the fields of `Theme` in `crates/datarig-tui/src/theme.rs`. A mistake in the
file (an unknown token, a bad color), an unknown name or a file named like a built-in theme is
reported with the file and line, and the app draws with `terminal` until it is fixed.

## Safety model

- **What asks**: every statement is parsed with PostgreSQL's parser before it is sent. A
  destructive or unknown statement opens a confirmation with Cancel focused; a policy with
  `confirm = "writes"` asks for every write.
- **What is refused**: a read-only profile refuses anything that is not a read, transaction
  control or a safe session setting before sending it, and the server rejects the rest, because
  every transaction is opened `READ ONLY`.
- **What it is not**: the confirmation is a guardrail against mistakes in what the text shows.
  Functions called from a query, triggers, views and rules are not inspected; a read-only
  policy is the guarantee.
- Results that page keep an implicit transaction open on the server; it is closed after the
  policy's idle timeout (30 s by default) so it does not hold back vacuum.

## SSH tunnels

Each profile can reach its database through one SSH bastion. datarig opens the tunnel itself
(pure Rust, no `ssh` binary) and dials every connection of the profile through `direct-tcpip`
channels; nothing else is requested from the server. The key file must not be readable by
others, as OpenSSH requires. The key's passphrase or the SSH password uses the same sources as
database passwords. Keepalives detect a dead tunnel, and the explorer marks the profile until
the next use reconnects.

The profile form's SSH section picks the tunnel: **off**, a **tunnel preset**, or **this profile
only** (its own bastion fields, `[connections.ssh]`).

- **Tunnel presets** (`[tunnels.<name>]`, the profile names one with `tunnel = "<name>"`) live in
  the explorer's **Tunnels** section, below the connections: `n` makes one, `e` edits it (its name
  too: the profiles that name it follow), `c` copies it, `d` deletes it, `t` tests it, and `Enter`
  shows the profiles that name it. Each preset shows whether its connection is closed, opening,
  open (and how many profiles are on it) or lost (and why).
- Profiles that use the same preset at the same time share **one SSH connection**: the first one
  opens it (one login, one host key question, one secret prompt, for every profile waiting), the
  others take it as it is, and it closes when the last of them disconnects. When it is lost,
  every profile on it says so, and each connects again on its next use.
- A preset's secret (the key's passphrase or the password) is kept once, under the preset's own
  keychain or secrets-file account, which a rename keeps. Deleting a preset asks first, lists the
  profiles that use it and removes its saved secret.
- A preset changed while profiles are connected through it: they keep the connection they have
  until they connect again; the next connect uses the new settings.
- A profile whose preset does not exist (or that names a preset and has its own tunnel turned on)
  does not connect and says why; it **never** connects directly instead.
- "Save as tunnel preset" (`Ctrl+B` in the form's own tunnel) makes a preset of the profile's own
  tunnel when the form is saved: the settings move to the preset and the profile names it; its
  stored secret is copied to the preset (and read back) before the old copy is removed. A secret
  that cannot be read stays where it was and is asked for at the next connect.
- Testing a preset opens its SSH connection and then one `direct-tcpip` channel to each database
  address of the profiles that use it (opened and closed again; nothing is sent to a database).
- The app never rewrites the config on its own: presets change only when you save a form, rename
  or delete one.

## Development

The workspace layout:

| Path | What |
|---|---|
| `crates/datarig-core` | UI-free core: driver interface, SQL tooling and the safety classifier, profiles, secrets, config, i18n |
| `crates/datarig-driver-postgres` | the PostgreSQL driver |
| `crates/datarig-ssh` | SSH tunnels |
| `crates/datarig-tui` | the terminal UI and the `datarig` binary |
| `crates/datarig-bench` | benchmarks and the CI performance budgets |
| `vendor/tokio-postgres` | a patched tokio-postgres (per-column result formats, pipelining); see [vendor/README.md](vendor/README.md) |
| `locales` | the UI text catalogs (`en.toml`, `ko.toml`) |
| `dev` | Docker Compose with PostgreSQL, MySQL, PgBouncer and an SSH bastion for the tests |
| `docs` | [architecture](docs/architecture.md), [key bindings](docs/keybindings.md), [performance](docs/perf.md) |

Unit and flow tests need nothing else:

```sh
cargo test --workspace
```

The integration tests need the services of `dev/`; without their environment variables they
print `SKIPPED`:

```sh
cd dev
docker compose up -d
docker compose --profile pooler up -d pgbouncer
sh ssh/make-fixture.sh                       # prints the fixture directory
docker compose --profile ssh up -d --build ssh-bastion
cd ..

export DATARIG_TEST_PG_URL=postgres://datarig:datarig@127.0.0.1:55432/datarig
export DATARIG_TEST_POOLER_URL=postgres://datarig:datarig@127.0.0.1:56432/datarig
export DATARIG_TEST_SSH_BASTION=127.0.0.1:52222
export DATARIG_SSH_FIXTURE=$PWD/dev/ssh/.fixture
export DATARIG_REQUIRE_PG=1 DATARIG_REQUIRE_POOLER=1 DATARIG_REQUIRE_SSH=1   # fail instead of skipping
cargo test --workspace

# the driver's suite again with every connection going through a dialer, as through a tunnel
DATARIG_TEST_DIAL=tcp cargo test -p datarig-driver-postgres --test integration_pg
```

The performance budgets (needs tmux and the database above):

```sh
cargo build --release -p datarig-tui -p datarig-bench
./target/release/datarig-bench budget
```

Before sending a change: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --
-D warnings` and the tests. Snapshot tests use [insta](https://insta.rs); after an intended
rendering change, `INSTA_UPDATE=always cargo test -p datarig-tui` rewrites the snapshots, and
the diff is part of the review. `docs/keybindings.md` is generated:
`DATARIG_BLESS=1 cargo test -p datarig-tui --test keybindings_doc`. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

datarig is dual-licensed under either of

- the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- the MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in datarig by you, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.

Third-party code: `vendor/tokio-postgres` keeps its own MIT/Apache-2.0 licenses, and the C code
built in through `pg_query` (libpg_query, PostgreSQL, protobuf-c, xxHash) has its notices in
`vendor/licenses/pg_query/`. `scripts/third-party-notices.sh` (cargo-about) writes
`THIRD-PARTY-NOTICES.html` with the licenses of every crate in the binary; CI builds it as an
artifact. `cargo deny check` keeps the dependencies to permissive licenses.
