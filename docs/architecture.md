# datarig architecture

This document describes how the Cargo workspace is put together: the crates and their
dependency rules, the state the TUI keeps, and the design decisions behind the safety model,
the PostgreSQL driver, SSH tunnels and the UI strings.

## Crates and responsibilities

| Crate | Responsibility | Depends on |
|---|---|---|
| `datarig-core` | The UI-free core. The driver interface (`Driver`, `Capabilities`, `Session`, `DbCommand`/`DbEvent`, key metadata `driver::keys::KeyCatalog`), the copy formats of result rows (`export`), SQL tooling (lexer, statement splitter, completion engine, the safety classifier `sql::risk`), the connection profile model and DSNs, password stores (`SecretStore`: OS keychain, the secrets file `secrets.toml`, memory) and per-profile password sources (`keychain`, `file`, `command`, `env`, `prompt`), the trash, reading and saving the config file (`toml_edit`), the i18n catalogs, and the transport seam (`transport`) | External crates only (tokio `sync`, serde, toml, keyring, pg_query, ...) |
| `datarig-driver-postgres` | The PostgreSQL implementation `PgDriver`: connecting (`connect`), one connection per session role: metadata (`meta`: introspection for the tree and completion, one per profile) and query (`query`: one per tab, portal paging, cancel), value decoding (`values`) | `datarig-core` + tokio-postgres (vendored) |
| `datarig-ssh` | SSH tunnels: one bastion's `direct-tcpip` channels as a `transport::Dialer` (`Tunnel`), host keys against `~/.ssh/known_hosts` (read only) and datarig's own `known_hosts`, key files (`keys`). The only crate that names russh | `datarig-core` + russh |
| `datarig-tui` | The Ratatui UI and the `datarig` binary: app state, actions and the overlay stack (`app/`), the context keymap (`keymap/`), screens (`screens/`), widgets (`widgets/`), key input and the Hangul key mapping (`input/`), the clipboard (`clipboard.rs`: the system clipboard or OSC 52), the driver registry (`drivers.rs`) | `datarig-core`, `datarig-driver-postgres`, `datarig-ssh`, ratatui/crossterm |
| `datarig-bench` | Performance measurements and the CI budgets (binary `datarig-bench`): round trips counted through a latency proxy, paging, the editor and the grid in a headless `App`, and the real binary idle and at startup in `tmux -L perf`. Not product code (see `docs/perf.md`) | All of the above |

```
datarig-tui ──▶ datarig-driver-postgres ──▶ datarig-core
     │  └──────▶ datarig-ssh ─────────────────▲
     └────────────────────────────────────────┘
```

## Dependency rules

1. **Dependencies point one way: `tui → core ← driver-*`** (and `tui → ssh → core`). Core depends on no driver crate and no UI crate (ratatui, crossterm). A driver crate depends on core only and knows no other driver.
2. **The UI does not know driver types.** `datarig-tui` talks to a driver only through `Box<dyn Driver>` and a `Session`'s command and event channels, and turns menus and panels on and off with `Capabilities`. The only place that names a concrete type (`PgDriver`) is the registry, `crates/datarig-tui/src/drivers.rs`.
3. **Keep visibility narrow.** Only what is used outside a crate is `pub`; the rest is `pub(crate)` or private. A driver crate exports its driver type only.
4. **Versions and lints live at the root.** Versions go to `[workspace.dependencies]` and lints to `[workspace.lints]`; each crate takes them with `x.workspace = true` and `[lints] workspace = true`, and turns on only the features it needs.

Rule 1 is checked mechanically:

```sh
cargo tree -p datarig-core | grep -Ei 'ratatui|crossterm|datarig-driver' && echo "rule broken" || echo OK
```

## Module rules

- Modules use the `foo.rs` + `foo/` layout (no `mod.rs`).
- Unit tests live next to the source in `foo/tests.rs`; `foo.rs` keeps only `#[cfg(test)] mod tests;`, and the tests reach private items with `use super::*`.
- Integration tests live in the `tests/` directory of the crate they check. Real-database tests that use only the driver belong to the driver crate; tests that drive the `App` (app flows, snapshots, the explorer against a real database) belong to `datarig-tui`.
- The locale catalogs (`locales/*.toml`) are read by core's `build.rs` at compile time (see "UI strings (i18n)" below). If the crate moves, fix the relative paths in `build.rs` and `tests/catalog.rs`.
- Snapshot tests pin English screens only. A screen in another language is checked against its catalog (`assert_screen!` in `crates/datarig-tui/tests/common/mod.rs`), so the only non-English UI text in the repository is in `locales/`.

## Workspace state

The TUI has one screen, the workspace; dialogs sit on top of it in the overlay stack (`app/overlay.rs`).

```
App
├─ profiles, folders, last_used   the config's profiles (by id), folder tree and expanded folders
├─ conns: ConnectionManager       ProfileId → ProfileConn (app/conn.rs)
│    ProfileConn { meta: Option<Session>, generation, connected, connecting, error,
│                  tree (schemas), catalog, resolved (profile + password of the attempt),
│                  pending: Vec<Queued> (statements waiting for it), expanded, expand_on_connect,
│                  console }
├─ tabs: TabManager (may be empty) Tab { profile: Option<ProfileId>, binding (tab generation), pristine,
│                                       editor, results, exec: TabSession }
├─ explorer: Explorer             cursor (a row, not an index), scroll, `/` filter (app/explorer.rs)
└─ overlays                       command line, quick connect, profile form, chooser, name input,
                                  context menu, password prompt, confirmation, help, which-key, …
```

- **A profile's connection is its metadata session.** Several profiles can be connected at once, each with its own session, schema tree and completion catalog. Its node state (`○ ⠋ ● ✕`) comes from `ProfileConn::state()`; a failed attempt keeps its error for the line under the node.
- **A tab belongs to one profile** and opens its own query session on its first statement, with the password its profile connected with (`resolved`). A tab without a profile (a deleted profile brought back with `Space t u`, a recovered console, a profile the config does not list) shows a banner and asks for one in the quick connect list before it runs.
- **The workspace may have no tab**: it never starts with an unbound console, and closing the last tab leaves none; the right side then shows an empty state. `TabManager::active()` is a blank stand-in while there is none (id 0, never drawn or saved; `active_mut()` hands out a fresh one each time), the actions that need a tab are unavailable (`has_tab`) and only the explorer takes the focus.
- **A statement queued for a connection runs only where it was queued.** A tab whose profile is not connected yet queues its statement in the profile's `pending` as `Queued { tab, profile, binding, statements }`, where `binding` is the tab generation (`TabManager::bind` gives the tab a new one every time it is bound). On `Connected` it runs only if the tab still exists, is still bound to that profile and still has that generation; otherwise the tab gets a warning notice and nothing runs. A queued statement is work in progress like a running one (`App::tab_busy`, `any_running`): `Space c s`, closing the tab, `x`, deleting the profile and quitting ask first and then drop it; cancel drops it; a failed attempt tells its tabs. The other deferred paths hold no statement across a rebind: tree and catalog requests name their profile and are generation-checked, paging fetches only from the session whose portal is open, and `:run` in a tab without a connection runs on the profile the user picks for it.
- **A first connect opens a console** when the profile has no tab. A tab without a connection is never bound silently.
- **Events are routed by target and generation.** `EventTarget::Meta(ProfileId)` or `EventTarget::Tab(TabId)`, each with the generation of the session that sent it; every new attempt of a profile and every new tab session gets a new generation, so a late event of a closed or replaced session is dropped. Password commands report back with the profile and generation of their attempt.
- **Opening a table runs in that table's profile's tab** (its most recent one, else a new console), never in another profile's tab.
- **The explorer's rows are rebuilt when needed** from the profiles, the folders and the connections (`App::explorer_rows`): "＋ New connection", then folders before profiles at each level by name, the error line of a failed profile and the schema tree of a connected, expanded one. The cursor stores its row, so it stays on the same node while rows appear or disappear around it.
- **One password prompt at a time**: a second profile that needs one waits in `prompt_queue`.

## Saved queries and the workspace state

- **`datarig-core::scripts`**: `ScriptStore` over `<data>/scripts/` (plain SQL files; the list is a directory scan) and the index `<data>/scripts.toml` (script path → profile id). Names are checked in `scripts::name` (portable on every OS; collisions ignore case and decomposed Hangul). A save carries the `Stamp` (mtime + size) of the file as last read or written and is refused with `SaveError::Conflict`/`Missing` when the file changed or went away since.
- **`datarig-core::workspace`**: `workspace.toml` (tabs, active tab, open folders), `consoles/<id>.sql`, the trash `consoles/.trash/<millis>-<id>.sql`, and `InstanceLock` (`<state>/lock`, an OS file lock with the owner's pid inside).
- **No automatic pruning.** The only legitimate way a console leaves the workspace is an explicit tab close, so any console file no restored tab refers to means a crash, a bug or damage. `restore_workspace` never moves or deletes one: every such file comes back as a recovered tab, a listed file that is missing or unreadable gets a notice, and an unreadable one is retried at the next launch (no tab lists it then). Guessing "complete load, so the rest is garbage" is what kept losing text; there is no such heuristic.
- **The trash is only for user closes.** `trash_console` writes the tab's current text under a name later than anything in the trash (created exclusively), then removes the console's file. `Space t u` pops the closed tabs of this run (`ClosedTab::trashed` names the file, which `untrash` moves back under a new id), then takes the newest trashed file; `:recover` lists them. `trim_trash` runs only at launch, before anything can be closed, so nothing trashed in the same operation is ever deleted; it deletes a file only when it is both outside the newest `TRASH_KEEP` (50) and older than `TRASH_DAYS` (30), ordered by the time in the name, then the name, and only names the trash made.
- **Unknown is not absent.** A directory or file that cannot be read is an error, never "not there" or "empty": `ScriptStore::find`/`place`/`folder_empty`, `orphan_consoles`, `list_trash`, `free_backup`, the v1 config backup and `Paths::first_time`. Creating a saved query uses `fsutil::create_new` (a temporary file hard-linked into place, `O_EXCL` where there are no hard links) and renames use `fsutil::rename_new`, so an existing file is never replaced even if it appears after the check.
- **In the TUI** each `Tab` has a `Doc` (`app/tabs.rs`): the console id or the script path, the text and stamp as last written, when the next autosave is due, a failed save, a conflict, and whether the profile connects once the tab gets the focus (`lazy_connect`). `app/persist.rs` does autosave (at most `AUTOSAVE` after an edit, at once on leave/close/quit), the workspace state, restore on launch and the lazy connect (`App::after_input`, run after every input event). `app/script_ops.rs` does save as, open (one tab per script), rename/move/delete and the conflict question. A second instance (`App::read_only`) writes neither `workspace.toml` nor console files.
- Every write goes through `fsutil::atomic_write` (temporary file, `fsync`, rename, directory `fsync`).
- Autosave pace: a text over `LARGE_TEXT` (1 MB) is written once typing pauses for `AUTOSAVE`, at most `AUTOSAVE_LARGE` (5 s) after its oldest unsaved edit; the workspace timer writes only `workspace.toml`. A tab written and unchanged since (its editor's text version, unique across editors) is clean without comparing the text.

## Results, copying and settings

- **Key metadata is a driver capability** (`Capabilities::key_metadata`). The metadata session reads a `KeyCatalog` (tables, columns by number, primary/foreign/unique marks) once after the catalog and again on `DbCommand::LoadKeys` (a schema refresh), as `DbEvent::Keys(Result<…>)`. Result columns carry `ColumnMeta::origin` (table id + column number, from the RowDescription; `None` for an expression). The TUI keeps the catalog per profile (`app::Keys`: unknown, loaded, failed) and looks columns up when it draws, so a warm cache costs no request per query and a late one still shows. `DbEvent::Catalog` is a `Result` too: a catalog that cannot be read is never taken for an empty one.
- **Copying** (`app/copy.rs`): the grid's range (`GridState::anchor` to the cursor), the cell, the row or every fetched row, formatted by `core::export`; the SQL INSERT target comes from `driver::keys::insert_source` (an allowlist: one plain table in `FROM` by the lexer, every column of it by the RowDescription, else refused) or, for `:copy insert <schema.table>`, `driver::keys::insert_into` (columns matched by name). `clipboard.rs` picks the way (`clipboard = auto | system | osc52`, SSH from the environment); the system clipboard is an `Opener` the binary sets (arboard; tests use a fake, so no test touches the real one), and an OSC 52 sequence waits in `App::take_terminal_output` until the binary writes it between frames.
- **Settings**: `config::Prefs` (`[commands] position`, `detail_view`, `clipboard`, `copy_header`) next to the other settings. One table, `app::command::SETTINGS` (key, values, label, description, category), drives both `:set` and the settings screen (`app/settings.rs`, a large modal overlay `overlay.settings`), so they cannot drift apart; every change goes through the same `apply_setting` and `config::save` (toml_edit).
- **Drawing**: the command line is a popup or the bottom line (`widgets/cmdline.rs`); the inspector is a panel next to the results (`widgets/inspector.rs`, `Layout::detail`); the results title carries the paging state on its right (`widgets::panel_with_status`, the title clipped first). No emoji is drawn: marks are Nerd Font glyphs (`icons.rs`) or text, checked by `tests/no_emoji.rs`.

## Performance

Held by CI budgets (`docs/perf.md`).

- **Result rows** (`datarig-core::results`): a `RowStore` per result keeps `result_window_rows` rows in memory and writes every row past that to a spill file (`results::spill`: one `0600` file per result in `<state>/spill`, a varint encoding of the decoded cells, the offset of every 256th row in memory). The window follows the pages and moves to the rows the grid shows, reading ahead; `for_each_chunk` streams rows for copies (`export::Writer` writes any format a chunk at a time with the same text as all at once). A row not in memory is `Row::NotRead`, never an empty row. The file stops at `spill_limit` (config, policy) and fetching stops with it. Files are deleted with their result and at quit; a launch deletes only the files of a process id that is not running and whose `datarig-spill-<pid>.lock` nobody holds.
- **Editor** (`widgets/editor.rs`): lines, edits as splices of the lines they touch, undo as the changes of each command. The lexer state at each line start is cached (between tokens, or inside a token that spans lines, with its start) and invalidated from an edited line on; a frame lexes the lines on screen from the nearest known state. The statement under the cursor and the completer's text come from lines around the cursor, taking more until the `;` around it are in them; this equals splitting the whole text (tested on random texts).
- **Event loop** (`main.rs`): it sleeps until `App::next_tick` (100 ms only while something counts in tenths or animates; else the earliest autosave, portal close or countdown second; none when nothing waits) and draws no frame for a mouse move.
- **Round trips**: see the vendored tokio-postgres decision below.

## Terminal, runs and copies

- **Terminal** (`datarig-tui::terminal`): what the binary sets on the terminal (alternate screen, mouse, bracketed paste, kitty keyboard flags) is recorded in one `TermState` and undone exactly once, from a guard made before the setup or from the panic hook; the restore always gives the user's cursor shape back. The cursor's shape follows the key context (`terminal::cursor_shape`: a block in vim Normal/Visual, a bar in Insert and every text input; `editor.cursor_shape = off` leaves it alone). Escape sequences go to any writer, so tests check every exit path.
- **Theme tokens**: `MODE_*` (the status bar's mode badge, text `MODE_FG`) and `CURRENT_STMT_BAR` (the gutter bar of the statement a run would take), checked in the theme tests with the WCAG contrast helper and the xterm-256 mapping like the other tokens.
- **Runs of several statements**: still one `DbCommand::Execute`; the driver reports `DbEvent::Started`/`Finished` per statement and stops between statements when the session's canceller flagged a cancel (`DbError::Cancelled`). The tab keeps the run as an ordered list of per-statement outcomes (`app::runlog::RunLog`); the last statement's rows are the tab's result.
- **Driver replies**: the query session's `Reply` answers a run by value (`page`, `done`, `fail`, `closed`), so a second terminal event does not compile; the steps of a run hand the reply back while the run goes on.
- **Copies** (`app::copy`): a scope (`CopyScope`: the selection, or every fetched row) and a format (`CopyFormat::MENU`, written by `core::export`); `Action::Copy(scope, format)` is one registry entry per pair, so keys, the command line and the menus share them. The grid's context menu has a second level (`app::menu::SubMenu`). SQL UPDATE is planned by `driver::keys::update_source` (the INSERT allowlist plus the table's whole primary key).
- **Inspector focus**: `Focus::Inspector` and the key context `inspector`; the conflict check allows exactly one shadowing there (its tab switch over `pane.next`/`pane.prev`).

## Safety

- **Classification is core's** (`datarig-core::sql::risk`, pure), **from PostgreSQL's own parse tree** (libpg_query through the `pg_query` crate; see the decision below): a statement's `Class` (read, session, tx, write, DDL, maintenance, procedural, unknown), whether it writes rows, a missing, always-true or column-less `WHERE` (`NoWhere`), why it asks (`Danger`: destructive, a `MERGE` that updates or deletes, `ALTER … TYPE` with or without `USING`, `DO`/`CALL`, an unknown prepared statement, a text the parser rejects, a text too long or too deeply nested to parse), its target, and `Explain` (plan only, or `ANALYZE` in any quoting, classified as the statement it wraps). Data-modifying statements, row locks and `SELECT … INTO` are found at any depth; `set_config()` of the read-only settings marks a statement as asking for read-write. `Risk::read_only`, `Risk::confirm` and `Risk::rolls_back` are the three questions the rest asks. A text whose plain strings hold a backslash is parsed a second time with those strings spelled as `E''` strings (what a server with `standard_conforming_strings = off` runs); the worse reading counts. It is an allowlist: unknown text is treated like a write, and text the parser rejects always asks. `risk::Prepared` keeps a session's prepared statements by name, so `EXECUTE` is classified as the statement it runs; each tab keeps one for its query session (`TabSession::prepared`, empty for a new session), changed only by what the server confirmed (see "sent ≠ succeeded" below).
- **The lexer stays for the UI** (highlighting, the statement splitter, completion) and follows PostgreSQL's scanner where tokens end (ASCII-only whitespace, `\r` ends a `--` comment, `$` and non-ASCII characters inside identifiers, dollar-tag characters). Differential tests hold the splitter to libpg_query's split on a corpus of tricky texts; a piece the splitter got wrong would fail to parse and ask.
- **The TUI checks before it sends or queues** (`app/safety.rs`): `App::run_in` refuses what a read-only policy does not allow (a notice naming the policy), then asks about the dangerous statements (`Overlay::RunConfirm`, key context `overlay.run_confirm`, Cancel focused) and only then calls `run_approved`, which queues the run for a connecting profile or sends it; `run_approved` checks read-only again, and a queued run is not asked about twice. An answer runs only on the tab, profile and binding it was asked for (like `Queued`).
- **The server enforces read-only, per transaction** (the hard guarantee): with `ConnectOptions::read_only` (set for every session of a profile whose policy is read-only then or now) the query session opens every transaction `READ ONLY` itself: a portal's own transaction starts with `BEGIN READ ONLY`, a statement without rows outside the user's block runs between `BEGIN READ ONLY` and `COMMIT` (transaction control and session settings are not wrapped), and the user's block gets `SET TRANSACTION READ ONLY` with its first statement (before an `EXPLAIN ANALYZE` savepoint too). All of it goes out in the write the statement goes in (no extra round trip; the budget has read-only scenarios), and the user's SQL is not rewritten. A statement that asks for read-write is never sent (`DbError::ReadWriteRefused`), because a block may go back to read-write before its first query. The startup option `-c default_transaction_read_only=on` stays as a second layer; a server that drops it (a pooler) no longer fails the connect: `DbEvent::ReadOnlyPerTransaction` follows `Connected` and the status bar says so. `TabSession::read_only` records it; a policy that became read-only after the session opened blocks runs there until a reconnect.
- **Sent ≠ succeeded**: state that depends on what a statement did changes only when the server confirms the statement succeeded, never when it is sent; when the outcome is unknown the state becomes unknown, and unknown counts as dangerous. For prepared statements: `App::run_approved` keeps the run's statements in `TabSession::unconfirmed`; `DbEvent::Finished` (a statement before the last) and the run's answer (`Done`, or the first `Page` of the last statement) apply them to `TabSession::prepared` (`TabSession::succeeded`); `Failed` (error or cancel, including the statements the run never reached), `Lost`, a failed reconnect and an unanswered cancel forget every name they may touch (`TabSession::unsure`, `risk::Prepared::forget`), so an `EXECUTE` of such a name asks and a read-only policy refuses it. A new body is never assumed. A statement that may run code the text does not show (`risk::Risk::runs_code`: `DO`, `CALL`, a call of a function not in `risk::BUILTINS`, which is PostgreSQL 17's `pg_catalog` less `risk::RUNS_CODE` (the built-ins that run a query or code they are given: `query_to_xml` and friends, `ts_stat`, `ts_rewrite`, `pg_input_is_valid`, `brin_summarize_range`), any write, DDL or maintenance statement, which may fire a trigger, `COMMIT`, `PREPARE TRANSACTION`, `SET CONSTRAINTS`, `FETCH`/`MOVE`, a text that is not read) forgets every name, on success and on an unknown outcome alike (a plpgsql function can re-prepare a name as a `DELETE`); a new session starts with none. Not seen, because the parse tree does not show a function call without the catalog: a view's query, row-level security, operators, casts, domain `CHECK`s and input functions of user types, a user's function in attribute notation (`t.f`, `(t).f`, `(t.*).f` are column references and indirections to the parser; a name of `risk::RUNS_CODE` in that notation is taken as a call and forgets), and an unqualified call that resolves to a user function sharing a built-in's name (an overload in a schema of `search_path`, for built-in argument types too, or a schema ahead of `pg_catalog`).
- **Never a crash in the classifier**: `Prepared::classify` runs on a thread of its own (`risk::THREAD`, stack `risk::STACK` = 256 MiB of address space, committed as used), because parsing and walking the tree recurse per level of nesting in C and Rust and a 30 000-term `1 + 1 + …` overflowed the UI thread's 8 MiB. Before parsing, a linear lexer estimate caps the input (`MAX_BYTES` 256 KiB, `MAX_DEPTH` 16 000 levels: chains count, lists and `AND`/`OR` do not, joins and set operations count whatever separates them, groups add their depth); over the caps the text is `Danger::TooComplex` without being parsed. The caps also bound the time (libpg_query's protobuf output grows with the square of the depth: about 0.1 s at the cap in release). At the cap a release build uses under 32 MiB of stack, a debug build under 96 MiB (`[profile.dev.package.pg_query] opt-level = 2`: the generated decoder's unoptimized frames are tens of KiB per level). prost's own recursion limit (100) is off (`no-recursion-limit`), since it made ~46 chained operators or ~50 joins unreadable; right-nested texts still stop at PostgreSQL's parser limit ("memory exhausted") and ask. A panic on the thread is caught (`Danger::Unparsed`); the binary's panic hook, process-wide, restores the terminal for a panic on any other thread and leaves it alone for this one, whose panic the app survives.
- **Typed refusals**: a read-only policy names why it refuses a text it could not check (`ReadOnlyBlock::Unparsed`, `ReadOnlyBlock::TooComplex`). `COPY … FROM STDIN` and `COPY … TO STDOUT` (`Risk::stdio`) are refused under any policy before anything is sent (`App::unsupported`): the driver does not carry the COPY protocol yet, and a `FROM STDIN` sent broke the connection. `COPY` with a program or a file name runs on the server's operating system and always asks (`Danger::CopyProgram`, `Danger::CopyFile`, the target being the file or program of a `COPY … TO`); a read-only policy refuses it as a write. A call of a built-in in `risk::SERVER_FILES` (server files by a path it is given: `lo_import`, `lo_export`, `pg_read_file`, `pg_ls_*dir`, `pg_stat_file`) or `risk::SERVER_ACTIONS` (beyond the transaction: backends, configuration, WAL, replication slots and origins, statistics resets, index summarizing) is `Danger::ServerFile` or `Danger::ServerAction`, naming the function, wherever it runs (not in what `EXPLAIN` only plans or a view, rule or function body stores; the parameters of a plain `EXPLAIN EXECUTE`, bare or in `CREATE TABLE … AS EXECUTE`, do run: the server evaluates them to plan, `risk::Prepared::plans`); since the server's read-only transaction does not stop them, a read-only policy refuses them (`ReadOnlyBlock::ServerFile`, `ReadOnlyBlock::ServerAction`). An integration test pins every volatile built-in of PostgreSQL 17 to one of the lists or a reviewed harmless list. The call is recognised in every form the parse tree names it (`risk::Call`): a `FuncCall` whose last name is in a list, however qualified (`datarig.pg_catalog.lo_export(…)`, which the server accepts, or a user's schema, which over-asks), and attribute notation, where each name of an `A_Indirection` and each name after the first of a `ColumnRef` counts (`('/etc/hostname'::text).pg_read_file`, `(0).pg_cancel_backend`; a real column of such a name asks too). The built-in allowlist reads `db.pg_catalog.f` like `pg_catalog.f`. **Queries given as text**: a built-in that runs a query it is given as text (`risk::query_position`: `query_to_xml`, `query_to_xmlschema`, `query_to_xml_and_xmlschema`, `ts_stat`, `ts_rewrite(tsquery, text)`) runs what that query calls, so a string constant there (also cast to `text`, by position or named `query`, in every call form of `risk::Call`) is classified as a statement of the session on the same parse thread (`risk::Prepared::query_text`: both backslash readings, the same caps per text, at most `risk::MAX_QUERY_TEXT_NESTING` texts deep and `MAX_BYTES` of query texts per classification, counted in a thread-local of the parse thread), and its risk is the call's: a harmless query stays a read; anything else (not a constant, rejected by the parser, over a cap) is `Danger::RunsQueryText`, which asks and which a read-only policy refuses (`ReadOnlyBlock::RunsQueryText`). The other names of `risk::RUNS_CODE` run no caller SQL (`cursor_to_xml` fetches a cursor whose `DECLARE` was checked; `table_to_xml` and friends read tables; `pg_input_is_valid` runs a type's input function): they only forget the prepared names. The refusal covers the calls the text shows: a server built-in called by a view, a trigger or a user's function is not seen.
- **The metadata session is read-only by its startup option only**: under a read-only policy it opens with `default_transaction_read_only = on` but, unlike the query session, does not wrap each transaction in `BEGIN READ ONLY` (that would add a round trip to every tree and catalog read). It runs only the app's own catalog queries (constant text, no user input), so there is nothing to turn the default off; behind a pooler that drops the startup option its implicit transactions are read-write, which those queries cannot use.
- **The confirm is a guardrail, not a boundary**: it catches mistakes in what the text shows. Functions called from a query are not asked about (it would ask for nearly every query), and a view's query, row-level security, operators and casts of user types, or a user function that shadows a built-in's name do not even forget the prepared names; a read-only policy is what guarantees that nothing is written.
- **`EXPLAIN ANALYZE` is the driver's**: the query session rolls back the portal's transaction instead of committing it, or, in the user's block, wraps the statement in `SAVEPOINT datarig_explain` / `ROLLBACK TO` / `RELEASE`. The UI only announces it (`TabSession::explain_rolled_back`).
- **Runs to the end**: a row-returning statement before the last of a run is fetched page by page to its end (`DbEvent::StepRows`, kept per statement in `TabSession::steps`), so it runs as psql runs it. The session reports `TxAborted` besides `TxOpen`, and reports a portal's implicit transaction only when it stays open.
- **Pending intents are bound**: a copy that waits for an answer or for rows carries a `copy::Intent` (tab, profile, binding, session generation, result id) and is dropped when any of them changes.
- **Signals** (binary, Unix): SIGTERM, SIGHUP, SIGINT and SIGQUIT end the event loop through `App::quit_on_signal` (write, close sessions, quit); the terminal guard restores the terminal as on a normal quit. Their test (`tests/signals.rs`) runs the binary under `script` in a guard that kills and reaps both on every path, a panic included.

## Layout, result tabs and pages

- **Tabs are documents** (`app::tabs`): `TabKind::{Console, Script, Table}`. A console has a number (`Doc::console_no`, the lowest free one, kept in `workspace.toml`); a table tab has its `TableRef` and its query as its (hidden) editor text, is never saved as a file and cannot switch connection (`action::query_tab`). The connection stays attached to the tab as before (profile, binding generation, session generation); the tab bar and the editor's first line only show it.
- **Panes** (`app::pane`): `Tab::pane` (`PaneLayout`: share, hidden, maximised; per tab, in `workspace.toml`) and `Tab::ran` decide whether a query tab draws its results pane (`App::results_shown`) and its editor (`App::editor_shown`); `App::focusable` / `fix_focus` keep the focus on a drawn pane after every input. `pane::results_rows` splits the height within minimums. `Layout::body` and `Layout::divider` let a drag of the results' top border resize.
- **Result tabs** (`TabSession`): the shown row result is the tab's `results`/`grid` as before, so copy, the inspector and the grid keys are unchanged; the others are parked in `TabSession::steps` (`Parked`: rows and their own `GridState`) by statement index; `ResultView` picks rows or Messages (`draw_messages`, from `RunLog` and its `notes`). A run replaces the row results only at its first row event (`replace_pending`, `drop_rows`); until then, and when it returns none, `kept_log` holds the log the rows came from, so `shown_sql` (copy's allowlist source) and `answer_index` stay right.
- **Pages** (`app::pages`, `app::paging`): `GridState::page`/`page_size` and `GridState::window` restrict the grid to a page; movement, clicks and drags stay inside it. Paging is explicit: `page_next` shows a fetched page, asks `FetchMore` (the page comes back to `want_page`), or runs the statement again past a closed portal (`DbCommand::Resume { id, sql, skip }`) only when `sql::risk::repeat::repeatable` allows it, the result's `pages::Origin` (profile, binding, session generation) is the tab's now, and the read-only and confirm checks pass; the answer is appended only when its columns match (`Resuming`). `Paging::Open { in_block }` is never idle-closed inside the user's transaction; `ClosedIdle`, `Replaced` and `Interrupted` (a fetch that failed or was cancelled; also a cancel the session never answered keeps `more`) are closed portals that keep `more`: unknown is not absent. The title's right side is built by `results_title` from the widest form to the narrowest (`widgets::title_and_status` takes a list): the arrows go first, then the portal's state, and the page number stays. The result tab strip (`draw_strip`, `strip_window`) always keeps the shown tab and Messages.
- **The allowlist has two halves** (`sql::risk::repeat`): the text's (`names`: one plain query, built-in non-volatile functions, built-in operators and types from `volatile.txt`, `operators.txt`, `types.txt`, all generated from the server and pinned by integration tests) and the server's (`check_query` of the `Names` the text shows: relations are tables, partitioned tables or materialized views without row-level security, inheritance included; no user function, operator or type shadows a name used; no column type with code of the user's). The driver sends the question right before a `Count` or a `Resume` in the same transaction (after the count's or the re-run's savepoint inside the user's block, in the same request) and answers `DbError::NotRepeatable` when the server gives a reason; nothing else is sent then. Inside the user's block a `Resume` runs under `SAVEPOINT datarig_resume`, rolled back to on a failure or a cancel and released when its portal ends (`committed(…, savepoint, …)`).
- **What a count counts**: the driver pages a statement outside the user's block in a plain `BEGIN` (`BEGIN READ ONLY` on a read-only session) sent with its first page, at the session's own isolation, and sends nothing else (a `REPEATABLE READ` + `LOCK TABLE` paging transaction was tried and reverted: locking every partition of a partitioned table exhausted the server's lock table). A count asks its transaction's isolation (`ISOLATION`, in the allowlist question's request) and answers `DbEvent::Counted { snapshot }`; the app labels it "now" (`ResultSet::counted_now`) unless it ran in the user's current block with one snapshot and every page was read in that block (`ResultSet::pages_in`).
- **Counting** (`DbCommand::Count` → `DbEvent::Counted`): `sql::risk::repeat::count_query` wraps an allowed statement; the driver checks it again, then runs it in the portal's or the user's transaction under `SAVEPOINT datarig_count` (rolled back to on failure) or on its own (`BEGIN READ ONLY … COMMIT` on a read-only session). `Running::count` marks it busy and cancellable.
- **The user's block** (`DbEvent::Block`, before the `TxOpen` of the same change) is tracked apart from `TxOpen` (which also counts a portal's own transaction): `Tab::block_began`/`block_ended` number the transactions (`tx_epoch`) and set `ResultSet::tx` (`TxMark::InTx`, `Ended`, `RolledBack`) on the results read in them; `Tab::ending_rolled_back` reads the statement that ended the block (and `tx_aborted`), and every path that ends a session marks a rollback.
- **Workspace** (`core::workspace`, version 2): console numbers, table tabs, `results = { share, hidden, maximized }`; a version 1 file reads with defaults and is copied to `workspace.toml.v1.bak` (`create_new`) before it is first written; tabs of an unknown kind are kept as TOML text (`WorkspaceState::unknown_tabs`) and written back, and the file's `active` counts them. `save` reads the file it replaces and merges back every key it does not know (`merge`: the top, `[explorer]`, each tab found again by `tab_key`, `results`); it refuses to replace a file of a later version (`Loaded::newer`), and the app then restores that file's tabs and runs workspace-read-only (`App::workspace_newer`, the banner says why).

## Contexts, the tab bar and the saved-queries tree

- **Database and schema per tab** are a driver capability (`Capabilities::contexts`). The TUI
  names a `SessionContext { database, schema }` in `ConnectOptions::context` and sends
  `DbCommand::LoadDatabases` to a metadata session; the driver decides how (PostgreSQL: a
  connection to that database; `search_path` set to that schema then `public` as a startup
  option, checked right after connecting) and reports `DbEvent::Context` / `DbEvent::Databases`.
  When a pooler dropped the option the driver never sets the path for the session
  (behind a transaction pooler it would outlive the transaction on a connection other clients
  get); the query session sends `SET LOCAL search_path …` first in every transaction it opens or
  first uses, in the same write, exactly where a read-only session sends `BEGIN READ ONLY` /
  `SET TRANSACTION READ ONLY` (`query::Env::begin`, `Env::first_in_block`; statements are parsed
  in such a transaction too, `Client::prepare_wrapped` of the vendored pipeline), and reports
  `DbEvent::ContextPerTransaction` once. The safety classifier never sees any of it.
  A chain (`COMMIT AND CHAIN`, `query::TxAfter::Chain`) is a new block that gets them
  again; the connect check takes the option as applied only when `pg_settings.source` of
  `search_path` is `client` (same query); a statement refused in the path's transaction with
  SQLSTATE `25001` or `2D000` is `DbError::NeedsNoTransaction`, which the UI words with the
  workaround; the UI warns (never blocks) about a session-level `search_path` of the user's
  (`risk::Risk::session_path`). One round trip more than elsewhere: a prepared statement without
  rows behind such a pooler (`CALL`, `PREPARE`, `EXECUTE`), whose own transaction is committed
  after it. No SQL of a driver is in the TUI. A later MySQL
  driver implements the same with `USE`.
- A tab's context is part of its binding (`TabManager::bind_in`): a switch is a rebind, so
  every piece of deferred work bound to (tab, profile, generation) is refused afterwards.
- **Another database's metadata** (its schemas for the picker, its completion catalog and its
  keys) comes from an extra metadata session per (profile, database), `ConnectionManager`'s
  `AuxMeta`, whose events come back as `EventTarget::Aux(id)`; it is opened on demand once the
  profile is connected and closed with the profile's sessions. It is also the
  explorer's view of that database (`AuxMeta::tree`, rows `RowKind::AuxNode`), and it closes
  after `AUX_IDLE` (60 s) when no tab works in that database and nothing used it
  (`App::aux_idle`, on the tick); what it read stays, and `App::ensure_aux` opens it again under
  a new id on the next use, as after a failure. Why one could not be opened goes to
  `errors.log` and to the app's notices (`App::notices`, which Messages lists after the run's
  notes), one per database, removed when it opens; a table of another database opens in a table
  tab bound to that database (`TabManager::find_table` matches the database too); `:use` of a
  name missing from a list read earlier waits for that list to be read again
  (`App::pending_use`, bound to the tab, its binding and the profile's generation) before it
  switches or refuses.
- **The explorer's databases level**: under a connected profile
  `App::push_databases` lists `RowKind::Database(id, None)` (the profile's own, its tree from
  the profile's metadata session), then `Database(id, Some(db))` for the others the server
  listed (`ProfileConn::databases`, asked once when the node opens), with
  `RowKind::DatabasesNote` / `DatabaseNote` for what is loading or cannot be read. `App::tab_keys` and the
  completion's catalog never use one database's metadata for another (table ids differ).
- `app::quick` holds the two-level picker (`QuickRow`), `app::script_tree` and
  `widgets::script_tree` the tree dialog of save as and open (contexts `overlay.script_tree` and
  `overlay.script_tree.name`), and the tab bar records what it drew (`App::tab_hits`) so a
  click hits exactly the tab on screen.

## Remote sessions and other terminals

- **The keychain is never called on the UI thread.** `datarig_core::secret::Guarded` wraps
  the keychain store (`App::set_secret_stores` wraps whatever it is given): each call runs on a
  thread of its own and gives up after `KEYCHAIN_TIMEOUT` (10 s; `App::set_keychain_timeout`
  in tests) with `KeychainFault::NoAnswer`. A call given up on is left behind (it cannot be
  stopped); while one hangs the next calls fail at once, and once it returns the store is asked
  again. On top of that `app::keychain` runs every keychain use of the UI on a worker
  (`App::keychain_job`: a blocking task, inline when headless) and handles the answer as
  `AppEvent::Keychain(KeychainDone)`: the password of a connection attempt (bound to the
  profile and its generation; `Connecting::keychain` lets `Ctrl+C` in a tab cancel it), of a
  test (its sequence), of the profile form (the profile it reads, `ProfileForm::reading`), a
  write (the form, the prompt's save after connecting), removals (the form, deleting a
  profile) and a source change (`source::switch_copy` on a worker, the config saved on the UI
  thread, then `source::switch_remove`; `ProfileForm::saving` holds the form's keys meanwhile).
  The launch-time migration and probe run off the UI thread too and end within the limit. The secrets file and the environment are still read on the UI thread (local and
  fast).
- **Unknown is not absent.** A read that did not answer or failed opens the password prompt for
  that attempt (nothing is sent to the server) and marks the keychain as not working
  (`Secrets::unavailable`; a later answer that works clears a `NoAnswer`); the prompt offers to
  save there unchecked (`App::keychain_unsure`). Only `KeychainFault::NoStore` (no keychain on
  the system) counts as "nothing stored". The message names SSH (`clipboard::in_ssh`) and, on
  macOS, `security unlock-keychain`. `DATARIG_SECRET_STORE=stall` (debug builds) is a keychain
  that never answers, for checks by hand.
- **Icons are asked for, not guessed.** `IconsSetting::Auto` means "not decided yet" and draws
  text marks (`IconsSetting::on`); the terminal's name is not read. The binary sets
  `App::set_ask_icons` when stdout is a terminal, and `App::launch` then pushes
  `Overlay::IconsAsk` (context `overlay.icons_ask`, No focused) while the config has not
  decided; the answer goes through `App::set_icons` (saved with `toml_edit`), and `auto` from
  the settings asks again.
- **Whole rows and columns in the grid.** `GridState::shape` (`widgets::grid::Shape`: cells,
  rows, columns) goes with the anchor; `GridState::selection` / `Shape::span` give the range
  every copy and the drawing read. The render records the gutter's columns and the headers'
  row (`gutter_x`, `header_y`) for the clicks; `Drag::Grid` carries the shape, and `drag_at`
  lets the wheel extend a drag.
- **The tab bar** (`widgets::tabbar`): `tabbar::state` reads a tab's session into a `State`
  (idle, connected, running, open transaction, trouble) drawn as a mark in theme colors;
  labels are `Part`s whose document part shortens first when the tabs do not fit, and the `×`
  part records a `TabHit::Close` of its own, which `tab_bar_click` sends down the `Ctrl+W`
  path (`ConfirmAction::CloseTab` keeps the tab on Enter).
- **Run keys the terminal can send.** `keymap::works` leaves `Ctrl+Enter` out of the keyboard
  help, the command line's list and the hints (`Keymap::hint_keys`) when the kitty keyboard
  protocol was not granted; `Ctrl+E` is bound wherever `Ctrl+Enter` is.

## Explorer icons and confirmations

- **Tree icons.** `icons::TREE` (a `TreeIcon` per node kind), `icons::TYPES` (a
  `TypeCategory` per kind of column type, `TypeCategory::of` reads the type's name as the
  server formats it) and `icons::STRUCTURE` (a glyph per group of a table's structure, which its
  items share) hold Material Design glyphs of Nerd Fonts v3 with their names; a test pins
  each name to its code point (checked against `glyphnames.json` of 3.5.1). The explorer's
  `node_parts` puts the icon before a node's label only with icons on, so the tree without
  icons is unchanged. `DbEvent::Objects` carries `SchemaObjects` (tables, views, and the names
  of the views that are materialized); `widgets::tree::Children::Loaded` keeps them and
  `Tree::is_materialized` picks the icon.
- **Table structure.** `driver::structure::TableStructure` (core) is one table's structure as a
  driver reads it: its kind (`RelationKind::groups` says which groups apply), a row estimate and
  a size (`None` when unknown: never 0 for "not analyzed"), columns (with `ColumnFill`: default,
  identity, generated), the primary key, foreign keys, indexes, unique and check constraints and
  triggers, each constraint, index and trigger with the server's own definition, for a DDL view
  to reuse. A driver with `Capabilities::structure` answers `DbCommand::LoadStructure` with
  `DbEvent::Structure` on its metadata session; the PostgreSQL driver builds it as one JSON
  document in a single unnamed catalog statement (`meta::structure`, one round trip, budget
  `rtt.table_structure`), never reading the table. The TUI has no SQL for it:
  `widgets::tree::Tree::structures` caches it per `(schema, table)` with what of it is open,
  `TreeAction::LoadStructure` asks for it the first time a table opens (again after a failure,
  or with `r`), another database's through its aux session, and `TreeAction::Reveal` moves the
  cursor to a foreign key's table (asking for its schema's objects first when needed). Without
  the capability an open table shows its columns from the completion catalog, as before.
- **Keychain calls in order.** `App::keychain_job` takes the keychain accounts a job touches;
  `app::keychain::KeychainQueue` gives it a place in each account's queue when it is asked for
  (on the UI thread, a short lock) and its worker waits until it is first in all of them
  (`Turn::wait`; dropping the `Turn` leaves the queues, also on a panic). A job waits only for
  earlier jobs, each within the keychain's limit. A write that succeeds for a profile deleted
  since removes the entry again.
- **Confirmations.** `App::confirm_key` maps `Enter` to `n` for every `ConfirmAction` except the
  copies (`Copy`, `FetchThenCopy`), so what would be lost is kept unless `y` is pressed.
- **The busy notice** has its own context `overlay.busy` (`q` quits; `query.cancel` from the
  root's `Ctrl+C` quits while it is on top, before the action's own condition). Quitting
  abandons the migration's blocking task; `App::try_persist` never writes while `migrating`.
- **Small ones.** `App::cancel_connect` replaces the profile's "reading the keychain" or
  "connecting" status; the icons question is not pushed while `config_broken`, and `Esc`
  closes it without an answer; a whole-column selection (`Shape::Cols`) of a result the server
  has more of is `Block::partial`; dropping a selection clears its "Selecting…" flash
  (`App::selecting_done`). The tab bar draws an open transaction as `tabbar::TX_OPEN` (`◆`).
- **The user's transaction.** `TabSession::user_tx` (`tx_open && in_block`) is the user's block: the tab
  bar's `◆`/`!` and the status bar's "TX open" read it; the driver's paging transaction
  (`TxOpen` without `Block`) is not. `TabSession::tx_at_risk` is what a rollback would lose:
  the user's block, or a paging transaction whose statement is not `repeat::repeatable`; the
  quit, close, disconnect, rebind and delete confirmations, their notices and `lost_tx` read
  it. `App::open_table` on a table tab that exists and has not run (`!ran`, nothing running
  or queued) activates it and runs its query through `run_in`.
- **The SSH key file picker** (`app::key_picker`) is the saved queries' tree dialog
  (`ScriptTree`) in `TreeMode::KeyFile`: `root` is a folder of the file system, `entries` are
  listed a folder at a time (`ScriptTree::load`, `read_dir` and a `stat` for links; no file is
  opened), `TreeRow::Up` re-roots at the parent. `form.pick_key_file` (`Ctrl+O` in
  `overlay.profile_form`) and the field's `[…]` button (`App::form_mouse`, `ProfileForm::
  key_button`) open it; a pick sets `ProfileForm::ssh_key_note` from the file's mode and name
  (`key_picker::key_note`).

## Connection poolers

- **Nothing outside the query session depends on a prepared statement.** The metadata and aux
  sessions' catalog reads, the connect-time context check and the test connection's version
  read are each one unnamed statement (`Client::query_typed`: Parse, Bind, Execute and Sync in
  one write, one round trip). Behind a pooler in transaction mode without prepared statement
  support (PgBouncer before 1.22, or `max_prepared_statements = 0`) the next transaction may
  run on another server connection, so a statement prepared in one request and bound in the
  next was lost at random (`26000`).
- **The query session's statement cache** (`ConnectOptions::statement_cache`, from the
  profile's `statement_cache`, on by default; `query::Prepared::off`): off, no statement
  outlives its transaction. Outside the user's block the statement is prepared as the unnamed
  statement right after its transaction's `BEGIN` (`prepare_opened`, one write) and bound in
  that transaction (`Client::transaction_opened`); in the block it is the unnamed statement
  after the block's first statements; a statement without rows found that way is rolled back
  and runs as the other statements without rows do; the client's statements for type lookups
  are dropped before the transaction ends (`Client::forget_typeinfo_statements`). Each
  statement then costs what a new one costs with the cache on (two round trips for a first
  page instead of one).
- **The automatic fallback** (per session, never written to the profile): the session counts
  the times the server lost its statements (`26000` at Bind or at a type lookup's Parse).
  The first can be the user's doing (`DISCARD ALL` in a function) and is prepared again once,
  as before; the second, or a `42P05` (a statement of the same name already there), turns the
  cache off, and the statement runs once more without it (nothing ran). The session sends
  `DbEvent::StatementCacheOff` once; the tab puts it in the run's Messages, and the status bar
  says it once per connection of the profile (`ProfileConn::cache_off_said`).
- **Statement names** of the vendored tokio-postgres are `s<prefix>_<n>` with a random prefix per
  process (upstream `s<n>`), so statements other clients left on a pooled server connection do
  not collide with ours. `DEALLOCATE`/`DISCARD ALL` also drops the client's type lookup
  statements (they are gone on the server too).
- Tests: `datarig-driver-postgres/tests/pooler_pg.rs` against PgBouncer 1.25 (compose profile
  `pooler`, and a CI service) in transaction mode without prepared statements and with round
  robin over three server connections, which makes the loss reproducible.

## The transport seam

- **`datarig_core::transport`**: a `Dialer` hands out one byte stream (`BoxedStream`, tokio's
  `AsyncRead + AsyncWrite`) per connection to `host:port` as the far end of the transport sees
  them. `ConnectOptions::dialer` and `Driver::ping`'s `dialer` carry it (`DialerRef`: compared
  and printed by identity). A dialer never opens its transport: the app does, and a dial on a
  transport that is not open fails at once (`DialError::NotOpen`). `DbError::Transport`
  carries a `DialError` (not open, refused by the far end with a `Refusal`, failed with a
  fault, timed out); the TUI words each (`App::dial_error_text`; a failure to reach the far end
  reads as any connection failure).
- **PostgreSQL** (`driver-postgres/src/route.rs`): a session with a dialer takes its `Route`
  from the config (one TCP host, or the `hostaddr`, and one port; several hosts or a Unix
  socket are refused before anything is dialed), dials it within the connect timeout and runs
  `Config::connect_raw` over the stream. Every cancel request (`Cancel`: the canceller's and
  the one closing a running session sends) dials a stream of its own and uses
  `CancelToken::cancel_query_raw`. Without a dialer nothing changes.
- **Tests**: `transport::TcpDialer` (feature `test-util`, a plain TCP connect that counts its
  dials). The driver's integration suite runs a second time in CI with
  `DATARIG_TEST_DIAL=tcp`, every session, test connection and cancel through it; one test
  counts the dials of a session, its pages, a cancel, a close while running, a metadata
  session and a test connection.
- The SSH tunnel (`datarig-ssh`) implements `Dialer`; the app hands it to `open_session` and
  the test connection. A profile without a tunnel passes no dialer.

## SSH tunnels

- **`datarig-ssh`** (russh 0.63, `ring` backend, no compression): `Tunnel::open` connects to one
  hop (the name resolved here, a per-step timeout that stops while a question waits), checks the
  host key, logs in, and is then a `Dialer` whose dials are `direct-tcpip` channels. The client
  handler refuses every channel the server opens; nothing but `direct-tcpip` is ever requested.
  Host key types with SHA-1 signatures (`ssh-rsa`) are not offered, and the types stored for the
  host come first; an RSA login signs with `rsa-sha2-512/256` (a server that takes only `ssh-rsa`
  is refused). Keepalives (default 15 s, three missed) end a dead tunnel; `Tunnel::lost` says
  why.
- **Host keys** (`known_hosts`): `~/.ssh/known_hosts` is read (hashed names, `[host]:port`,
  wildcards, negations, `@revoked`) and never written; datarig's own file
  (`<data dir>/known_hosts`, 0600, written whole) decides for the hosts it has and is the only one
  written. A key the files do not have for a host that has entries is **changed**, never unknown.
  Every unknown or changed key goes to the app (`Asker::host_key`); nothing is accepted by itself.
- **Keys** (`keys`): OpenSSH and PEM (PKCS#1 RSA as AWS hands them out, PKCS#8, SEC1 EC),
  encrypted or not, with a `-cert.pub` next to the key; a file others may read is refused as
  OpenSSH does; a PuTTY key is named as such.
- **The profile** (`core::profile::ssh::SshSettings`, `[connections.ssh]`): one bastion (no
  multi-hop and no `~/.ssh/config` import yet), its secret under its
  own account `profile:<id>:ssh` with the same five sources as the database password.
- **The app** (`app/tunnel.rs`): a profile whose tunnel is on opens it once its database password
  is known (`start_connect`), or reuses its open one when the settings are the same; every
  session of the profile (`open_session`: `ConnectOptions::dialer`), its cancels and its aux
  sessions dial through it; with no tunnel open they dial `ClosedTunnel` (never directly). The
  tunnel opens on a task (`Tunnels`: `SshTunnels` in the binary, a recorded fake in tests); its
  secret is read there; its questions come back as `AppEvent::Tunnel` (a test's as
  `AppEvent::TestTunnel`) with a channel for the answer, bound to their owner (`tunnel::Owner`: the
  attempt's generation, or the test's sequence): a host key is a `ConfirmAction::TrustHostKey`
  confirmation (Enter cancels), a secret is the password prompt (`PromptPurpose::Tunnel`), saved
  to the tunnel's store once it opened. `Connecting::tunnel` holds the stage (the status bar says
  it) and keeps the attempt's own timeout from running. Disconnect, delete, cancel and a connect
  failure that asks for nothing close the tunnel; a lost one marks the node
  (`ProfileConn::tunnel_lost`) and the next use connects again.
- **Test connection**: a throwaway tunnel of the form's settings; the stage while it opens, then
  its time next to the database's result (`TunnelTest`, `test_msg`).
- **Tests**: an SSH server in the test process (`datarig-ssh/tests/tunnel.rs`: host keys, every
  login, refused channels, timeouts, keepalive loss, a scratch `ssh-agent`); a real OpenSSH bastion
  (`dev/ssh`, compose profile `ssh`, a CI step; keys made at test time by
  `make-fixture.sh`) in front of PostgreSQL and PgBouncer (`datarig-ssh/tests/bastion.rs`,
  `datarig-tui/tests/integration_ssh.rs`); the app's flows with a recorded tunnel
  (`flows_tunnel.rs`); round trips through the tunnel (`[rtt_ssh]` budgets); `cargo audit`.

## Decision: PostgreSQL's parser for safety classification

- **Context**: the lexer-based classifier was fooled wherever the lexer and the server disagreed: `EXPLAIN ("analyze") DELETE` (a quoted option name), a `--` comment ended by a lone `\r`, `x<NBSP>$$` (an identifier to PostgreSQL, a dollar quote to the lexer). Each was a DELETE or UPDATE of every row without a question. Chasing the scanner and the grammar by hand does not end.
- **Decision**: classify from the parse tree of libpg_query (PostgreSQL 17's `gram.y` and `scan.l` built as a C library) through the `pg_query` crate 6.2 (MIT; libpg_query BSD-3-Clause; PostgreSQL code under the PostgreSQL License; protobuf-c and xxHash BSD-2-Clause; see `vendor/README.md`). On every platform, Windows included: pg_query supports Windows since 5.1 and its CI builds on `windows-latest` (MSVC and GNU). The build compiles the C library with `cc` and generates bindings with `bindgen`, which needs libclang (present on the hosted runners and with Xcode; on a developer machine install LLVM). `protoc` is not needed. The tree is walked as JSON (the crate's `serde` serialization), so nested nodes of every kind are visited without a traversal of our own.
- **Costs**: about 3.6 MB more in the release binary (6.0 MB to 9.6 MB when this was decided; the budget is 16 MB, and the binary had since grown to 13.7 MiB on macOS arm64 and 20.7 MiB on Linux x86_64; the release profile's LTO and stripping brought it to 9.5 MiB on macOS arm64), a longer first build (the C library, about 80 s), and libclang at build time. A parse per classified statement (only when a run is checked, never per keystroke).
- **Alternatives rejected**: fixing the lexer only (the next disagreement is the next bypass); a lexer-based fallback on Windows (not needed); asking the server (`PREPARE` of the text needs a connection and is itself a statement).
- **Parse failure**: `Danger::Unparsed`: it always asks, and a read-only policy refuses it ("PostgreSQL's parser cannot read this statement"). A text over the caps is `Danger::TooComplex`, which does the same ("too long or too deeply nested to check"); see "Never a crash in the classifier" above.

## Decision: a vendored tokio-postgres

- **Context**: the grid and every copy format must show PostgreSQL's own text for every type. The driver decodes the common types from binary and needs the server's text output for the rest (ranges, geometry, bit strings, text search, `money`, `"char"`, `reg*`, enums, composites, extension and unknown types). tokio-postgres 0.7.18 binds every portal with the result format `1` (binary) and exposes no way to choose; the upstream pull request that would, [sfackler/rust-postgres#961](https://github.com/sfackler/rust-postgres/pull/961), has been open since 2023-11.
- **Decision**: vendor tokio-postgres 0.7.18 in `vendor/tokio-postgres`, used through `[patch.crates-io]`, with one additive API, `Transaction::bind_with_formats` (a result format code per column); every existing API behaves as upstream. Each change is marked `datarig:` in the source.
- **Alternatives rejected**: the simple query protocol (text only, no portal paging, no types), casting every column to `text` in the statement (changes the user's SQL and loses the types), and a fork on GitHub (more to maintain than three small patches).
- **Costs**: re-syncing by hand with each upstream release (security fixes included); the workspace excludes `vendor`, so its tests and lints do not run here, while the driver's PG tests (every type compared with `format('%s', …)` and round-tripped through SQL INSERT) cover the change.
- **Pipelining**: the copy also pipelines a portal's first page (`[BEGIN] + Bind + Describe + Execute + Sync` in one write) and a statement without rows (`Parse + Describe + Bind + Execute + Sync`), so a first page costs one round trip (two for a statement new to the session) and DML one. The driver keeps the session's prepared row-returning statements by text (64) and prepares again after the server refuses one whose result changed, or inside the user's block. Alternatives rejected: parsing the socket ourselves (a "socket tap"), a second fork, and holding the implicit transaction with `Flush` instead of `BEGIN` (no fewer round trips, one more for a large result, and the statement timeout would keep counting while the user reads, since it stops only at `Sync`).
- **Exit**: drop the copy once a tokio-postgres release lets a caller choose per-column result formats and pipeline a portal's first page. Provenance (checksum, upstream commit), the exact change list, the re-sync steps and the exit criteria are in [`vendor/README.md`](../vendor/README.md).

## Failures as data

- **Core never words a failure.** `datarig_core::fault::Fault` is a kind (`io::ErrorKind`, a TOML error with its line, a key of the wrong shape, one of the keychain failures, or other) plus the raw detail. Core error types carry it (`workspace::Loaded::broken`, `scripts::IndexProblem`, `secret::Unavailable`/`StoreError`/`FileError::Io`/`SourceError::Keychain`, `CommandError::Start`, `SwitchError::Config`, `migrate::Report`, `config::save`). `config::load` returns a `ConfigError` enum (read, syntax, version, a bad value with its key, profile and allowed values, duplicate id, missing key). The driver reports `driver::DbError`: the server's own message (data, shown verbatim), `Closed`, `NoAnswer`, `Settings`, `Connection` (a `Fault` with the io kind found in the error chain) and `NotSupported`.
- **The TUI words them.** `app::fault_reason` maps a fault to a short catalog reason, `config_error_msg` and `App::db_error_text` do the same for config and database failures, and `App::fault_text` also appends the raw detail to `<state>/errors.log` (`fault::ErrorLog`, one line per failure). The status line never shows OS, parser, keychain or driver text; only database server messages are shown as they are.

## UI strings (i18n)

Every piece of UI chrome text (titles, labels, hints, status and error messages, action and key-context names) comes from the locale catalogs, and the compiler enforces it.

- **Source of truth**: `locales/en.toml` defines the keys. `crates/datarig-core/build.rs` checks every catalog listed in its `LOCALES` table and generates `OUT_DIR/messages.rs`, which `datarig_core::i18n` includes:
  - `Label`: one `Copy` variant per key without placeholders (`pane.tree.title` → `Label::PaneTreeTitle`).
  - `Msg`: `Msg::Label(Label)` or one variant per key with placeholders, whose fields are the arguments (`query.done_rows` → `Msg::QueryDoneRows { count: u64, elapsed: Duration }`). The field type follows the placeholder name: `count` is a `u64` (thousands separators), `elapsed`/`latency` a `Duration` (`123ms` / `1.2s`), anything else a `String`.
  - **Plurals**: an entry with `{count}` is a table with two forms, `"key" = { one = "…", other = "…" }`; the generated `render` picks `one` when `count` is 1. The `one` form may leave the number out (`"its open tab"`). A language without plural forms (Korean) gives a plain string for the same key. Never write `tab(s)`.
  - A key typo is an unknown variant and a missing or mistyped argument is a struct-literal error, so both fail to compile. The rules live in `build/catalog.rs`, a pure module that `tests/catalog.rs` exercises with fixtures; the `i18n` module docs have `compile_fail` examples.
- **Rendering**: `I18n::label(Label)` and `I18n::msg(&Msg)` return `Localized`. Widgets that draw chrome take `&Localized`, not `&str`: `panel`, `modal`, the status line, hints, which-key and help entries, command-line entries. Status messages are stored as `Notice { msg: Msg, level }`, so a language switch re-renders them.
- **Escape hatch**: `Localized::verbatim(..)` wraps text that is not translated: user and database data, SQL, key names (`Ctrl+E`), or chrome composed from those and already localized parts (the which-key title `Space c — Connection`). Never pass English UI wording to it; add a key instead. `git grep -n 'Localized::verbatim'` lists every use for review. Data paths (grid cells, tree items, the editor) keep their own `String`/`&str` types, and the low-level `put` drawing helper takes `&str` for both.

### Adding a UI string

1. Add `"area.name" = "Text with {placeholder}"` to `en.toml`, and the translation with the same placeholders to `ko.toml`. Keys are lowercase words joined by `.` or `_`. A text with a number of things uses `{count}` and both English forms: `"area.name" = { one = "{count} row", other = "{count} rows" }`; `ko.toml` keeps one string.
2. Use it: `i18n.label(Label::AreaName)` or `i18n.msg(&Msg::AreaName { placeholder: value })` (`Notice::new(msg, level)` for the status bar). `cargo build` fails until both catalogs have the key with the same placeholders.
3. For a table of labels (action registry, `:` commands and settings, leader groups, hints), store the `Label`, not a string.

### How locales are enforced

The build fails, naming the file, the key and the problem, when a locale misses a key of `en.toml`, has a key `en.toml` lacks, or has a different placeholder set for a key. It also rejects malformed keys, two keys with the same Rust name, and placeholders that are Rust keywords. Plural rules: in `en.toml` every entry with `{count}` must have `one` and `other` forms, a plural entry must have `{count}`, both forms must have the same placeholders (`{count}` may be left out of `one`), and `(s)` pseudo plurals are rejected; a translation may use one string or both forms for a plural entry, but no forms for an entry that is not plural in `en.toml`. Each locale in `build.rs` has a policy: `Policy::Required` (en and ko today) makes a missing key an error; `Policy::Optional` makes it a `cargo:warning` and the generated code uses the English text for it. Extra keys and placeholder mismatches are errors under every policy. Shipping a partial translation is therefore a one-line change in `LOCALES`.

### Adding a locale

1. Add `locales/<code>.toml` with the keys of `en.toml`.
2. Add `("<code>", Policy::Required)` (or `Policy::Optional` for a partial translation) to `LOCALES` in `crates/datarig-core/build.rs`.
3. Add the matching `Lang` variant (`ja` → `Lang::Ja`) in `crates/datarig-core/src/i18n.rs` and its detection in `detect_lang`; the generated `match lang` does not compile until the variant exists. Then add the language setting and actions in the TUI (`LangSetting`, `action.ui.language.*`).


## Adding a driver crate (for example MySQL)

1. Create `crates/datarig-driver-mysql/` with `datarig-core.workspace = true` and the database client crate in its `Cargo.toml` (versions go to the root `[workspace.dependencies]`), and `[lints] workspace = true`.
2. Add it to `members` and to `[workspace.dependencies]` (`datarig-driver-mysql = { path = "crates/datarig-driver-mysql" }`) in the root `Cargo.toml`.
3. Implement `datarig_core::driver::Driver`:
   - `capabilities()`: turn on only what the driver really supports (`server_paging`, `cancel`, `introspection`, `contexts`, `key_metadata`, ...).
   - `connect(cfg, role, opts, events)`: for each role (`SessionRole::Meta` for the tree and completion, `SessionRole::Query` for a tab's statements) start a background task with **one connection** and return `Session::new(caps, role, tx, canceller)`. Report `opts.application_name` to the server. Progress and results go out as `DbEvent`s (`Connected` / `ConnectFailed`, `Schemas`, `Objects`, `Catalog`, `Page`, `Done`, `Failed`, `TxOpen`). A metadata session answers `Execute` with `Failed`. With `Capabilities::structure` the metadata session also answers `LoadStructure` with a `driver::structure::TableStructure` (`DbEvent::Structure`), which the explorer shows under an open table.
   - `ping()`: the test connection. It honours the time limit and cancellation (the future being dropped).
   - Export the driver type only (`pub use connect::MySqlDriver;`).
4. Add the dependency to `datarig-tui`'s `Cargo.toml` and register the name in `driver_for` in `src/drivers.rs` (`"mysql" | "mariadb" => Some(Box::new(MySqlDriver))`). No UI code changes.
5. Tests: unit tests such as value decoding go to `src/<module>/tests.rs`, real-database tests to `crates/datarig-driver-mysql/tests/`. Take the connection URL from an environment variable, print `SKIPPED` without it, and fail when the matching `DATARIG_REQUIRE_*` variable is set (as `integration_pg.rs` does with `DATARIG_REQUIRE_PG`). Add a service container to the CI `integration` job.
6. If core seems to need driver-specific code (a dialect's statement splitting, say), do not branch on the driver's name: add a trait or a capability to core and let the driver implement it.
