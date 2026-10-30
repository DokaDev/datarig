# datarig architecture

This document describes how the Cargo workspace is put together: the crates and their
dependency rules, the state the TUI keeps, and the design decisions behind the safety model,
the PostgreSQL driver, SSH tunnels and the UI strings.

## Crates and responsibilities

| Crate | Responsibility | Depends on |
|---|---|---|
| `datarig-core` | The UI-free core. The driver interface (`Driver`, `Capabilities`, `Session`, `DbCommand`/`DbEvent`, key metadata `driver::keys::KeyCatalog`), the copy and export formats of result rows (`export`: text, and the Excel and Parquet writers), SQL tooling (lexer, statement splitter, completion engine, the safety classifier `sql::risk`), the connection profile model and DSNs, password stores (`SecretStore`: OS keychain, the secrets file `secrets.toml`, memory) and per-profile password sources (`keychain`, `file`, `command`, `env`, `prompt`), the trash, reading and saving the config file (`toml_edit`), the i18n catalogs, and the transport seam (`transport`) | External crates only (tokio `sync`, serde, toml, keyring, pg_query, ...) |
| `datarig-driver-postgres` | The PostgreSQL implementation `PgDriver`: connecting (`connect`), one connection per session role: metadata (`meta`: introspection for the tree and completion, one per profile) and query (`query`: one per tab, portal paging, cancel), value decoding (`values`) | `datarig-core`, `datarig-tls` + tokio-postgres (vendored) |
| `datarig-driver-mysql` | The MySQL implementation `MyDriver` on mysql_async (vendored): connecting directly or over a tunnel's channel (`connect`, `route`, `link`), the same session roles as the PostgreSQL driver: metadata (`meta`) and query (`query`), what the server says changed in the session (`session`), value decoding (`values`) | `datarig-core`, `datarig-tls` + mysql_async (vendored) |
| `datarig-ssh` | SSH tunnels: one bastion's `direct-tcpip` channels as a `transport::Dialer` (`Tunnel`), host keys against `~/.ssh/known_hosts` (read only) and datarig's own `known_hosts`, key files (`keys`). The only crate that names russh | `datarig-core` + russh (its ssh-key vendored) |
| `datarig-tls` | TLS for the drivers' connections: a profile's TLS settings (`core::tls`) become a `Connector` (the CA, client certificate and key files read, the verifier picked by the mode: none, the chain against the CA file, or the chain and the name against the CA file or the system's trusted certificates through rustls-platform-verifier), which does the handshake over a stream a driver opened and says what the connection got (`Encryption`) or why it failed (`TlsProblem`); PostgreSQL's SCRAM channel binding. The only crate that names rustls | `datarig-core` + rustls (ring) |
| `datarig-tui` | The Ratatui UI and the `datarig` binary: app state, actions and the overlay stack (`app/`), the context keymap (`keymap/`), screens (`screens/`), widgets (`widgets/`), key input and the Hangul key mapping (`input/`), the clipboard (`clipboard.rs`: the system clipboard or OSC 52), the user's own editor (`external.rs`), the driver registry (`drivers.rs`) | `datarig-core`, `datarig-driver-postgres`, `datarig-driver-mysql`, `datarig-ssh`, ratatui/crossterm |
| `datarig-bench` | Performance measurements and the CI budgets (binary `datarig-bench`): round trips counted through a latency proxy, paging, the editor and the grid in a headless `App`, and the real binary idle and at startup in `tmux -L perf`. Not product code (see `docs/perf.md`) | All of the above |

```
datarig-tui ──▶ datarig-driver-postgres ──▶ datarig-core
     │  ├──────▶ datarig-driver-mysql ────────▲
     │  │          (both) ──▶ datarig-tls ────▲
     │  └──────▶ datarig-ssh ─────────────────▲
     └────────────────────────────────────────┘
```

## Dependency rules

1. **Dependencies point one way: `tui → core ← driver-*`** (and `tui → ssh → core`, `driver-* → tls → core`). Core depends on no driver crate and no UI crate (ratatui, crossterm). A driver crate depends on core and tls only and knows no other driver.
2. **The UI does not know driver types.** `datarig-tui` talks to a driver only through `Box<dyn Driver>` and a `Session`'s command and event channels, and turns menus and panels on and off with `Capabilities`. The only place that names a concrete type (`PgDriver`, `MyDriver`) is the registry, `crates/tui/src/drivers.rs`.
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
- Snapshot tests pin English screens only. A screen in another language is checked against its catalog (`assert_screen!` in `crates/tui/tests/common/mod.rs`), so the only non-English UI text in the repository is in `locales/`.

## Workspace state

The TUI has one screen, the workspace; dialogs sit on top of it in the overlay stack (`app/overlay.rs`).

```
App
├─ profiles, folders, last_used   the config's profiles (by id), folder tree and expanded folders
├─ presets, shared                the tunnel presets and their shared SSH connections
├─ session_presets                the Startup SQL presets (`[startup.<name>]`)
├─ policies                       the default rules and the environments (`[policy.<name>]`)
├─ conns: ConnectionManager       ProfileId → ProfileConn (app/conn.rs)
│    ProfileConn { meta: Option<Session>, generation, connected, connecting, error,
│                  tree (schemas), catalog, resolved (profile + password of the attempt),
│                  pending: Vec<Queued> (statements waiting for it), expanded, expand_on_connect,
│                  console }
├─ tabs: TabManager (may be empty) Tab { profile: Option<ProfileId>, binding (tab generation), pristine,
│                                       editor, results, exec: TabSession }
├─ explorer: Explorer             cursor (a row, not an index), scroll, `/` filter (app/explorer.rs)
└─ overlays                       command line, quick connect, profile form, chooser, name input,
                                  action menu, password prompt, confirmation, help, which-key, …
```

- **A profile's connection is its metadata session.** Several profiles can be connected at once, each with its own session, schema tree and completion catalog. Its node state (`○ ⠋ ● ✕`) comes from `ProfileConn::state()`; a failed attempt keeps its error for the line under the node.
- **A tab belongs to one profile** and opens its own query session on its first statement, with the password its profile connected with (`resolved`). A tab without a profile (a deleted profile brought back with `Space t u`, a recovered console, a profile the config does not list) shows a banner and asks for one in the quick connect list before it runs.
- **The workspace may have no tab**: it never starts with an unbound console, and closing the last tab leaves none; the right side then shows an empty state. `TabManager::active()` is a blank stand-in while there is none (id 0, never drawn or saved; `active_mut()` hands out a fresh one each time), the actions that need a tab are unavailable (`has_tab`) and only the explorer takes the focus.
- **A statement queued for a connection runs only where it was queued.** A tab whose profile is not connected yet queues its statement in the profile's `pending` as `Queued { tab, profile, binding, statements }`, where `binding` is the tab generation (`TabManager::bind` gives the tab a new one every time it is bound). On `Connected` it runs only if the tab still exists, is still bound to that profile and still has that generation; otherwise the tab gets a warning notice and nothing runs. A queued statement is work in progress like a running one (`App::tab_busy`, `any_running`): `Space c s`, closing the tab, `x`, deleting the profile and quitting ask first and then drop it; cancel drops it; a failed attempt tells its tabs. The other deferred paths hold no statement across a rebind: tree and catalog requests name their profile and are generation-checked, paging fetches only from the session whose portal is open, and `:run` in a tab without a connection runs on the profile the user picks for it.
- **A first connect opens a console** when the profile has no tab. A tab without a connection is never bound silently.
- **Events are routed by target and generation.** `EventTarget::Meta(ProfileId)` or `EventTarget::Tab(TabId)`, each with the generation of the session that sent it; every new attempt of a profile and every new tab session gets a new generation, so a late event of a closed or replaced session is dropped. Password commands report back with the profile and generation of their attempt.
- **Opening a table runs in that table's profile's tab** (its most recent one, else a new console), never in another profile's tab.
- **The explorer's rows are rebuilt when needed** from the profiles, the folders and the connections (`App::explorer_rows`): "＋ New connection", then folders before profiles at each level by name, the error line of a failed profile and the schema tree of a connected, expanded one. The cursor stores its row, so it stays on the same node while rows appear or disappear around it. The `/` filter (`explorer::Needle`: the text folded once, names compared without allocating) is applied while the rows are built: a folder or profile that matches by name shows as without it, a connected profile is searched in the trees it has read (`Tree::rows_matching`: schemas, objects of listed schemas, items of read structures), and the way to a match is shown open through `Row::opened` (the arrow too) without touching any expansion state, so clearing the filter gives the tree back unchanged and nothing is asked of the server; `Row::hit` marks what matched (the cursor goes to the first one). Under a tunnel preset the rows are the profiles connected through it now (`App::preset_connected_users`); `preset_users` (every profile that names it) still drives its count, the delete question and the test.
- **The tab list** (`app::tab_list`, `widgets::tab_list`, overlay `TabList`, context `overlay.tab_list`): the open tabs by `TabManager::by_recent` (the active one, then the ones made active before it, most recent first, from `TabManager::activate`, which every switch, open, close and reopen goes through; the tabs not made active in this run follow in the tab bar's order, so a restored workspace starts in its own order; the order is not saved), then `TabManager::closed` (the `Space t u` list, newest first), each `ClosedTab` found by its `serial`. Filtering ranks every entry with the action menu's `menu::rank`. `Ctrl+D` is `App::request_close` (`Ctrl+W`'s path, with its confirmations, for any tab); `App::close_tab` refreshes an open list, which keeps its selection on the same entry (or in the same place among the open tabs). `Enter` on a closed one is `App::reopen_closed`, `Space t u`'s path for that entry. Rows draw the tab bar's own number and marks (`tabbar::number`, `tabbar::marks`).
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

- **Key metadata is a driver capability** (`Capabilities::key_metadata`). The metadata session reads a `KeyCatalog` (tables, columns by number, primary/foreign/unique marks) once after the catalog and again on `DbCommand::LoadKeys` (a schema refresh), as `DbEvent::Keys(Result<…>)`. Result columns carry `ColumnMeta::origin` (`ColumnOrigin::Pg`: table id + column number, from the RowDescription; `None` for an expression). The TUI keeps the catalog per profile (`app::Keys`: unknown, loaded, failed) and looks columns up when it draws, so a warm cache costs no request per query and a late one still shows. `DbEvent::Catalog` is a `Result` too: a catalog that cannot be read is never taken for an empty one.
- **Copying** (`app/copy.rs`): the grid's range (`GridState::anchor` to the cursor), the cell, the row or every fetched row, formatted by `core::export`; the SQL INSERT target comes from `driver::keys::insert_source` (an allowlist: one plain table in `FROM` by the lexer, every column of it by the RowDescription, else refused) or, for `:copy insert <schema.table>`, `driver::keys::insert_into` (columns matched by name). `clipboard.rs` picks the way (`clipboard = auto | system | osc52`, SSH from the environment); the system clipboard is an `Opener` the binary sets (arboard; tests use a fake, so no test touches the real one), and an OSC 52 sequence waits in `App::take_terminal_output` until the binary writes it between frames.
- **Export to a file** (`app/export.rs`, `app/export_form.rs`, `app/export_path_pick.rs`, `widgets/export.rs`, `core::export::{stream,keyset,xlsx,parquet}`): a job writes `<path>.partial` on a writer thread of its own (`spawn_writer`) and renames it on success; each job ends with exactly one terminal event (`ExportNews::Ended`). The fetched rows and the selection are read from the result's store. Every row of a result with more on the server is streamed by the tab's session (`DbCommand::Export`) into a bounded channel the writer drains: a keyed table tab by short keyset reads, any other statement by running it again (asked first unless it is on the `sql::risk::repeat` allowlist, and asked in a read-only or prod-like environment); outside the user's transaction the driver reads in a read-only transaction of its own. The writer feeds a `RowSink`: `stream::TextWriter` for the text formats, `xlsx::XlsxWriter` (rust_xlsxwriter in constant memory mode: rows to temporary files in `TMPDIR`, zipped as ZIP64 when the writer ends, a drop asked meanwhile keeps nothing) and `parquet::ParquetWriter` (the `parquet` crate's column writers without Arrow, Snappy, a row group of at most `GROUP_ROWS` rows or `GROUP_BYTES` in memory, the footer written at the end, also after a cancel). `xlsx::read` and `parquet::read` read the files back for the tests. The path field completes names (`Tab`) and `Ctrl+O` opens the file-system tree (`ScriptTree` in `TreeMode::ExportPath`) over folder listings only.
- **Settings**: `config::Prefs` (`[commands] position`, `detail_view`, `clipboard`, `copy_header`, `[editor] cursor_shape` and `clipboard`, `[explorer] size_heat`, `[ui] yank_flash`, `[ui] tips`) next to the other settings. One table, `app::command::SETTINGS` (key, values, label, description, category), drives both `:set` and the settings screen (`app/settings.rs`, a large modal overlay `overlay.settings`), so they cannot drift apart; every change goes through the same `apply_setting` and `config::save` (toml_edit).
- **Tips** (`app/tips.rs`, `core::usage`): one catalog (`tips::catalog()`) of tips, each with its place, its subject (an action and the context whose key it names, a command, a setting's value) and a catalog entry with a `{key}`, `{command}` or `{setting}` placeholder; the text never names a key itself (a unit test scans en and ko), the key comes from `App::tip_key` (`Keymap::tip_keys`: the user's own key first, else one without a function or Menu key). The candidates of a place are the next first step whose key works there (empty workspace and empty editor), then the tips whose key works in the current key context (command and setting tips only where `:` opens the command line); the pick is `candidates[epoch % len]`, kept per place with its rendered line for (epoch, usage generation, key map, language, icons, key context), so a frame only looks it up, and no pick at all while an overlay is open (nothing kept, so the tip is back when it closes). The epoch moves only when the page turns (focus, tab, a run starting or ending: `App::tips_follow` after input and database events), so a tip costs no timer and no wakeup. Every run is counted by source (`App::dispatch_from`: key, menu, command line; `dispatch` itself counts nothing; nothing with tips off) in memory and written to `<state>/usage.toml` with the workspace and when the first steps are dismissed; a first step taken another way (any menu opening, any run) is recorded as such (`App::step_taken`); no usage file but profiles or a workspace marks the first steps done (users from before tips); an unreadable file is never written over, unknown ids are kept, counts merge by maximum. A first run with the first steps showing starts on the welcome panel (its new-connection key one press away); otherwise the new-profile form opens over it at once.
- **Grid search** (`app/grid_search.rs`, the pattern rules shared with the editor in `widgets/search.rs`): `/` and `?` open a prompt (`Ctx::GridSearch`, text input whose reserved `Ctrl+W` deletes a word, `Ctx::is_prompt`) drawn on the line under the grid; `Enter` makes it the search that is on, and while one is on the grid's keys resolve in `Ctx::GridMatches`, a mode of `grid` whose `n`, `N` and `Esc` hide the grid's (`Ctx::is_mode`, exempt from the shadow check only), so `n` is the next page again once it ends (and once a search finds nothing). A search reads the rows on a worker thread over a `RowsSnapshot` of every fetched row (memory or the spill file, the window untouched), never on the UI thread: each job has a number and is bound to its tab, the result id and the rows and columns it read; a new key stops the job before it (an `AtomicBool` it checks between chunks of rows), and an answer for a job the tab no longer waits for (replaced, another result, another view, a new run) is dropped. A job that finds every match answers the first one from its origin (the cursor moves, to another page when it lies there, unless it moved meanwhile) and then every matching cell as a sorted list of row-major indexes of the rows it read, capped at 1,000,000. `n`/`N` step through that list while no rows came since; past the cap, or while the list is still being found for these rows, a seek job (`seek`) looks for the next match only, from the cursor's row on (backward a chunk at a time from the end of each part, `RowsSnapshot::for_each_chunk_in`) and wraps with a second range, beside the job that finds them; after rows came, a job finds every match again from the cursor. The line under the grid carries what the paging line would say there (with the next page's key as `grid.matches` resolves it), or that the server has more rows, shortened where it does not fit. The highlight looks each cell on screen up in the list (`[grid]` budget); without a search it looks nothing up. Headless, `App::pump_grid_search` hands the answers over.
- **Drawing**: the command line is a popup or the bottom line (`widgets/cmdline.rs`); the inspector is a panel next to the results (`widgets/inspector.rs`, `Layout::detail`); the results title carries the paging state on its right (`widgets::panel_with_status`, the title clipped first). No emoji is drawn: marks are Nerd Font glyphs (`icons.rs`) or text, checked by `tests/no_emoji.rs`.

## Performance

Held by CI budgets (`docs/perf.md`).

- **Result rows** (`datarig-core::results`): a `RowStore` per result keeps `result_window_rows` rows in memory and writes every row past that to a spill file (`results::spill`: one `0600` file per result in `<state>/spill`, a varint encoding of the decoded cells, the offset of every 256th row in memory). The window follows the pages and moves to the rows the grid shows, reading ahead; `for_each_chunk` streams rows for copies (`export::Writer` writes any format a chunk at a time with the same text as all at once). A row not in memory is `Row::NotRead`, never an empty row. The file stops at `spill_limit` (config, policy) and fetching stops with it. Files are deleted with their result and at quit; a launch deletes only the files of a process id that is not running and whose `datarig-spill-<pid>.lock` nobody holds.
- **Editor** (`widgets/editor.rs` and its modules): lines, edits as splices of the lines they touch, undo as the changes of each command (`buffer`). Vim's command grammar is parsed in `vim` (`["x] [count] operator [count] motion|text object`, doubled operators, the `g`/`z` prefixes and the character after `f t r`); a motion (`motion`) or a text object (`textobj`) gives where it leads and, for an operator, a range that is exclusive, inclusive or whole lines, with Vim's adjustments; `%` and the bracket objects find brackets with the lexer's tokens (`brackets`), so brackets in strings and comments do not count; the operators apply to a range or to the Visual selection (`visual`, by character or by line) as one undo step. `.` (`repeat`) replays the keys of the last change (its count apart, an Insert session's keys included, a Visual operator's selection as its size) through the same parser; the tests compare every command with what Neovim does on the same text. Registers (`registers`) follow Vim: named ones with append, `"0`, the `"1`-`"9` delete ring, `"-`, `"_`, and `"+`/`"*`, by character, by line or as a block. A write Vim with `clipboard=unnamedplus` would send to the clipboard is offered to the app (`Editor::take_yank`, routed by `App::editor_yanked` through the copy path and `[editor] clipboard`); for `"+p` the app asks `Editor::reads_clipboard` before the key and reads the system clipboard only then (`SystemClipboard::get_text`, never over OSC 52). The widget never touches the clipboard. The lexer state at each line start is cached (between tokens, or inside a token that spans lines, with its start) and invalidated from an edited line on; a frame lexes the lines on screen from the nearest known state. The statement under the cursor and the completer's text come from lines around the cursor, taking more until the `;` around it are in them; this equals splitting the whole text (tested on random texts).
- **Event loop** (`main.rs`): it sleeps until `App::next_tick` (100 ms only while something counts in tenths or animates; else the earliest autosave, portal close or countdown second; none when nothing waits) and draws no frame for a mouse move, unless the move selects another item of an open menu or the keyboard help, or changes which dialog button or list row, or which small target of the workspace, is highlighted (`App::menu_hover`, `App::help_hover`, `App::overlay_hover`, `App::workspace_hover`; the terminal reports motion without a button through crossterm's any-event tracking, `?1003h`).
- **Round trips**: see the vendored tokio-postgres decision below.

## Terminal, runs and copies

- **Terminal** (`datarig-tui::terminal`): what the binary sets on the terminal (alternate screen, mouse, bracketed paste, kitty keyboard flags) is recorded in one `TermState` and undone exactly once, from a guard made before the setup or from the panic hook; the restore always gives the user's cursor shape back. The cursor's shape follows the key context (`terminal::cursor_shape`: a block in vim Normal/Visual, a bar in Insert and every text input; `editor.cursor_shape = off` leaves it alone). Escape sequences go to any writer, so tests check every exit path.
- **Handing the terminal over** (`app/effects.rs`, `external.rs`, `term.rs`): `Ctrl+G` and `Ctrl+Z` / `:suspend` queue an `Effect` (`App::take_effect`) that the binary runs between frames. It stops the event stream first (so it reads none of the editor's keys), gives the terminal back (`term::release`: the `TermState` restore, which marks the state undone, so a panic meanwhile writes nothing more and one after `term::reclaim` restores again), and afterwards takes it again with the same kitty flags, starts a new event stream and draws everything on a cleared screen. The editor (`external::edit`) is `$VISUAL`, else `$EDITOR`, else `vi` (`notepad` on Windows), split by `shell-words` and run without a shell on a new file (`create_new`, `0600`, in `<state>/edit/`, `0700`) holding the text and a line break; the file is removed afterwards. What comes back (`external::Edited`) is new text, no change (the same bytes, or the same text without the last line break), or a failure with its reason (an exit code or signal: Vim's `:cq` keeps the text; a file that cannot be read back is never an empty text). `App::external_edit_done` replaces the tab's text as one undo step (`Editor::replace_text` splices only the changed middle, so marks around it stay), or opens a new console for a table tab's copy, or for a tab whose text changed meanwhile. Suspending sends `SIGTSTP` to the process group (`term::stop`, as Vim does), so `fg` resumes it; Windows has no suspend (`effects::SUSPEND_SUPPORTED`). A running query goes on on the server; while the editor is open its events wait on the channel.
- **Theme** (`datarig-tui::theme`): every color and style a widget draws comes from a `Theme` value; no widget names a color. Plain colors are roles (`bg`, `surface`, `fg`, `accent`, `warning`, `mode_*`, `current_stmt_bar`, `running_stmt_bar`, the series colors `chart_1` … `chart_6`, …); a token a theme without RGB may need to show with a modifier is a `Style` applied with `Style::patch` (`selection`, `range`, `cursor_line`, `hover` (what is under the pointer: a background of its own, never an underline), `current_stmt`, `running_stmt`, `run_hint`, the `syn_*` tokens, and `search_match`, `match_paren`, `read_only_mark`, `danger_mark`, `plan_hot`, `plan_misestimate`, declared for features not drawn yet, and `size_mild`, `size_warm`, `size_heated`, `size_hot`, the explorer's estimates of a schema's largest tables); derived, not tokens: `Theme::selection_unfocused` (a pane's selection without the focus: the selection's background a fifth toward the pane's, or dimmed), `Theme::name_color` / `name_style` (a profile's name in its environment's color, moved toward white or black until it reads at 4.5:1, bold while connected, muted toward the background while not, 3:1 at least) and `theme::chip` (an environment's badge, drawn as is everywhere, `App::badge_chip`); `dim` says how the screen behind a dialog is dimmed (blend toward a tone, or `Modifier::DIM`). `App::theme` holds the active theme; `screens::draw` makes it the thread's current theme for the frame (`theme::scope`) and widgets read it once per render with `theme::cur()`, which is `dark` on a thread that set none, so tests on parallel threads never see each other's theme. Profile colors (`theme::PROFILE_COLORS`, `profile_color`) are not part of a theme.
  - *Choosing a theme*: one config key, `theme` (`Config::theme`, written only when it is not `terminal`), holds a name. `theme::resolve` turns it into a `Theme`: a built-in (`theme::BUILTINS`: `terminal`, the default, of ANSI roles and `Reset` with `Dim::Modifier`; `dark`, `light`, `high-contrast` and the Catppuccin, Tokyo Night, Gruvbox, Nord and Dracula palettes in `theme/builtins.rs`), a family (`theme::FAMILIES`) whose variant follows the terminal's background, or a user file `<config dir>/themes/<name>.toml`. Core (`datarig_core::theme`) reads and checks a theme file strictly (tokens, colors as `#rrggbb`/ANSI names/`default`, modifiers; each problem with its line) into a plain `ThemeSpec` without ratatui; the TUI maps it onto the `extends` theme (`theme::from_spec`, token names from `theme::COLOR_TOKENS`/`STYLE_TOKENS`). `App::theme_name` keeps the configured name even when it does not resolve; then the app draws with `terminal` and says why (a launch notice and the settings screen, `App::theme_problem`). The background (`theme::Background`) is asked once by the binary (`term::background`: OSC 11 through `terminal-colorsaurus`, at most 100 ms, only when stdout is a terminal) before the event stream starts, and handed to `App::set_background`; tests inject it. `:set theme=` completes over `App::theme_names` (the built-ins, then the files); the settings screen's theme row (`command::Values::Themes`, the one setting whose values are listed when asked) previews with `h`/`l`, keeps with `Enter` (`App::set_theme`, saved like every setting) and goes back with `Esc`.
  - *Tests*: the theme tests run over every built-in: WCAG contrast (body text on the background and the surfaces 4.5:1, muted text and the status colors 3:1, mode badges `mode_*` with `mode_fg`, the statement bar), xterm-256 distinctness of key marks and mode badges, readability of the dimmed screen, marks that carry a modifier and stand apart from warnings; the `terminal` theme by role (warnings yellow, errors and danger red, selection reversed). Flow tests draw with `dark` unless they choose a theme (the harness pins it for the default `terminal`, which has no RGB to compare).
- **Runs of several statements**: still one `DbCommand::Execute`; the driver reports `DbEvent::Started`/`Finished` per statement and stops between statements when the session's canceller flagged a cancel (`DbError::Cancelled`). The tab keeps the run as an ordered list of per-statement outcomes (`app::runlog::RunLog`); the last statement's rows are the tab's result.
- **Run marks in the editor** (`widgets::editor::runs`, `app::run_hints`): `Ctrl+E` takes the statements with their byte spans (`Editor::run_statements`) and stages them; `run_approved` binds them to the run's query id when it sends exactly those statements (`Editor::start_run`; a queued, confirmed or rebound run still matches, anything else is not marked). Every splice moves the spans (an edit before a span shifts it, one inside marks it edited), so the mark stays on its text. Each frame the app passes the statement the run is at (`RunLog::current`) and the spinner frame (`Editor::set_running`). After every event of the tab's session, `settle_run_hints` turns the ended run's `RunLog` into one hint per statement (`Editor::finish_run`); a statement edited meanwhile gets none, and the driver's `Block(false)` after a `ROLLBACK`'s answer amends its hint. Hints are drawn after the text, never in it, and go with the first edit of their statement.
- **Driver replies**: the query session's `Reply` answers a run by value (`page`, `done`, `fail`, `closed`), so a second terminal event does not compile; the steps of a run hand the reply back while the run goes on.
- **Copies** (`app::copy`): a scope (`CopyScope`: the selection, or every fetched row) and a format (`CopyFormat::MENU`, written by `core::export`); `Action::Copy(scope, format)` is one registry entry per pair, so keys, the command line and the menus share them. The grid's context menu has a second level (`app::menu::SubMenu`). SQL UPDATE is planned by `driver::keys::update_source` (the INSERT allowlist plus the table's whole primary key).
- **Action menu** (`app::menu`, drawn by `widgets::menu`): one `ContextMenu` overlay for a right click (at the pointer) and `menu.open` / `tab.menu` (next to the selection). It holds the items for what it was opened on and the pane's, the filter (`overlay.context_menu` is a text-input context) and a `MenuTarget`: the explorer row and its name, the grid's result, cell and range, the editor's text version, cursor and mode, or the tab, with the tab's binding. An item runs only while `menu_target_now` still equals it, so a menu left open while the tree was reloaded or a result replaced never acts on something else. The filter ranks prefix matches, then word starts, then letters in order (`menu::rank`); with no match it lists `action::search`, the command line's search.
- **Inspector focus**: `Focus::Inspector` and the key context `inspector`; the conflict check allows exactly one shadowing there (its tab switch over `pane.next`/`pane.prev`).

## Safety

- **Classification is core's** (`datarig-core::sql::risk`, pure), **from PostgreSQL's own parse tree** (libpg_query through the `pg_query` crate; see the decision below): a statement's `Class` (read, session, tx, write, DDL, maintenance, procedural, unknown), whether it writes rows, a missing, always-true or column-less `WHERE` (`NoWhere`), why it asks (`Danger`: destructive, a `MERGE` that updates or deletes, `ALTER … TYPE` with or without `USING`, `DO`/`CALL`, an unknown prepared statement, a text the parser rejects, a text too long or too deeply nested to parse), its target, and `Explain` (plan only, or `ANALYZE` in any quoting, classified as the statement it wraps). Data-modifying statements, row locks and `SELECT … INTO` are found at any depth; `set_config()` of the read-only settings marks a statement as asking for read-write. `Risk::read_only`, `Risk::confirm` and `Risk::rolls_back` are the three questions the rest asks. A text whose plain strings hold a backslash is parsed a second time with those strings spelled as `E''` strings (what a server with `standard_conforming_strings = off` runs); the worse reading counts. It is an allowlist: unknown text is treated like a write, and text the parser rejects always asks. `risk::Prepared` keeps a session's prepared statements by name, so `EXECUTE` is classified as the statement it runs; each tab keeps one, inside its language's `risk::Classifier` (see "The dialect seam"), for its query session (`TabSession::prepared`, empty for a new session), changed only by what the server confirmed (see "sent ≠ succeeded" below).
- **The lexer stays for the UI** (highlighting, the statement splitter, completion) and follows PostgreSQL's scanner where tokens end (ASCII-only whitespace, `\r` ends a `--` comment, `$` and non-ASCII characters inside identifiers, dollar-tag characters). Differential tests hold the splitter to libpg_query's split on a corpus of tricky texts; a piece the splitter got wrong would fail to parse and ask.
- **Environments** (`core::policy`): an `Environment` is a `[policy.<name>]` table other than `default` (its rules, an optional color and badge, an in-memory `EnvironmentId` that survives a rename), kept in the file's order in `Policies` with the default rules. `Policies::get` gives a profile's rules: the default rules without a name, the environment's, or `UnknownEnvironment` for a name no environment has, which is an error of that profile (`App::connect` refuses it like a `RouteError`, `read_only_refusal` refuses every statement of a connected one, `App::rules_of` treats it as read-only), never the default rules. `config::save` writes the environments only where they changed (`write_environments`: a table found by the name it was read with, each item compared by value with what the file has, so an untouched value keeps its text). The section's first row edits the default rules (`[policy.default]`, `config::write_default_rules`, only what changed). The app's section, form and presets are in `app/environments.rs`; the profile form picks one with `Picker::Environment`. The fields with a few fixed values (driver, SSL mode, password storage, bastion login and its secret's storage: `Field::is_choice`) are picked the same way with `Picker::Choice` (the value's index, set by `ProfileForm::pick`, which refuses one that cannot be picked, a driver the app has none for or the keychain while it does not work; the list shows it struck through).
- **The TUI checks before it sends or queues** (`app/safety.rs`): `App::run_in` refuses what a read-only policy does not allow (a notice naming the policy), then asks about the dangerous statements (`Overlay::RunConfirm`, key context `overlay.run_confirm`, Cancel focused) and only then calls `run_approved`, which queues the run for a connecting profile or sends it; `run_approved` checks read-only again, and a queued run is not asked about twice. An answer runs only on the tab, profile and binding it was asked for (like `Queued`).
- **The server enforces read-only, per transaction** (the hard guarantee): with `ConnectOptions::read_only` (set for every session of a profile whose policy is read-only then or now) the query session opens every transaction `READ ONLY` itself: a portal's own transaction starts with `BEGIN READ ONLY`, a statement without rows outside the user's block runs between `BEGIN READ ONLY` and `COMMIT` (transaction control and session settings are not wrapped), and the user's block gets `SET TRANSACTION READ ONLY` with its first statement (before an `EXPLAIN ANALYZE` savepoint too). All of it goes out in the write the statement goes in (no extra round trip; the budget has read-only scenarios), and the user's SQL is not rewritten. A statement that asks for read-write is never sent (`DbError::ReadWriteRefused`), because a block may go back to read-write before its first query. The startup option `-c default_transaction_read_only=on` stays as a second layer; a server that drops it (a pooler) no longer fails the connect: `DbEvent::ReadOnlyPerTransaction` follows `Connected` and the status bar says so. `TabSession::read_only` records it; a policy that became read-only after the session opened blocks runs there until a reconnect.
- **Sent ≠ succeeded**: state that depends on what a statement did changes only when the server confirms the statement succeeded, never when it is sent; when the outcome is unknown the state becomes unknown, and unknown counts as dangerous. For prepared statements: `App::run_approved` keeps the run's statements in `TabSession::unconfirmed`; `DbEvent::Finished` (a statement before the last) and the run's answer (`Done`, or the first `Page` of the last statement) apply them to `TabSession::prepared` (`TabSession::succeeded`); `Failed` (error or cancel, including the statements the run never reached), `Lost`, a failed reconnect and an unanswered cancel forget every name they may touch (`TabSession::unsure`, `risk::Prepared::forget`), so an `EXECUTE` of such a name asks and a read-only policy refuses it. A new body is never assumed. A statement that may run code the text does not show (`risk::Risk::runs_code`: `DO`, `CALL`, a call of a function not in `risk::BUILTINS`, which is PostgreSQL 17's `pg_catalog` less `risk::RUNS_CODE` (the built-ins that run a query or code they are given: `query_to_xml` and friends, `ts_stat`, `ts_rewrite`, `pg_input_is_valid`, `brin_summarize_range`), any write, DDL or maintenance statement, which may fire a trigger, `COMMIT`, `PREPARE TRANSACTION`, `SET CONSTRAINTS`, `FETCH`/`MOVE`, a text that is not read) forgets every name, on success and on an unknown outcome alike (a plpgsql function can re-prepare a name as a `DELETE`); a new session starts with none. Not seen, because the parse tree does not show a function call without the catalog: a view's query, row-level security, operators, casts, domain `CHECK`s and input functions of user types, a user's function in attribute notation (`t.f`, `(t).f`, `(t.*).f` are column references and indirections to the parser; a name of `risk::RUNS_CODE` in that notation is taken as a call and forgets), and an unqualified call that resolves to a user function sharing a built-in's name (an overload in a schema of `search_path`, for built-in argument types too, or a schema ahead of `pg_catalog`).
- **Never a crash in the classifier**: `Prepared::classify` runs on a thread of its own (`risk::THREAD`, stack `risk::STACK` = 256 MiB of address space, committed as used), because parsing and walking the tree recurse per level of nesting in C and Rust and a 30 000-term `1 + 1 + …` overflowed the UI thread's 8 MiB. Before parsing, a linear lexer estimate caps the input (`MAX_BYTES` 256 KiB, `MAX_DEPTH` 16 000 levels: chains count, lists and `AND`/`OR` do not, joins and set operations count whatever separates them, groups add their depth); over the caps the text is `Danger::TooComplex` without being parsed. The caps also bound the time (libpg_query's protobuf output grows with the square of the depth: about 0.1 s at the cap in release). At the cap a release build uses under 32 MiB of stack, a debug build under 96 MiB (`[profile.dev.package.pg_query] opt-level = 2`: the generated decoder's unoptimized frames are tens of KiB per level). prost's own recursion limit (100) is off (`no-recursion-limit`), since it made ~46 chained operators or ~50 joins unreadable; right-nested texts still stop at PostgreSQL's parser limit ("memory exhausted") and ask. A panic on the thread is caught (`Danger::Unparsed`); the binary's panic hook, process-wide, restores the terminal for a panic on any other thread and leaves it alone for this one, whose panic the app survives.
- **Typed refusals**: a read-only policy names why it refuses a text it could not check (`ReadOnlyBlock::Unparsed`, `ReadOnlyBlock::TooComplex`). `COPY … FROM STDIN` and `COPY … TO STDOUT` (`Risk::stdio`) are refused under any policy before anything is sent (`App::unsupported`): the driver does not carry the COPY protocol yet, and a `FROM STDIN` sent broke the connection. `COPY` with a program or a file name runs on the server's operating system and always asks (`Danger::CopyProgram`, `Danger::CopyFile`, the target being the file or program of a `COPY … TO`); a read-only policy refuses it as a write. A call of a built-in in `risk::SERVER_FILES` (server files by a path it is given: `lo_import`, `lo_export`, `pg_read_file`, `pg_ls_*dir`, `pg_stat_file`) or `risk::SERVER_ACTIONS` (beyond the transaction: backends, configuration, WAL, replication slots and origins, statistics resets, index summarizing) is `Danger::ServerFile` or `Danger::ServerAction`, naming the function, wherever it runs (not in what `EXPLAIN` only plans or a view, rule or function body stores; the parameters of a plain `EXPLAIN EXECUTE`, bare or in `CREATE TABLE … AS EXECUTE`, do run: the server evaluates them to plan, `risk::Prepared::plans`); since the server's read-only transaction does not stop them, a read-only policy refuses them (`ReadOnlyBlock::ServerFile`, `ReadOnlyBlock::ServerAction`). An integration test pins every volatile built-in of PostgreSQL 17 to one of the lists or a reviewed harmless list. The call is recognised in every form the parse tree names it (`risk::Call`): a `FuncCall` whose last name is in a list, however qualified (`datarig.pg_catalog.lo_export(…)`, which the server accepts, or a user's schema, which over-asks), and attribute notation, where each name of an `A_Indirection` and each name after the first of a `ColumnRef` counts (`('/etc/hostname'::text).pg_read_file`, `(0).pg_cancel_backend`; a real column of such a name asks too). The built-in allowlist reads `db.pg_catalog.f` like `pg_catalog.f`. **Queries given as text**: a built-in that runs a query it is given as text (`risk::query_position`: `query_to_xml`, `query_to_xmlschema`, `query_to_xml_and_xmlschema`, `ts_stat`, `ts_rewrite(tsquery, text)`) runs what that query calls, so a string constant there (also cast to `text`, by position or named `query`, in every call form of `risk::Call`) is classified as a statement of the session on the same parse thread (`risk::Prepared::query_text`: both backslash readings, the same caps per text, at most `risk::MAX_QUERY_TEXT_NESTING` texts deep and `MAX_BYTES` of query texts per classification, counted in a thread-local of the parse thread), and its risk is the call's: a harmless query stays a read; anything else (not a constant, rejected by the parser, over a cap) is `Danger::RunsQueryText`, which asks and which a read-only policy refuses (`ReadOnlyBlock::RunsQueryText`). The other names of `risk::RUNS_CODE` run no caller SQL (`cursor_to_xml` fetches a cursor whose `DECLARE` was checked; `table_to_xml` and friends read tables; `pg_input_is_valid` runs a type's input function): they only forget the prepared names. The refusal covers the calls the text shows: a server built-in called by a view, a trigger or a user's function is not seen.
- **The metadata session never waits on another session** (`meta::read`): each catalog read runs in a transaction of its own, `BEGIN READ ONLY; SET LOCAL lock_timeout = '2s'` and JIT off (`set_config('jit', 'off', true)`, PostgreSQL 11 and later: a catalog of some thousand relations can plan a read over `jit_above_cost`, and compiling it cost ~0.6 s for a read of a few ms), the statement and `COMMIT` pipelined into one write (no round trip more; both settings end with the transaction, so a pooler hands nothing of them to another client). A read that would wait for a lock (a catalog under `VACUUM FULL`, a table an `ALTER TABLE` holds) gives up with `DbError::Locked` instead of hanging, and of holding up the sessions queued behind it. It runs only the app's own catalog queries (constant text, no user input), under any policy; the startup option `default_transaction_read_only = on` of a read-only policy is a second layer.
- **The confirm is a guardrail, not a boundary**: it catches mistakes in what the text shows. Functions called from a query are not asked about (it would ask for nearly every query), and a view's query, row-level security, operators and casts of user types, or a user function that shadows a built-in's name do not even forget the prepared names; a read-only policy is what guarantees that nothing is written.
- **`EXPLAIN ANALYZE` is the driver's**: the query session rolls back the portal's transaction instead of committing it, or, in the user's block, wraps the statement in `SAVEPOINT datarig_explain` / `ROLLBACK TO` / `RELEASE`. The UI only announces it (`TabSession::explain_rolled_back`).
- **Runs to the end**: a row-returning statement before the last of a run is fetched page by page to its end (`DbEvent::StepRows`, kept per statement in `TabSession::steps`), so it runs as psql runs it. The session reports `TxAborted` besides `TxOpen`, and reports a portal's implicit transaction only when it stays open.
- **Pending intents are bound**: a copy that waits for an answer or for rows carries a `copy::Intent` (tab, profile, binding, session generation, result id) and is dropped when any of them changes.
- **Signals** (binary, Unix): SIGTERM, SIGHUP, SIGINT and SIGQUIT end the event loop through `App::quit_on_signal` (write, close sessions, quit); the terminal guard restores the terminal as on a normal quit. While the external editor runs the terminal is out of raw mode, so its `Ctrl+C` and `Ctrl+\` reach datarig too: SIGINT and SIGQUIT received meanwhile (or within 50 ms after, still on their way from the handler) are dropped, SIGTERM and SIGHUP end the program once it has. A terminal that cannot be taken back (it hung up) ends the program like SIGHUP, with what the editor saved written. Their test (`tests/signals.rs`) runs the binary under `script` in a guard that kills and reaps both on every path, a panic included; it also drives `Ctrl+G` with a script as the editor and `Ctrl+Z`.

## Layout, result tabs and pages

- **Tabs are documents** (`app::tabs`): `TabKind::{Console, Script, Table, Ddl}`. A console has a number (`Doc::console_no`, the lowest free one, kept in `workspace.toml`); a table tab has its `TableRef` and its query as its (hidden) editor text, is never saved as a file and cannot switch connection (`action::query_tab`). The connection stays attached to the tab as before (profile, binding generation, session generation); the tab bar and the editor's first line only show it.
- **Panes** (`app::pane`): `Tab::pane` (`PaneLayout`: share, hidden, zoom; per tab, in `workspace.toml`) and `Tab::ran` decide whether a query tab draws its results pane (`App::results_shown`) and its editor (`App::editor_shown`); `App::focusable` / `fix_focus` keep the focus on a pane there after every input. `pane::results_rows` splits the height within minimums. `Layout::body` and `Layout::divider` let a drag of the results' top border resize.
- **Zoom and the explorer** (`app::pane`): `PaneLayout::zoom` (tmux style, per tab) draws one pane (`Tree`, `Editor`, `Results` with its inspector, `Inspector`) over the explorer's and the tabs' place, the tab bar and status bar staying (`App::zoomed`: the zoom while its pane is there; the status bar says `ZOOM`). The results' maximise (`z`, `Space r z`) is their zoom. `App::settle_panes` (`fix_focus`, `fix_zoom`, `fix_explorer`) runs after every input and every driver event: a focus moved by the input to a pane the zoom hides ends it; otherwise (a tab switch, the results coming back, also in the background) the focus goes to the zoomed pane. The action menu's own focus change is not a move. Only the results' zoom is kept (`maximized`, as before; a restored one waits for the results); the others end with the session. `Explorer::hidden` and `Explorer::width` are for every tab (`[explorer] hidden, width`): pane cycling skips a hidden explorer, a move of the focus to it shows it again (a focus that lands there otherwise, a tab switch, goes to the editor or the results; a zoom of it draws it without showing it), and a restart with it hidden focuses the restored tab. `pane::explorer_width` clamps the width to `EXPLORER_MIN` and three fifths of the workspace when drawing (the preference stays); without one, a quarter of it (24 to 40). The screen records what it drew in `Layout` (`workspace`, `tree`, `explorer_edge`, …), the only source of the mouse's hit tests: a pane not drawn has an empty rect.
- **Result tabs** (`TabSession`): the shown row result is the tab's `results`/`grid` as before, so copy, the inspector and the grid keys are unchanged; the others are parked in `TabSession::steps` (`Parked`: rows and their own `GridState`) by statement index; `ResultView` picks rows, the plan (see below), the chart or Messages (`draw_messages`, from `RunLog` and its `notes`); the strip (`App::strip_shown`) is drawn whenever the results pane is, in a table tab too: the result tabs (one `Result` tab while there is no row result), the Plan, the Chart, then Messages. A run replaces the row results only at its first row event (`replace_pending`, `drop_rows`); until then, and when it returns none, `kept_log` holds the log the rows came from, so `shown_sql` (copy's allowlist source) and `answer_index` stay right.
- **Holding a result or not** (`policy::Policy::paging`, `driver::PagingMode`): the app decides per run from the tab's policy and sends it with `Execute` and `Resume`; the driver does it. `NoHold` (the default): outside the user's block the last statement's first page goes out with its transaction's `COMMIT` (`ROLLBACK` for `EXPLAIN ANALYZE`) in the same write (the vendored `Transaction::bind_first_page_then`), so a result never leaves a transaction, a lock or a snapshot on the server while it is read, behind a transaction pooler too; when more rows follow, `DbEvent::Released` comes right before that page, and the app's paging is `Paging::Released` (closed: past the rows only a re-run). A `Resume` whose skipped rows need more than its first request reads them first and commits before its page. A statement that writes and returns rows is shown only once that `COMMIT` succeeded (a failed `COMMIT`, a deferred constraint, fails the run). `Hold`: the portal and its transaction stay open while more rows follow (`TxOpen(true)`), closed after the policy's `paging_idle_timeout`. Inside the user's block both hold (the portal lives in the block). Engine-neutral in the app; the transaction handling is the driver's.
- **Pages** (`app::pages`, `app::paging`): `GridState::page`/`page_size` and `GridState::window` restrict the grid to a page; movement, clicks and drags stay inside it. Paging is explicit: `page_next` shows a fetched page, asks `FetchMore` (the page comes back to `want_page`), or runs the statement again past a closed or released portal (`DbCommand::Resume { id, sql, skip, paging }`) only when `sql::risk::repeat::repeatable` allows it, the result's `pages::Origin` (profile, binding, session generation) is the tab's now, and the read-only and confirm checks pass; the answer is appended only when its columns match (`Resuming`). `Paging::Open { in_block }` is never idle-closed inside the user's transaction; `Released` (never held: the first page of a statement that is not on the allowlist is all there is, `Msg::ResultsFirstPageOnly` says why and what to do, and the next page is refused with the same advice), `ClosedIdle`, `Replaced` and `Interrupted` (a fetch that failed or was cancelled; also a cancel the session never answered keeps `more`) are closed portals that keep `more`: unknown is not absent. The title's right side is built by `results_title` from the widest form to the narrowest (`widgets::title_and_status` takes a list): the arrows go first, then the portal's state, and the page number stays. The result tab strip (`draw_strip`, `strip_window`) always keeps the shown tab and Messages; its right end shows the keys of `results.tab.prev`/`results.tab.next` in the results pane's context (`strip_keys`, from the keymap) only while the pane has the focus and they fit after the tabs.
- **The allowlist has two halves** (`sql::risk::repeat`): the text's (`names`: one plain query, built-in non-volatile functions, built-in operators and types from `volatile.txt`, `operators.txt`, `types.txt`, all generated from the server and pinned by integration tests) and the server's (`check_query` of the `Names` the text shows: relations are tables, partitioned tables or materialized views without row-level security, inheritance included; no user function, operator or type shadows a name used; no column type with code of the user's). The driver sends the question right before a `Count` or a `Resume` in the same transaction (after the count's or the re-run's savepoint inside the user's block, in the same request) and answers `DbError::NotRepeatable` when the server gives a reason; nothing else is sent then. `DbCommand::CheckRepeat` (view as plan, below) sends the question alone: inside the user's block or a portal's transaction under the driver's count savepoint, rolled back to after it; inside an aborted block it is not sent and answers a refusal. The allowlist assumes functions are labelled honestly: a user function wrongly declared `IMMUTABLE` or `STABLE` can still run while a statement is planned (in a `CHECK` constraint read for constraint exclusion, for instance), for a re-run for the next page as for a plan. Inside the user's block a `Resume` runs under `SAVEPOINT datarig_resume`, rolled back to on a failure or a cancel and released when its portal ends (`committed(…, savepoint, …)`).
- **What a count counts**: the driver pages a statement outside the user's block in a plain `BEGIN` (`BEGIN READ ONLY` on a read-only session) sent with its first page, at the session's own isolation (and, not held, ends it in the same write: each page read again is a transaction and a snapshot of its own, so a result not held never claims one snapshot), and sends nothing else (a `REPEATABLE READ` + `LOCK TABLE` paging transaction was tried and reverted: locking every partition of a partitioned table exhausted the server's lock table). A count asks its transaction's isolation (`ISOLATION`, in the allowlist question's request) and answers `DbEvent::Counted { snapshot }`; the app labels it "now" (`ResultSet::counted_now`) unless it ran in the user's current block with one snapshot and every page was read in that block (`ResultSet::pages_in`).
- **Counting** (`DbCommand::Count` → `DbEvent::Counted`): `sql::risk::repeat::count_query` wraps an allowed statement; the driver checks it again, then runs it in the portal's or the user's transaction under the driver's count savepoint (rolled back to on failure; named `datarig_count_` and 16 random hex digits, made once per process, so it is never one of the user's) or on its own (`BEGIN READ ONLY … COMMIT` on a read-only session). `Running::count` marks it busy and cancellable.
- **The user's block** (`DbEvent::Block`, before the `TxOpen` of the same change) is tracked apart from `TxOpen` (which also counts a portal's own transaction): `Tab::block_began`/`block_ended` number the transactions (`tx_epoch`) and set `ResultSet::tx` (`TxMark::InTx`, `Ended`, `RolledBack`) on the results read in them; `Tab::ending_rolled_back` reads the statement that ended the block (and `tx_aborted`), and every path that ends a session marks a rollback.
- **Query plans** (`sql::plan`, core, pure): an engine-neutral model (`Plan`, `PlanNode`: the operation and its target as the source's text names them, estimated cost and rows, measured rows and loops (`Actual`), buffers, rows removed by filters, workers, every other property as the text form writes it, and the derived total and self time and self cost) with what the views ask of it (`measure`: measured time, else estimated cost, never presented as time; `share`, `is_hot` at a fifth of the whole, `misestimate` at ten times either way, per loop). `sql::plan::pg` reads PostgreSQL's `EXPLAIN (FORMAT JSON)` (13 to 18; unknown keys stay properties, never an error) through `sql::plan::json`, a JSON reader that keeps every value in one arena and recurses nowhere, so a deep plan neither hits a recursion limit nor overflows the stack. PostgreSQL's times are per loop: a node's total is `Actual Total Time × Actual Loops`, divided by the processes that ran it at once under a `Gather` (the gather's child's loops per gather loop); self time is the total less the children's, never below zero; a CTE's time is taken from the CTE scans that read it, not from the node it hangs under. `sql::plan::text` writes the plan as `psql` shows a text `EXPLAIN` (tests compare it to the server's own text of the same plans), never asking the server again. Fixtures of PostgreSQL 13 to 18 live in `sql/plan/fixtures`. `sql::plan::mysql` reads MySQL's tree text (`EXPLAIN FORMAT=TREE`, `EXPLAIN ANALYZE`; 8.0, 8.4, 9.x fixtures in `fixtures/mysql`); a node the server gives no cost carries its children's, flagged `PlanNode::cost_derived`, which every view marks `≈`. `sql::plan::explain_sql` and `pg::is_plan_column` are the PostgreSQL-only parts behind `ExplainFormat::PostgresJson`, with `sql::plan::explain::tree` (MySQL: a table `EXPLAIN` asked again as `EXPLAIN FORMAT=TREE` of its statement, never with `ANALYZE`) beside `sql::plan::explain::json`, which asks a text `EXPLAIN` again as `FORMAT JSON` (read with the lexer, its other options and comments kept as written, then checked with PostgreSQL's parser through `risk`: both texts are an `EXPLAIN` and `ANALYZE` stays as it was, or it is refused; `JsonExplain::evaluates` is false only when the `EXPLAIN` has no `ANALYZE` and no `EXECUTE` and the statement it wraps (`JsonExplain::statement`) passes the text half of `risk::repeat`, the allowlist of what the app may run again unasked).
- **The Plan tab** (`app::plan`, `widgets::plan`): `query.explain` and `query.explain_analyze` run the statement under the cursor (one statement selected) wrapped by the tab's format (`ExplainFormat::explain_sql`) through `App::run`, so the read-only refusal, the confirmation and the driver's rollback of `EXPLAIN ANALYZE` are the run's own. A result of one complete row of one column that the tab's format holds and reads (`ExplainFormat::holds`, `read`; `plan::plan_of`, in `tab_event` for the answer and for a statement before the last) becomes `TabSession::plan` (`PlanTab`: the plan, its statement's index, its source text and format, the view, the selection, closed nodes, the detail, scroll and pan) and `ResultView::Plan` is shown; the rows stay a result tab. The rows of a text `EXPLAIN` say under them that `results.view_as_plan` views them as a plan: it runs the statement of those rows (`Tab::shown_sql`, never the editor's text) through the format's `again` (`explain::json`, `explain::tree`) and `App::run`; when it `evaluates` a confirmation comes first; otherwise the server's half of that allowlist is asked first on the tab's session (`DbCommand::CheckRepeat` → `DbEvent::RepeatChecked`, `repeat::check_query` alone, under a savepoint inside the user's block or a portal's transaction), and its refusal (a view, a name a user's function or operator shadows) asks too. The wait lives only while its tab, session generation and session are the same (`App::sweep_as_plan` after every event; `Ctrl+C` on the tab ends it too), always ending with one notice; one wait at a time (a press on another tab ends the first, said there); the tab says it is asking until the wait ends (then it says how); a refusal that arrives while a dialog is open or the focus types text only says to press again, and the question that opens by itself takes no key until it has been on screen for `ARM_DELAY` (`ConfirmAction::ExplainAgain`, the statement waiting in `App::pending_as_plan` with the tab, binding, session generation, run and result it came from, run only while they are the same). Refused: rows read under another binding or session than the tab's now (`RunLog::binding`, `RunLog::generation`), rows of an earlier run (`kept_log`: statements ran since) and rows of a run of several statements (what ran beside it is not run again). Whether the shown rows are a text plan is worked out once per result (`App::text_plan`), not per frame. It goes with the row results of its run (`drop_rows` drops it; a run without rows keeps it). Its key context is `plan` (below `nav`), its menu target `MenuTarget::Plan` (the run, the statement, the view and the node: a later plan makes the menu stale). `widgets::plan` draws a line of the views' names (the pointer picks one) with what the numbers are, the view (the tree, summary cards over a compact tree, an icicle and a flame graph laid out in one pass over the nodes, a squarified treemap of the own times with the nodes too small for a cell together, a timeline of the per-loop times to the first and last row, the row flow on a log scale, a box diagram laid out as a tidy tree (leaves side by side, parents over the middle of their children) of which only the boxes on screen are drawn, the raw text), and the selected node's detail beside it (wide) or below it; every view draws only the lines on screen and counts the nodes it walks (`widgets::plan::take_work`, the `plan` budget). On the focused selection a mark's color may not read on a light theme's selection, so only its modifier stays there.
- **Charts** (`datarig-core::chart`, pure and engine-neutral; `app::chart`, `widgets::chart`): `results.chart` (`c` in the grid), like the Chart tab the strip always names, shows `ResultView::Chart`; `App::chart_sync` makes `TabSession::chart` (`ChartTab`) the first time that view is drawn or acted on, so naming the tab reads no row; without a row result the tab only says so. Nothing is sent to the driver. Each column gets a `chart::Role` from `ColumnMeta::kind` (a number, a date, time or timestamp) or, for text, from its first rows (every value a date); `chart::infer` picks the first `Spec` (kind, X column or the row number, Y columns, a "by" column, log scale), and `chart::carry` keeps the user's `Spec` by column name for a later result. `chart::Builder` reads every fetched row once (`RowStore::for_each_chunk`, the spill file included) into a `Model`: points along the X axis (summing rows with the same X; sorted along a number or time axis; past `MAX_BARS` the largest and an "others" bar), series (past `MAX_SERIES` the largest and an "others" series), what was skipped (NULL, not a number), and per value how many rows it sums and its first row (the readout reads that row's cell for the exact text). The model is rebuilt only when the result (`ResultSet::id`), its row count or the `Spec` changes (`ChartTab::built`, `Source`); `App::chart_sync` makes the chart follow the shown result before a frame or an action. `chart::scale` makes the axes (1/2/5 steps, powers of ten, calendar steps for times). `widgets::chart` draws from the model: bars in eighths of a cell, scrolled to the cursor; a line chart rasterized into braille dots once per model and plot size (`Raster`, per dot column its lowest and highest dot, so a frame paints only cells); hits for the mouse; `work` counts rows read, points and dot columns walked and cells painted for the `chart` benchmark. Stale bindings: the column lists carry the result's id (`ChooserPurpose::ChartX/Y/By`), the menu's `MenuTarget::Chart` the shown result's id and the `Spec`.
- **Workspace** (`core::workspace`, version 2): console numbers, table tabs, `results = { share, hidden, maximized }`, `[explorer] hidden, width`; a version 1 file reads with defaults and is copied to `workspace.toml.v1.bak` (`create_new`) before it is first written; tabs of an unknown kind are kept as TOML text (`WorkspaceState::unknown_tabs`) and written back, and the file's `active` counts them. `save` reads the file it replaces and merges back every key it does not know (`merge`: the top, `[explorer]`, each tab found again by `tab_key`, `results`); it refuses to replace a file of a later version (`Loaded::newer`), and the app then restores that file's tabs and runs workspace-read-only (`App::workspace_newer`, the banner says why).

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
  again and the `Guarded::on_late` hook hears of it (`KeychainDone::Late`). On top of that `app::keychain` runs every keychain use of the UI on a worker
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
- **The grid scrolls sideways by character.** The columns are laid out as one row of
  `"│ " + value + " "` (`GridState::col_x`, the start of each, from the widths the render
  computes) and `GridState::x_off` is how many characters of it the view starts after; the
  row-number gutter stays. The render clamps `x_off` to `0..=x_max()` (the last column's end at
  the view's right edge) every frame, so a resize, the inspector or wider values never leave an
  empty band, and draws the columns at the edges through `text::slice_cols` (a wide character
  cut there is a blank). `hit_cols` holds the part of each column on screen and `cut_col` the
  one cut on the left, so `cell_at`, `cell_rect` (hover, the search's highlight, the copy flash),
  `header_col` and the context menu work on that part. Unless the wheel detached the view
  (`detached`), the render follows the cursor's column as little as it takes (`follow`): cut on
  the left, from its start; cut on the right, up to its end; wider than the view, from its start
  unless it fills the view already. The wheel (`ScrollLeft`/`Right`, Shift+vertical) and a drag
  event past the left or right edge (`drag_edge`; not the wheel extending a drag) move it
  `grid::SIDE_STEP` characters. A column of which only the separator shows is not in `hit_cols`.
  A cell cut by the right edge whose text columns on screen are blank though its text is not
  gets a dim `›` at the edge (never a header, never a cell showing only its separator or
  blank). A search's jump (`grid_search::reveal`) sets `GridState::reveal`, the match's start in
  the value as drawn, and the next render shows it in a column wider than the view. The binary hands a burst of wheel events of one kind (as of pointer motions)
  to the app and draws once (`input::burst::take_burst`, at most `PER_FRAME` events; the
  benchmark uses it too).
- **The tab bar** (`widgets::tabbar`): `tabbar::state` reads a tab's session into a `State`
  (idle, connected, running, open transaction, trouble). The number carries the connection:
  the profile's color while the tab's session is connected, `fg_muted` while it is not, the
  status bar's spinner frame (as wide as the number) while a statement runs; only the
  warnings get a mark after the name, `◆` (`warning`, kept while a statement runs in the
  user's transaction) and `!` (`accent_warm`). Clicks and `Space n` go by the tab's index
  (`TabHit`), never by the drawn digit;
  labels are `Part`s whose document part shortens first when the tabs do not fit, and the `×`
  part records a `TabHit::Close` of its own, which `tab_bar_click` sends down the `Ctrl+W`
  path (`ConfirmAction::CloseTab` keeps the tab on Enter).
  A dragged tab (`app::tab_drag`) is shown at its place among the others
  (`tab_drag::shown_order`) and drawn where the pointer holds it, over them; `tab_drag::follow`
  moves it past a neighbour once it went past that neighbour's middle, by what the bar drew
  last (`tabbar::BarGeom`). Tabs are put together in a line of cells before the buffer
  (`put_tab`, then `mend` blanks what is left of a wide character another tab covers). A change
  of places the drag made (a neighbour moving over, the drop, a cancel) slides
  (`tab_drag::TabSlide`, keyed by `TabId`: `SLIDE` long, a frame every `SLIDE_FRAME`, the only
  timer it asks for, and only while something slides); any other change of places is drawn
  at once. `tab.grab` lifts the active tab the same way for keys (`TabDrag::keys`: drawn at its
  place, sliding, never under the pointer; a press, a dialog or another active tab ends it);
  `App::move_tab_to` makes every move slide. The event loop draws pointer motions read
  together as one frame (`main.rs` `input`).
- **The pointer on the workspace's small targets** (`app::hover`): the tab bar's tabs, `×` and
  scroll marks, the result tab strip's entries, the paging arrows, the Plan tab's view names and
  the entries of the inspector's Row tab (`Layout::detail_list` from `Detail::first`, lit with
  `widgets::hover_style`, the theme's `hover` background; a click there arms
  `App::detail_press`, an `overlay::Press`, and the release over the same entry moves the grid's
  column), and the grid's cells (`PointerOn::Cell`, from `GridState::cell_at`; lit the same way
  after the grid is drawn, `GridState::cell_rect`, in a selected range too). An entry and the
  selected row's cell of its column light each other; a hover never moves the selection or the
  focus, and a grid laid out otherwise (scrolled, another result) drops the hovered cell.
  `App::workspace_hover` finds the target under the pointer in the hits the last frame drew
  (`tab_hits`, `strip_hits`, `Layout::page_prev`/`page_next`, `PlanTab::view_hits`) and keeps it
  in `App::pointer_on`; a frame that lays those hits out otherwise (a tab closed or added, the bar
  scrolled, a resize) drops it before it is drawn, so no light is left on something else, and
  none is drawn under a dialog. The light of the scroll marks, the strip, the arrows and the view
  names is `widgets::pointer_style` (the text on the selection, readable on every theme); a tab's
  `×` changes its text color, to the theme's error color (an ANSI red on `terminal`); a tab other
  than the active one under the pointer takes the theme's `hover` as its background (on
  `terminal` the bright black with the terminal's text color).
- **Run keys the terminal can send.** `keymap::works` leaves `Ctrl+Enter` out of the keyboard
  help, the command line's list and the hints (`Keymap::hint_keys`) when the kitty keyboard
  protocol was not granted; `Ctrl+E` is bound wherever `Ctrl+Enter` is.
- **Keys in messages follow the keymap.** A message that names an app key has a `{key}`
  placeholder filled by `App::key_for` (the key the hint line would show in a fixed context);
  an action without a key there shows the command that runs it (`:w`), else its name, which
  the command line finds. Keys a dialog reserves for itself (`y`/`n`, its `Esc`) stay written
  out.

## Explorer icons and confirmations

- **Tree icons.** `icons::TREE` (a `TreeIcon` per node kind), `icons::TYPES` (a
  `TypeCategory` per kind of column type, `TypeCategory::of` reads the type's name as the
  server formats it) and `icons::STRUCTURE` (a glyph per group of a table's structure, which its
  items share) hold Material Design glyphs of Nerd Fonts v3 with their names; a test pins
  each name to its code point (checked against `glyphnames.json` of 3.5.1). The explorer's
  `node_parts` puts the icon before a node's label only with icons on, so the tree without
  icons is unchanged. `DbEvent::Objects` carries `SchemaObjects` (tables, views, the names
  of the views that are materialized, and `stats`: the `RelationStats` of each relation with
  storage); `widgets::tree::Children::Loaded` keeps them and
  `Tree::is_materialized` picks the icon.
- **Estimates on the object lines.** The PostgreSQL driver lists a schema's objects with their
  row and size estimates in one statement (`meta::SCHEMA_OBJECTS`, one round trip as before,
  budget `rtt.schema_objects`). The estimates are the `stats_ctes!` of `meta`, which the table
  structure's statement uses too, so the list and an open table never disagree: the rules are
  those of the table structure below, grouped by root (every table, partitioned table and
  materialized view of the schema, each partition its own and its parent their sum), in one pass
  over `pg_inherits`, `pg_class` and `pg_index`, locking no relation. A heap's rows are its
  `reltuples`, or the live rows of the cumulative statistics (`pg_stat_get_live_tuples`, the
  `n_live_tup` of `pg_stat_all_tables`, no lock either) when those are more than twice as many
  (rows added since the last `ANALYZE`; the counters restart after a statistics reset and can
  count again rows an `ANALYZE` saw, so they only ever raise the estimate, and only past that
  margin); its pages then grow in the same proportion (the planner's assumption: as many rows
  per page as at the last `ANALYZE`). `Tree::set_structure`
  puts a structure's estimates in the list too (`r` on an open table refreshes them). The
  explorer draws them right-aligned after the name in the room it leaves, two blanks at least
  (`explorer::inline_stats`, longest first: rows and size, rows, or the size alone when the rows
  are unknown), and the status bar the whole line with the estimates in words, "unknown"
  included (`explorer::line_preview`). `tree::size_heat` ranks a schema's estimates once, when
  they are stored or a structure updates them, into `Children::Loaded::heat` (the largest tenth
  by known size, one at least, in tiers `Mib`, `Mib100`, `Gib`, `Gib10` from 1 MiB, 100 MiB, 1 GiB
  and 10 GiB; equal sizes rank alike); the explorer draws those estimates with the theme's
  `size_warm` / `size_hot` (`[explorer] size_heat = "3"`, the two smaller tiers and the two
  larger), with `size_mild` / `size_warm` / `size_heated` / `size_hot` (`"5"`), or dim (`"off"`).
- **Table structure.** `driver::structure::TableStructure` (core) is one table's structure as a
  driver reads it: its kind (`RelationKind::groups` says which groups apply), a row estimate and
  a size (both estimates from the statistics; `None` when unknown: never 0 for "not analyzed";
  `TableStructure::stats`), columns (with `ColumnFill`: default,
  identity, generated), the primary key, foreign keys, indexes, unique and check constraints and
  triggers, each constraint, index and trigger with the server's own definition, for a DDL view
  to reuse. A driver with `Capabilities::structure` answers `DbCommand::LoadStructure` with
  `DbEvent::Structure` on its metadata session; the PostgreSQL driver builds it as one JSON
  document in a single unnamed catalog statement (`meta::structure`, one round trip, budget
  `rtt.table_structure`), never reading the table and never waiting for a lock on it: the size
  is `relpages` of the table, its TOAST table and their indexes (of the leaf partitions of a
  partitioned table, found through `pg_inherits`) times `block_size` (`meta::stats_ctes!`), since
  `pg_total_relation_size` and `pg_partition_tree` lock each relation. Whether it is known is
  the heap's own statistics (`reltuples` or `relpages` of the table, or of every leaf
  partition): an index and a TOAST table's index have pages from their creation on, so a table
  never vacuumed or analyzed would otherwise show the few pages of its indexes as its size. The
  heaps, their TOAST tables and indexes are joined once (semi-joins through `pg_index`), not
  looked up per heap, so the statement's planned cost grows with the catalog (under
  `jit_above_cost` with thousands of relations; the integration test pins it) and a table of
  2000 partitions reads in milliseconds. The deparsing functions
  (`pg_get_expr` with a relation, `pg_get_indexdef`, `pg_get_constraintdef` of a check,
  `pg_get_triggerdef` of a trigger with `WHEN`) lock the table (`AccessShareLock`), so they run
  only when `pg_locks` shows no `AccessExclusiveLock` on it, held or waited for; otherwise the
  statement asks for no lock and the answer is `DbError::Locked`, which the tree shows as
  "structure unavailable: the table is locked by another session (try again)". What needs no
  lock comes from the catalogs themselves: an index key's order (`indoption`, for a method that
  orders), operator class and collation when not the defaults (`Index::options`), and a
  trigger's `UPDATE OF` columns (`tgattr`, quoted by `quote_ident` as an index's keys are); a trigger's `WHEN` condition is taken from
  `pg_get_triggerdef` (`Trigger::condition`), so only where that runs. The status bar shows the
  line under the explorer's cursor whole (`explorer::line_preview`; for a structure that
  could not be read, the reason alone); a message too long for it takes the policy's room, then
  what it needs of the connection's name (which keeps its first 8 columns), and only then is
  cut in its middle when its end is a short last part (`text::clip_middle`: ` · ` and a closing
  ` (…)`), keeping what to do and what a trigger calls. A trigger's `WHEN` condition is a line
  of its own under it (`tree::item_details`), so a long condition never pushes its function
  out of the status bar. The TUI has no SQL for it:
  `widgets::tree::Tree::structures` caches it per `(schema, table)` with what of it is open,
  `TreeAction::LoadStructure` asks for it the first time a table opens (again after a failure,
  or with `r`), another database's through its aux session, and `TreeAction::Reveal` moves the
  cursor to a foreign key's table (asking for its schema's objects first when needed). Without
  the capability an open table shows its columns from the completion catalog, as before.
- **Show DDL.** A driver with `Capabilities::ddl` answers `DbCommand::LoadDdl { id, object }`
  (`driver::ddl::DdlObject`: a relation, an index, a trigger, a trigger's function, or a name the
  user typed with the tab's schema) with `DbEvent::Ddl { id, result }` on the metadata session
  of the tab's database: the catalog's parts as `driver::ddl::DdlSource` (a relation's
  `TableStructure` plus what its `CREATE` needs), which `sql::ddl::ddl_text` (core, pure, golden
  tests) writes out in `pg_dump`'s order under the header `-- Reconstructed by datarig from the
  catalog (not pg_dump)`, names through `sql::ident::sql_ident`. The PostgreSQL driver
  (`meta::ddl`) pipelines, in one read-only transaction and one round trip (budget
  `rtt.table_ddl`): the lookup into the transaction-local setting `datarig.ddl_target` (`r:`,
  `t:` or `f:` and an `oid`; a typed name in the tab's schema first), `search_path = ''` so every
  printed name is qualified, and the statement of that kind (`meta::ddl::RELATION`, which
  includes `structure_parts!`, `TRIGGER`, `FUNCTION`). Each statement checks `pg_locks` for an
  `AccessExclusiveLock`, held or waited for, on the relations its deparsing locks — the object,
  an index's or trigger's table, the relations a view's rule, a policy or an SQL-standard
  function body depend on (`pg_depend`), as probed on PostgreSQL 13 to 18 — and answers
  `locked` with nothing deparsed when there is one (`DbError::Locked`); `aclexplode`,
  `pg_get_userbyid`, comments, sequences and a string-bodied function lock nothing. In the TUI
  a DDL tab (`TabKind::Ddl`, `tabs::DdlTab`) holds its object, its state (`DdlState`: not
  read, waiting for the connection, reading, read, locked, failed) and the id of the request it
  waits for (`App::ddl_seq`): `app::ddl` sends it on the metadata session of the tab's profile
  and database (an aux one for another database) and takes only the answer of that request from
  that session; a session that ends, a failed attempt or a closed connection ends the wait. Its
  editor is read-only (`Editor::read_only`: the Normal and Visual commands that change the text,
  Insert mode, puts, pastes and `:s` are refused before anything happens, and the app says so);
  in Normal mode its keys are the context `editor.ddl` (`r` reads again, `o` opens the text in a
  new console through `console_with`), below `editor.vim.normal`. The run key reads it again; a
  restored or reopened tab (`workspace.toml` kind `ddl`) is not read until asked. Without the
  capability `D` says the driver cannot show DDL.
- **Keychain calls in order.** `App::keychain_job` takes the keychain accounts a job touches;
  `app::keychain::KeychainQueue` gives it a place in each account's queue when it is asked for
  (on the UI thread, a short lock) and its worker waits until it is first in all of them
  (`Turn::wait`; dropping the `Turn` leaves the queues, also on a panic). A job waits only for
  earlier jobs, each within the keychain's limit. A write that succeeds for a profile deleted
  since removes the entry again. A removal refused because a call given up on still hangs is
  kept (`keychain_unremoved`) and runs again when that call returns; a late write for a profile
  that is gone is removed then too, so no entry outlives its profile.
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
  `overlay.profile_form`) and the field's `[…]` button (`App::form_mouse`, `FormHit::KeyFile`)
  open it; a pick sets `ProfileForm::ssh_key_note` from the file's mode and name
  (`key_picker::key_note`).

- **The mouse on dialogs** (`app/dialog_mouse.rs`). While a dialog is on top, `App::overlay_mouse`
  sends the mouse to it and `App::overlay_hover` the pointer's moves; a screen too small to draw
  hits nothing. Hit-testing reads only what the renderer kept from the frame on screen, on the
  overlay itself (a new dialog starts with nothing to hit): `Buttons` (each button's rect and the
  one under the pointer), a list's rect and first row (`Chooser`, `QuickConnect`, `CommandLine`,
  `ScriptTree`, `Help`, the settings' `rows`), the profile form's `hits` (`FormHit` per rect,
  clipped to the box, the last drawn on top), and `TextInput`, which keeps the area, mask and
  hidden span it was drawn with so `TextInput::click` puts the cursor before the grapheme under
  the pointer (wide letters and its scroll included). A click does what the key for it does
  (a confirmation's button sends its key through `confirm_key`, a form button is `Enter` on it),
  so the key paths stay the only ones that act. A button, a list row (chooser, quick connect)
  and a command line entry act as a GUI button: a press arms it and the release over the same
  target acts (`overlay::Press`); presses in the first `overlay::ARM_DELAY` (400 ms by the app's
  clock) after the dialog came on top are ignored (the clock starts when it is drawn as the top
  overlay and starts again after another dialog covered it), and a move with no button held
  drops an arm whose release was lost. So a dialog that appears under a clicking pointer (a host
  key question, a conflict, one uncovered by a prompt that closed, the second press of a double
  click) does not take that click. Focusing a field, placing the cursor and stepping a value act
  on the press. The
  pointer only highlights a button or a list row (`Buttons::hover`, `ProfileForm::hover`, the
  `hover` of `Chooser`, `QuickConnect` and `SettingsScreen`); the focus and the selection, what
  `Enter` acts on, never move to it, so a destructive confirmation's default stays the safe
  button and a twitch of the pointer never changes what quick connect picks (the menu and the
  keyboard help, where the pointer selects, aside); a key, the wheel or new rows drop a list's
  highlight. A selector's `›` is clickable only where it was drawn. A click outside a dialog
  does nothing (the menu alone closes on one).

## Manual commit

- **The rule is the environment's** (`policy::CommitRule`, `commit = "auto" | "manual"`, with `tx_idle_warning` and `tx_idle_rollback`, at least `policy::TX_IDLE_FLOOR`; `tx_idle_timeout` is read as its old name, a table with both is `ConfigError::Alias`), read at each run (`App::tab_rules`), and applies only where the driver says it can (`Capabilities::manual_commit`, both drivers). `tx_idle_rollback` is kept by the app on every driver: the idle timer (`app/idle.rs`, `App::tx_rollback_of`, the limit the transaction began with) sends `DbCommand::EndTx { commit: false }` once a transaction opened for manual commit sat idle that long (the tab not busy), as an `Ending` with an `IdleEnd` deadline (`idle::TX_ROLLBACK_ANSWER_WAIT`, `TX_ROLLBACK_ANSWER_WAIT_MYSQL` on MySQL); the tab stays busy until the answer. `TxEnded` says it was rolled back and the session stays; a `Failed` answer or none by the deadline closes the session instead (fail closed; the server rolls back). PostgreSQL also gets a backstop as `TxRule::Manual { idle_timeout }`: the limit plus `idle::TX_ROLLBACK_SERVER_MARGIN` (the server's `SET LOCAL idle_in_transaction_session_timeout` with the `BEGIN`), so a stalled or killed app still has the server end it; MySQL has no such server limit. Auto sends `TxRule::Auto`, which is the driver's behavior from before, byte for byte on the wire.
- **The app decides which statement may write; the driver decides whether a transaction is open.** `App::tx_rule` turns each statement of a run into a `driver::TxStep` from the tab's classifier (`risk::Classifier::tx_step`, an allowlist: only a plain read the repeat allowlist takes, a session setting that calls nothing, transaction control, `EXPLAIN ANALYZE`, a PostgreSQL `no_tx_block` statement and a MySQL `implicit_commit` one do not open): `Open`, `ReadOnly` (a plain read: PostgreSQL runs it in `BEGIN READ ONLY` outside a block, so a write hidden behind it fails; MySQL runs it as it is) or `AsIs`, sent as `TxRule::Manual { steps, idle_timeout }` (`idle_timeout` only for PostgreSQL). The driver, at each `Open` statement, looks at its own state (PostgreSQL's `Tx::block`, MySQL's status flags after the statements before it) and opens a transaction only when none is open: PostgreSQL pipelines `BEGIN` (through `Env::begin`, so read-only and the per-transaction search path still apply) and `SET LOCAL idle_in_transaction_session_timeout` with the statement (no round trip more), MySQL sends `START TRANSACTION` as a request of its own (one more, on the first write only). `ManualBegun` comes once the statement succeeded, before the `Block(true)`; a run that fails in the transaction it opened rolls it back at once (`ManualRolledBack`).
- **Before sending**, `App::tx_check` follows the run statement by statement as the session would (what opens, `BEGIN`/`COMMIT`, MySQL's implicit commits) and refuses (a `no_tx_block` statement while a transaction is open, under either rule; `LOCK TABLES` on MySQL with manual commit), adds reason lines to the run confirmation (`TxReason::OutsideTx`, `ImplicitCommit` with what would be committed, `CommitsItself` for a procedure offered to run once outside a transaction after PostgreSQL refused it as the first write), or adds notes to the run's Messages.
- **Run savepoints** (PostgreSQL): a run in a block the driver opened before the run goes under `SAVEPOINT datarig_run_<n>`, set with its first statement and released with the next run's first (no round trip of its own). A failed or cancelled run goes back to it (`RunRolledBack`) and the block stays usable. Runs with transaction control of their own and the user's `BEGIN` blocks get none (the server's rules). User savepoints named `datarig_…` are refused there (`DbError::ReservedSavepoint`). MySQL sets none: its server undoes only the failed statement, and a deadlock that ends the whole transaction comes as `TxClosedBy { how: ServerEnd::RolledBack }` (an implicit commit as `Committed`, a failed procedure as `Unknown`).
- **Ending it**: `DbCommand::EndTx { commit }` gets exactly one answer: `TxEnded` with `TxOutcome::Committed`, `RolledBack` or `RolledBackInstead` (PostgreSQL answers `COMMIT` of an aborted block with `ROLLBACK`: never shown as committed), or `Failed`: `NoTransaction` (nothing sent), the server's error (a deferred constraint at commit; the block is closed), `CommitUnknown` (the connection ended during a commit; the app stops following the transaction and says the outcome is not known). A session the server ended for an idle transaction (`25P03`) is `Lost` with `DbError::IdleInTransactionTimeout`. Commit and rollback work for the user's own `BEGIN` too.
- **The tab's view** (`app/tx.rs`): `OpenTx` keeps what the open transaction did (statements that may have written and succeeded, their rows, the last `TX_LOG_CAP` of them; a run the session rolled back is taken out), from the run log (`Tab::book_tx`), for the status bar, the summary and the questions on close, disconnect and quit. Its time is shown by the wall clock (it goes on while the machine sleeps) and counted by the app's clock. Nothing of it is persisted, and the app never commits on its own.

## Idle connections and lock waits

- **Two DBMS-neutral settings** (`config::Prefs::idle_disconnect` and `lock_wait`, top level; `policy::Policy::idle_disconnect` / `lock_wait` per environment, `None` meaning the config's), resolved per profile by `App::idle_disconnect_of` / `App::lock_wait_of` when a session opens, and passed to the driver as `ConnectOptions::lock_wait` and `ConnectOptions::server_idle` (the app's idle limit plus `idle::SERVER_IDLE_MARGIN`, query sessions only).
- **Drivers**: PostgreSQL checks in one simple query after connect whether it talks to the server process directly (`pg_backend_pid()` is the id the startup's `BackendKeyData` gave, `Client::backend_key_pid`; a pooler gives an id of its own) and on a direct connection sets `lock_timeout` and, from version 14 on, `idle_session_timeout` for the session (`set_config` in a `CASE`, so nothing is set otherwise); behind a pooler the lock limit joins the search path in the statements each transaction starts with (`query::Settings::each_tx`, `SET LOCAL`), and no idle limit is set. Only a query session sets the lock limit; the metadata session keeps its own `SET LOCAL lock_timeout = '2s'` per read. MySQL adds `lock_wait_timeout`, `innodb_lock_wait_timeout` (query session) and `wait_timeout` (any session given one) to its one `SET` at connect. `55P03` raised from the server's interrupt check (`routine` `ProcessInterrupts`: a lock wait past `lock_timeout`, not `NOWAIT`) and MySQL's 1205 are `DbError::LockTimeout`; `57P05` and MySQL's 4031 are `DbError::IdleSessionTimeout`.
- **Startup SQL** (`core::profile::session`; session presets in 0.31): `SessionPreset` (`[startup.<name>]`, read as `[sessions.<name>]` too: a `Dbms` and a list of statements, an in-memory `SessionPresetId` like an environment's; written by `config::write_sessions`, a table found by the name it was read with), an environment's `Policy::session` (`SessionMap`, a preset per database) and a profile's `startup` (`SessionRef`: none of its own, `"none"`, a name) and `startup_sql` (each key also read under its 0.31 name; `config::STARTUP_ALIASES`). `session::plan` (pure) makes what a profile's query sessions start with, in order: the intents (`lock_wait`, `idle_disconnect`, `tx_idle_rollback`, each with its source), a `Skipped` entry for an environment's preset the profile turned off, the preset's statements, the profile's; each statement checked by `Classifier::session_set` (`sql::risk::session_set`: an allowlist of one `SET` of the session, from PostgreSQL's parse tree or MySQL's tokens in both quote modes, the app's own settings refused with their `Owner`, which says why; an allowed one returns the settings it sets, `SessionSet::settings`) and, read-only, by `Risk::read_only`, with its PostgreSQL `SET LOCAL` form (`OnPooler`: made from its own words and parsed again to the same setting and value, or why it has none). `App::session_plan_of` is checked by `connect`, `start_connect`, `start_test` and before each tab's session opens (`open_query_session`); `open_session` passes `SessionPlan::statements` as `ConnectOptions::session_sql`, query sessions only. PostgreSQL runs them after `apply_limits` (whose directness check also runs for them): each alone on a direct connection; behind a pooler each `SET LOCAL` once in an implicit transaction, then in `query::Settings::each_tx` after the lock limit, with `DbEvent::SessionPerTransaction` naming those that have no such form; when one sets `lock_timeout`, the last such `SET LOCAL` takes the lock limit's place there, so a transaction that leaves the start-up statements out (`CALL`, `DO` wrapped only for the search path, `query::Env::begin_for`) still has the limit the others have. MySQL runs them after its connect `SET`, following the tracked session state, before the read-only check. A failure is `DbError::SessionSql` (index, statement, the server's error) before `Connected`. The session view (`:session` or `:startup`, the profile form's `form.session_view`; `overlay::SessionView`, made by `App::open_session_view`) is built on `plan` too: a snapshot of its items with their sources and `OnPooler` forms. A statement that sets the lock wait (PostgreSQL `lock_timeout`, MySQL `lock_wait_timeout` / `innodb_lock_wait_timeout`) overrides the `lock_wait` intent: `plan` records it as `SessionPlan::lock_wait` (`LockWaitOverride`: the last such statement's source and the value read from it, `session::Wait`, `Unknown` for what it does not read and on MySQL for two limits that differ), the view shows it in the lock wait's row, and `App::effective_lock_wait` gives the tab the value its `DbError::LockTimeout` names (none when not known).
- **The app's timer** (`app/idle.rs`): each tab's session and each profile have a "last used" time (app clock and wall clock; the larger idle time counts, so sleep counts), touched by a run and by every answer of a session. `App::idle_due` is one deadline (the earliest over tabs and profiles; nothing when off, so the idle wakeup budget stays at zero), `App::idle_tick` closes the profile's sessions and tunnel at the profile's deadline and a tab's session at its own, after a warning `IDLE_WARN_LEAD` before for a tab with an open transaction. A profile closed for being idle keeps its resolved password, catalog and keys (`ProfileConn::idle_closed`); `App::connect` uses that password, and a run queues behind the connect like one on a profile not connected.

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
- Tests: `crates/drivers/postgres/tests/pooler_pg.rs` against PgBouncer 1.25 (compose profile
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
- **A cancel that cannot be sent** (PostgreSQL, `route::Cancel::send`, within `CANCEL_DIAL`;
  sent means written and flushed; the stream is then dropped, never shut down, so a shutdown
  that fails or hangs cannot turn a sent cancel into one not sent): the user's
  cancel is bound to the session's current request when it is asked (`route::Unsent`, counted
  by `Link::next`), so a late failure never touches a later request. While that request runs,
  `Link::guard` closes the connection (aborts the task that drives it), the run ends cancelled
  (a count or the allowlist's question answers `Cancelled`, a page fetch fails cancelled), and
  the session reports `Lost { CancelNotTaken(CancelFailure::NotSent(why)) }`; the TUI says it with the reason (`why`
  worded and logged as usual). Closing a session whose own cancel does not go out closes the
  connection too, never leaving it read on in the background. Nothing is known about the
  statement then: it may run to its end and commit outside a transaction block; an open block
  is rolled back when the server notices the closed connection; the session's savepoints,
  temporary tables, settings and advisory locks are gone with it.
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
- **Tunnel presets** (`core::profile::tunnel`): `TunnelPreset` (`[tunnels.<name>]`: the keys of
  `[connections.ssh]` but `enabled`, and a stable `id`, a `TunnelId`, assigned at load and written
  by the launch-time migration like a profile's), its secret under `tunnel:<id>` (a rename keeps
  it). A profile names one with `tunnel = "<name>"`; `tunnel::route` says how it reaches its
  database: `Direct`, `Inline` (its own table, on) or `Preset`, or a `RouteError` (`NotFound`: no
  preset of that name; `Both`: a preset and its own table on), which is an error of that profile
  (its node and status bar say it; the connect, the test and every dial stop there), never a
  direct connection. A malformed preset (its name, a missing key, a duplicate id) is an error of
  the file, as a profile's is. `config::save` finds a preset's table by its id (else the name it
  was read with) and writes it under its current name, comments kept.
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
- **Shared connections** (`app/tunnel/shared.rs`): the profiles of one preset share a
  `SharedTunnel` (`App::shared`, by serial): `Opening(stage)` with the attempts waiting for it
  (`(profile, generation)`), then `Open(handle)` with its `users`. The first attempt opens it
  (`tunnel::Owner::Shared(serial)`: its questions and stages come back as `AppEvent::SharedTunnel`);
  an attempt that comes while it opens waits for the same one (one host key question, one secret
  prompt naming every profile waiting; a secret saved with the prompt's checkbox goes to the
  preset's store once it opened); one that comes while it is open takes it at once. Each profile
  on it holds the handle in `ProfileConn::tunnel` with `shared: Some((serial, preset))`;
  `App::close_tunnel` lets go of it and the last one closes it. An opening no current attempt
  waits for any more is given up (its questions dropped, a late connection closed). Its loss
  (keepalives, closed) marks every profile on it as an own tunnel's loss does, and the preset's
  explorer row says why (`SharedTunnels::lost`) until it opens again. A shared connection keeps
  the settings it opened with: a preset changed while in use opens a new one for the attempts
  that come after (`tunnel_ready` compares the settings and the preset), while the profiles on
  the old one keep it until they connect again (the last one closes it).
- **Presets in the app** (`app/presets.rs`): the explorer's "Tunnels" section (shown when there
  are presets, or profiles and a config file the app can write: `RowKind::TunnelsHeader`,
  `Tunnel`, `TunnelError`, `TunnelUser`, `TunnelsEmpty`), its state per preset
  (`App::preset_state`: closed, opening, open with its profiles, lost). The profile form edits a
  preset too (`FormKind::Tunnel`: its name and the SSH section's bastion fields); a profile form's
  SSH section starts with a picker (`SshChoice`: off, a preset, this profile only). Saving a
  preset writes the config first (a rename moves the profiles that name it along) and then its
  secret: a typed one to its store; a store it left gives its copy to the new one (copied, read
  back, then removed). "Save as tunnel preset" (`form.save_as_tunnel`) makes, when the profile
  form is saved, a preset of the profile's own tunnel: one config write (the preset, and the
  profile naming it without its own table), then the secret moves from `profile:<id>:ssh` to
  `tunnel:<id>` (`source::rekey_copy`, then `rekey_remove`; a store that cannot be read keeps the
  old copy). Deleting a preset asks first (the profiles that name it listed; they keep the name
  and do not connect until another is picked), then writes the config and removes its secret.
  A preset's test (`App::start_preset_test`) is a throwaway tunnel of its settings, then a
  `direct-tcpip` channel opened and closed to each database address of its profiles (at most
  `PROBE_MAX`), nothing sent (`AppEvent::TestProbe`).
- **Test connection**: a throwaway tunnel of the form's settings; the stage while it opens, then
  its time next to the database's result (`TunnelTest`, `test_msg`).
- **Tests**: an SSH server in the test process (`crates/ssh/tests/tunnel.rs`: host keys, every
  login, refused channels, timeouts, keepalive loss, a scratch `ssh-agent`); a real OpenSSH bastion
  (`dev/ssh`, compose profile `ssh`, a CI step; keys made at test time by
  `make-fixture.sh`) in front of PostgreSQL and PgBouncer (`crates/ssh/tests/bastion.rs`,
  `crates/tui/tests/integration_ssh.rs`, where two profiles of one preset go through one TCP
  connection of a counting proxy); the app's flows with a recorded tunnel (`flows_tunnel.rs`,
  `flows_presets.rs`); round trips through the tunnel (`[rtt_ssh]` budgets); `cargo audit`.

## The dialect seam

Every SQL tool takes the dialect of the text it works on, and quoting and value kinds go
through it. The PostgreSQL driver's text is read in the PostgreSQL dialect and the MySQL driver's in
the MySQL dialect (lexed, split, completed, formatted, quoted, classified; see "MySQL text"
and "The MySQL classifier" below).

- **Types** (`datarig_core::sql::dialect`): `Dialect` (`Postgres`, the default, and
  `MySql(MySqlMode)`) is the SQL dialect of a text, and `Language` (`Sql(Dialect)`) the
  language of an editor's text
  (`Language::dialect`). Both are `Copy` enums: every tool will `match` on them, so a new
  variant makes each one decide what to do with it.
- **Who says which**: a driver, through `Capabilities::language` (`PgDriver`:
  `Sql(Postgres)`). `App::tab_language` reads it from the tab's profile's driver without
  connecting; a tab without a profile, or whose profile or driver is unknown, gets
  `Language::default()`. Two more capabilities describe the server rather than its text:
  `Capabilities::hierarchy` (`Hierarchy::DatabaseSchema`, databases holding schemas, as
  PostgreSQL's; `SchemaOnly`, a database that is the schema, as MySQL's) and
  `Capabilities::explain` (`Some(ExplainFormat::PostgresJson)`, `Some(ExplainFormat::MySqlTree)`:
  the plan the driver's sessions produce; `None`: no plan view). A session may say otherwise for
  its server once connected (`DbEvent::Explain`: MySQL's driver says none on MariaDB), kept per
  profile (`ProfileConn::explain`, forgotten at the next attempt). Until a session of the
  profile said it, the driver's holds: before a MariaDB profile's first connect the plan
  actions are offered, and the server refuses `EXPLAIN FORMAT=TREE` with a syntax error
  (nothing runs). `query.explain`,
  `query.explain_analyze` and `results.view_as_plan` are offered only on a tab whose server
  gives one (`App::tab_explain`; a tab without a known driver is in the default language,
  PostgreSQL, and counts as one).
- **The plan format seam**: everything that depends on how a server writes its plans goes
  through `ExplainFormat` (core `sql::plan`): `explain_sql` (how to ask), `holds` (whether a
  one-cell result is a plan), `read` (the reader: `plan::pg`, `plan::mysql`), `plan_in` (both, read once), `says_no_plan` (the server's word that it has no
  plan of the format for a statement), `is_text` (the raw view and the copies show the server's
  text as it came, MySQL's tree, instead of writing the model as `psql` text), `has_buffers`,
  `reports_statement_times`, and `again`/`again_text` (a text `EXPLAIN` asked again
  in the format: `explain::json`, `explain::tree`). The TUI asks the tab's format, never its
  dialect. A new format is a variant, a reader, its fixtures (`fixtures/<flavour>/<version>/`)
  and the session that reports it. Nothing reads `hierarchy` yet: the explorer and the context
  picker show databases holding schemas.
- **Where it goes**: `App::sync_tab_language` pushes the tab's language into its editor
  (`Editor::set_language`, which drops the cached lexer line states when the language changes)
  and gives the tab a new `risk::Classifier` when its language changes. It runs wherever a tab
  appears or its binding changes: a new console, table, DDL or saved-query tab, a tab brought
  back from the closed list or the trash, a restored workspace, `bind_tab_in` and the other
  rebinds, a deleted profile's tabs, and a saved profile (whose driver may have changed).
- **The classifier**: `risk::Classifier` (`Pg(Prepared)`, `MySql(MySqlMode)`) is what the app
  asks about a statement. It is built from a language (`Classifier::new`) and forwards to the
  classifier of that dialect unchanged: PostgreSQL's `classify`, `forget` and `knows` with the
  session's prepared statements and `repeatable`, `ordered` and `count_query` of
  `risk::repeat`; MySQL's of `risk::mysql` (which remembers no prepared statement); and
  `Classifier::classify_once` for a one-off look in a given language. Each tab keeps one for its
  query session (`TabSession::prepared`); a new session gets a new one of the same language.
  The app calls no PostgreSQL classifier function directly; the PostgreSQL driver does.
- **Quoting**: a `Dialect` writes names and strings and reads names back:
  `ident_quote` (the quote character it writes), `ident_quotes` (those it reads; a dialect may
  accept more), `needs_quotes`, `quote_ident` (bare when that reads back as the same name),
  `force_quote_ident` (always quoted), `quote_literal`, `fold` (what the server makes of an
  unquoted name) and `unquote` (a closed quoted name; `unquote_lenient` also takes one still
  being typed, `unescape_ident` the text inside the quotes). In the tab's dialect (the
  classifier's language, `App::tab_dialect`) they write the SQL copies (`export::sql_insert`,
  `sql_update`, `sql_in`, `sql_value` and the writer's `Format::Sql`/`Format::Update` take a
  `Dialect`; `OVERRIDING SYSTEM VALUE` is PostgreSQL's), a table tab's `TableRef::query` and the
  copies' messages, and they read the names in SQL text for the copies (`driver::keys`: the
  copied table, `:copy insert`'s target) and completion's names. Still PostgreSQL only: the DDL and plan
  renderers (`sql::ddl`, `sql::plan::pg`, through the `Dialect::Postgres` wrappers
  `sql::ident::sql_ident` and `export::quote_ident`/`quote_literal`), `sql::plan::explain`, the
  PostgreSQL classifier's own deparse (`risk`) and the PostgreSQL driver's quoting
  (`search_path`, array values). `:use` names are the command's own syntax, not SQL (no keyword
  is quoted): `app::command::context_name` writes them, `command::ident` reads them.
- **Values**: `ColumnMeta::kind` (`driver::ValueKind`: text, integer, decimal, float, bool,
  JSON, bytes, bit, date, time, timestamp, timestamp with time zone, interval, an array of
  `ArrayElement`s, other) says what a column holds whatever the server calls its type; the copy
  (`export::Kind::of_column`, which keeps bytes and bits apart for a dialect's own literals;
  PostgreSQL writes them as text) and the chart's roles read it, never `type_name`, which stays
  for display, except where a database's own type may be named like a built-in one: an `Other`
  column, or one whose type is named like an array, is still read by its type name. The
  PostgreSQL driver maps its types (`values::value_kind`): numbers as `is_numeric` says (a
  domain over a number too), JSON as `is_json`, an array by its built-in element type, and any
  other domain, an array of `box` (its elements are separated by `;`) or of a type of the
  database's own, and every type it does not name as `Other`. Tests over every built-in type,
  a domain and an array of each, and enums, composites and domains named like built-in types
  check that the copy and the chart treat each as they did when they read the type's name.
  `numeric` and `json` stay, derived from the kind (`ColumnMeta::new`).
- **Origins**: `ColumnOrigin` is `Pg { table, column }` (a table's oid and a column's attnum,
  from the RowDescription) or `Named { schema, table, column }` (a driver that names the
  column, as MySQL's column definitions do). `KeyCatalog` finds a `Named` one by name; the
  copies and the key marks take either, so a column's place is known by `schema.table.column`
  on any engine.
- **DDL**: `DdlSource::Verbatim { name, text }` is DDL the server writes itself (MySQL's
  `SHOW CREATE`). `sql::ddl::ddl_text` shows it as it is, without the "reconstructed" header;
  PostgreSQL never sends one.
- **The SQL tools**: each takes the dialect; the function without it is the PostgreSQL wrapper
  the PostgreSQL-only code and the tests call.
  - Lexer: `lexer::lex_in(src, d)` (`lex`), its keywords `Dialect::keywords`/`is_keyword`
    (`lexer::KEYWORDS`/`is_keyword` are PostgreSQL's), `changes_schema_in` (`changes_schema`).
    `lex_backslash_strings` is the PostgreSQL classifier's own. The lexer is a `match` on the
    `Copy` dialect: no allocation or dynamic dispatch on the editor's per-keystroke path.
    A text lexed from somewhere other than its start starts from the lexer's state there:
    `lexer::LexState` (`Copy`: the statement terminator in effect, whether a statement has
    begun, an executable comment open), `LexState::after(token)` carries it over each token,
    and `lex_from(src, d, state)` starts from it. PostgreSQL's is always the default (its
    tokens never depend on text before them); MySQL's client `DELIMITER` makes it matter.
    The editor carries it in its line states (see "The editor" below).
    `Token::ends_statement` is a terminator or a client command line (`Tok::Directive`).
  - Splitter: `split::split_in`, `segment_at_in` (`split`, `segment_at`), and
    `split_from`/`segment_at_from` from a `LexState`; `statement_at` reads statements already
    split.
  - Completion: `complete::complete_in_dialect(src, cursor, catalog, force, path, d)`
    (`complete_in`, `complete`; `complete_from` from a `LexState`): it lexes, offers keywords
    and reads and writes names in `d`.
    The path is `Dialect::default_path(schema)` (PostgreSQL: the schema then `public`, or
    `public`; `complete::default_path`/`schema_path` are its wrappers; MySQL: the session's
    database), `App::tab_path` asks it in the tab's dialect. A relation is found whatever the
    ASCII case of its name; in MySQL one written in the catalog's case comes first.
    `complete::complete_with(…, checks)` also offers values in a string literal compared with a
    column (`=`, `<>`, `!=`, `<=>`, an `IN` list): `ColumnInfo::values` (an enum's labels, which
    the PostgreSQL catalog read gets from `pg_type`/`pg_enum` in the same statement, through
    domains; MySQL's from `COLUMN_TYPE`), then what the table's checks list
    (`values::check_values`, read from a `TableStructure`: `IN`, `= ANY (ARRAY[…])`, `=`, an
    `OR` of them; nothing for another shape). The literal's opening, escaping and reading are
    `Dialect::literal_open`, `literal_body` and `unescape_literal_body` (bit, hex, `U&` and
    dollar strings hold no value). When the checks are not known, `Outcome::wants_checks` says
    so: the app asks `LoadStructure` once (`ChecksCache` per profile and per database, forgotten
    when the catalog is read again or DDL ran in a tab of the profile, in every database of it),
    and redoes the popup when the answer comes only if the tab is bound as before and its cursor
    is in the same literal (`ValuesWait`; typing keeps the wait, moving drops it). A failed read
    leaves the table unknown (`TableChecks::Failed`), asked again after `CHECKS_RETRY` (10 s);
    the explorer's tree takes only a structure that came, or a failure it was waiting for. An
    unqualified column is looked up in the innermost subquery around the literal first; a
    column inside a call gets nothing.
  - Formatter: `format::format_in(src, opts, indent, d)` (`format`): `sqlformat` lays the text
    out as `Dialect::sqlformat_dialect` (PostgreSQL: `PostgreSql`), and the checks lex in `d`.
    Only PostgreSQL masks dollar bodies and keeps `U&`, psql's `:var` and `\` together. MySQL
    text is laid out as `Generic`, a text with a `DELIMITER` line is refused, and a keyword's
    case changes only for a reserved word (a non-reserved one may be a table's name).
  - Generated SQL: what `query.explain` runs is the plan format's (`ExplainFormat::explain_sql`),
    not the dialect's. `plan::is_explain_in` and `ExplainFormat::again`/`again_text` take the
    tab's dialect (a MySQL `EXPLAIN` is read with MySQL's lexer and classifier). The count of a
    query's rows is the classifier's (`Classifier::count_query`), as paging's allowlist is.
  - The editor: every lexer and splitter use (`lexing.rs`: the line states,
    `current_statement`, `completion_context`; the highlighter; `pairs`, `brackets`, `target`,
    `runs`) lexes in the editor's language (`Editor::set_language`). Each cached line state
    (`LineState::Normal(LexState)` or `Inside { line, byte, state }`, where a token that spans
    the line break starts) holds the lexer's state there, folded over the tokens above with
    `LexState::after`, so a region lexed from a line starts with the terminator a `DELIMITER`
    far above set (`Editor::region_text` returns it, `Editor::lex` lexes from it); a Visual
    selection is split from the state where it starts (`Editor::state_at`), and completion
    gets the state of its text (`completion_context`, `complete_from`). `gc` writes
    `Dialect::comment_marker` and strips the marker `Dialect::line_comment_at` finds. The app lexes, splits,
    completes and formats in the tab's dialect (`App::tab_dialect`), as do `driver::keys`'s
    name readers and the schema-change and rollback checks of a run.
  - Still PostgreSQL's own, calling the wrappers: the PostgreSQL driver, the PostgreSQL
    classifier (`risk`, `risk::repeat`), the plan readers (`plan::explain`'s rewrite,
    `plan::pg`) and the DDL renderer (`sql::ddl`).
- **MySQL text** (`Dialect::MySql`; `lexer::mysql`, `ident::mysql`): where a token ends follows
  MySQL's server lexer, and where a statement ends its command-line client (`mysql`), which
  sends one statement at a time; where the client's reading is broken (an optimizer hint over
  several lines, a `DELIMITER` after a statement on its line or after an executable comment,
  where it drops text) the server's is followed. `"…"` is a string unless
  `MySqlMode::ansi_quotes`; a backslash escapes in every string the client tracks (`'…'`,
  `"…"` also under `ANSI_QUOTES`, `x'…'`, `b'…'`, `N'…'`) unless `no_backslash_escapes`;
  `` `…` `` is a quoted name; `#` and `--` followed by a blank or the line's end are line
  comments ending at `\n`; block comments do not nest; `/*!…*/` and `/*!80023 …*/` are
  executable comments, whose opening and closing are `Tok::ExecComment` and whose inside is
  code (the server runs it), `/*+ … */` a comment; `@x`, `@'x'`, `@@x` are `Tok::Variable`,
  `?` a parameter; names may start with a digit (`1col`) unless they are numbers (`1e5`,
  `0x1F`, `0b01`), and after `name.` comes a name (`t.1e5`, `t.select`); dollar quotes only
  when `MySqlMode::dollar_quotes` (a server where `select $$` is a syntax error, as the
  client asks: 8.4 and 9.x). The client's `DELIMITER` line (at a line's start, only blanks and
  same-line comments before it, where no statement has begun; any case; the word after it up
  to a space, a backslash escaping, or between quotes; up to 15 bytes; refused with a
  backslash) is a `Tok::Directive`, never sent, and sets the terminator, which ends a
  statement wherever it starts at an ASCII character outside strings, names in quotes and
  comments (also inside a name, `END$$`, a number or an executable comment, as the client
  does); `\g` and `\G` end one too. The client's other commands (`\c`, `source`, `use`
  without a terminator, …) are not read: such text reaches the server and fails there.
  Keywords for highlighting and completion are MySQL's common ones (without non-reserved
  words that are often names, as `status`); a name is written bare when it is plain ASCII
  (`[A-Za-z_][A-Za-z0-9_$]*`), not reserved in MySQL 8.0, 8.4 or 9.x
  (`ident::mysql::RESERVED`) and no charset introducer (`_binary`), else in backticks; names
  are not folded. `quote_literal` doubles `'` and, unless `NO_BACKSLASH_ESCAPES`, `\` (and
  writes NUL as `\0`): it is right for the session's mode only, so a MySQL driver must give
  the dialect its session's mode and keep it current (`MySqlMode` documents this).
  `crates/core/tests/mysql_split.rs` holds the splitter to MySQL's client and server
  (`DATARIG_TEST_MYSQL_CLIENT`; run against 8.0.45, 8.4.11 and 9.7.2): a script's statements
  are the ones the client sends (also for the quirks above), each runs alone, and literals and
  names the app writes read back as written, also under `NO_BACKSLASH_ESCAPES`.
- **The MySQL classifier** (`risk::mysql`; module docs for the rules): the same `Risk` as
  PostgreSQL's, read from the MySQL lexer's tokens, not a parse tree (see the decision below). Reads
  are an allowlist of forms (queries without `INTO`, a locking clause or a write's keyword inside;
  `SHOW`, `DESCRIBE`, `HELP`, `EXPLAIN` of a query); anything else is a write, DDL, maintenance,
  procedural, a session or transaction statement it knows, or `Danger::Unrecognized`. An executable
  comment (`/*!…*/`, MariaDB's `/*M!…*/`) is `Danger::ExecutableComment`; a text the server would
  read otherwise than the lexer (an unterminated token, a client command, a control character, an
  escape in a quoted name) is unrecognized. A text with a backslash or a `"` is also read in the sql
  modes that read it differently (`NO_BACKSLASH_ESCAPES`, `ANSI_QUOTES`), and the worse reading
  counts unless the server would refuse it as a syntax error; it runs on `risk::THREAD`, where a
  panic reads as unrecognized. MySQL-only dangers: `Locks` (`LOCK TABLES`, `HANDLER`, `FLUSH … WITH
  READ LOCK`), `Privileges`, `Rename`, `Setting` (`risk::mysql::RISKY_SETTINGS`, a client character
  set that is not UTF-8), `DynamicSql` (`PREPARE`), `ServerCommand` (`KILL`, `SET GLOBAL`, `FLUSH`,
  `RESET`, `PURGE`, replication, `INSTALL`, `XA`, …) and `FileAccess` (`LOAD DATA`, `INTO OUTFILE`).
  `Risk::implicit_commit` marks what commits the open transaction first; `Risk::unchecked_call` a
  call of a function that is not a built-in (`risk::mysql::BUILTINS`, from the servers' help
  tables), loadable or stored, whose effects cannot be checked and which a read-only policy refuses
  (`ReadOnlyBlock::UnknownFunction`). The paging allowlist (`repeatable`, `count_query`: `SELECT
  COUNT(*) FROM (…) AS datarig_count`) takes one query with built-in functions not in
  `risk::mysql::VOLATILE` and no variable; whether a name it reads is a view is the driver's to ask
  (`risk::mysql::names`). Tests: the corpus `risk/mysql/corpus.rs` (every statement with its
  expected verdict) and `crates/core/tests/mysql_classify.rs`, which runs the corpus on a
  session whose `transaction_read_only` is on (run against 8.0.45, 8.4.11 and 9.7.2): every read
  runs without error 1792, every write and DDL fails with it unless marked as one the server lets
  through (`FOR SHARE`, `LOCK TABLES … READ`, `HANDLER`, `INTO OUTFILE`, which only the classifier
  stops), and every built-in is a function of the server.
- **A new dialect** adds a `Dialect` variant and, at each `match` the compiler then points to:
  its lexer branch (`lex_in`) and keywords, quoting, folding and identifier quotes, its
  default path, comment markers, `sqlformat` dialect and formatter rules, a
  `Classifier` variant (risk, repeatability, the count query), and a driver whose
  `Capabilities` name its language, hierarchy and plan format.

## Decision: PostgreSQL's parser for safety classification

- **Context**: the lexer-based classifier was fooled wherever the lexer and the server disagreed: `EXPLAIN ("analyze") DELETE` (a quoted option name), a `--` comment ended by a lone `\r`, `x<NBSP>$$` (an identifier to PostgreSQL, a dollar quote to the lexer). Each was a DELETE or UPDATE of every row without a question. Chasing the scanner and the grammar by hand does not end.
- **Decision**: classify from the parse tree of libpg_query (PostgreSQL 17's `gram.y` and `scan.l` built as a C library) through the `pg_query` crate 6.2 (MIT; libpg_query BSD-3-Clause; PostgreSQL code under the PostgreSQL License; protobuf-c and xxHash BSD-2-Clause; see `vendor/README.md`). On every platform, Windows included: pg_query supports Windows since 5.1 and its CI builds on `windows-latest` (MSVC and GNU). The build compiles the C library with `cc` and generates bindings with `bindgen`, which needs libclang (present on the hosted runners and with Xcode; on a developer machine install LLVM). `protoc` is not needed. The tree is walked as JSON (the crate's `serde` serialization), so nested nodes of every kind are visited without a traversal of our own.
- **Costs**: about 3.6 MB more in the release binary (6.0 MB to 9.6 MB when this was decided; the budget is 16 MB, and the binary had since grown to 13.7 MiB on macOS arm64 and 20.7 MiB on Linux x86_64; the release profile's LTO and stripping brought it to 9.5 MiB on macOS arm64), a longer first build (the C library, about 80 s), and libclang at build time. A parse per classified statement (only when a run is checked, never per keystroke).
- **Alternatives rejected**: fixing the lexer only (the next disagreement is the next bypass); a lexer-based fallback on Windows (not needed); asking the server (`PREPARE` of the text needs a connection and is itself a statement).
- **Parse failure**: `Danger::Unparsed`: it always asks, and a read-only policy refuses it ("PostgreSQL's parser cannot read this statement"). A text over the caps is `Danger::TooComplex`, which does the same ("too long or too deeply nested to check"); see "Never a crash in the classifier" above.

## Decision: MySQL's classifier reads tokens, not a parse tree

- **Context**: there is no MySQL equivalent of libpg_query. `sqlparser` 0.63 (Apache-2.0)
  reads MySQL with a tokenizer of its own and rejects ordinary reads (`TABLE t`, `a MOD 2`,
  `FOR SHARE`, `LOCK IN SHARE MODE`, `HELP`), which a read-only policy would then refuse,
  and it added about 2.2 MiB to a release binary.
- **Decision**: read MySQL statements from the tokens of datarig's own MySQL lexer, which its
  tests hold to MySQL's client and server, with an allowlist of the forms that are reads and
  a check that a read holds no write's keyword. In MySQL a query cannot hold a write (no
  data-modifying `WITH`, no DML in a subquery), so a query's effects are its `INTO`, its
  locking clause and its function calls, all visible as tokens. Every verdict is held to the
  server by running the corpus on MySQL 8.0, 8.4 and 9.x with `transaction_read_only` on.
- **Unknown**: a form the classifier does not know, and a text the server would read
  otherwise than the lexer, is `Danger::Unrecognized`: it asks, and a read-only policy refuses
  it ("not a form the safety check knows").

## Decision: a vendored tokio-postgres

- **Context**: the grid and every copy format must show PostgreSQL's own text for every type. The driver decodes the common types from binary and needs the server's text output for the rest (ranges, geometry, bit strings, text search, `money`, `"char"`, `reg*`, enums, composites, extension and unknown types). tokio-postgres 0.7.18 binds every portal with the result format `1` (binary) and exposes no way to choose; the upstream pull request that would, [sfackler/rust-postgres#961](https://github.com/sfackler/rust-postgres/pull/961), has been open since 2023-11.
- **Decision**: vendor tokio-postgres 0.7.18 in `vendor/tokio-postgres`, used through `[patch.crates-io]`, with one additive API, `Transaction::bind_with_formats` (a result format code per column); every existing API behaves as upstream. Each change is marked `datarig:` in the source.
- **Alternatives rejected**: the simple query protocol (text only, no portal paging, no types), casting every column to `text` in the statement (changes the user's SQL and loses the types), and a fork on GitHub (more to maintain than three small patches).
- **Costs**: re-syncing by hand with each upstream release (security fixes included); the workspace excludes `vendor`, so its tests and lints do not run here, while the driver's PG tests (every type compared with `format('%s', …)` and round-tripped through SQL INSERT) cover the change.
- **Pipelining**: the copy also pipelines a portal's first page (`[BEGIN] + Bind + Describe + Execute + Sync` in one write, `+ COMMIT` before the `Sync` when the result is not held) and a statement without rows (`Parse + Describe + Bind + Execute + Sync`), so a first page costs one round trip (two for a statement new to the session) and DML one. The driver keeps the session's prepared row-returning statements by text (64) and prepares again after the server refuses one whose result changed, or inside the user's block. Alternatives rejected: parsing the socket ourselves (a "socket tap"), a second fork, and holding the implicit transaction with `Flush` instead of `BEGIN` (no fewer round trips, one more for a large result, and the statement timeout would keep counting while the user reads, since it stops only at `Sync`).
- **Exit**: drop the copy once a tokio-postgres release lets a caller choose per-column result formats and pipeline a portal's first page. Provenance (checksum, upstream commit), the exact change list, the re-sync steps and the exit criteria are in [`vendor/README.md`](../vendor/README.md).

## Failures as data

- **Core never words a failure.** `datarig_core::fault::Fault` is a kind (`io::ErrorKind`, a TOML error with its line, a key of the wrong shape, one of the keychain failures, or other) plus the raw detail. Core error types carry it (`workspace::Loaded::broken`, `scripts::IndexProblem`, `secret::Unavailable`/`StoreError`/`FileError::Io`/`SourceError::Keychain`, `CommandError::Start`, `SwitchError::Config`, `migrate::Report`, `config::save`). `config::load` returns a `ConfigError` enum (read, syntax, version, a bad value with its key, profile and allowed values, duplicate id, missing key). The driver reports `driver::DbError`: the server's own message (data, shown verbatim), `Closed`, `NoAnswer`, `Settings`, `Connection` (a `Fault` with the io kind found in the error chain) and `NotSupported`.
- **The TUI words them.** `app::fault_reason` maps a fault to a short catalog reason, `config_error_msg` and `App::db_error_text` do the same for config and database failures, and `App::fault_text` also appends the raw detail to `<state>/errors.log` (`fault::ErrorLog`, one line per failure). The status line never shows OS, parser, keychain or driver text; only database server messages are shown as they are.

## UI strings (i18n)

Every piece of UI chrome text (titles, labels, hints, status and error messages, action and key-context names) comes from the locale catalogs, and the compiler enforces it.

- **Source of truth**: `locales/en.toml` defines the keys. `crates/core/build.rs` checks every catalog listed in its `LOCALES` table and generates `OUT_DIR/messages.rs`, which `datarig_core::i18n` includes:
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
2. Add `("<code>", Policy::Required)` (or `Policy::Optional` for a partial translation) to `LOCALES` in `crates/core/build.rs`.
3. Add the matching `Lang` variant (`ja` → `Lang::Ja`) in `crates/core/src/i18n.rs` and its detection in `detect_lang`; the generated `match lang` does not compile until the variant exists. Then add the language setting and actions in the TUI (`LangSetting`, `action.ui.language.*`).


## Adding a driver crate (for example MySQL)

1. Create `crates/drivers/mysql/` with `datarig-core.workspace = true` and the database client crate in its `Cargo.toml` (versions go to the root `[workspace.dependencies]`), and `[lints] workspace = true`.
2. Add it to `members` and to `[workspace.dependencies]` (`datarig-driver-mysql = { path = "crates/drivers/mysql" }`) in the root `Cargo.toml`.
3. Implement `datarig_core::driver::Driver`:
   - `capabilities()`: turn on only what the driver really supports (`server_paging`, `cancel`, `introspection`, `contexts`, `key_metadata`, ...), and name the `language` of its sessions.
   - `connect(cfg, role, opts, events)`: for each role (`SessionRole::Meta` for the tree and completion, `SessionRole::Query` for a tab's statements) start a background task with **one connection** and return `Session::new(caps, role, tx, canceller)`. Report `opts.application_name` to the server. Progress and results go out as `DbEvent`s (`Connected` / `ConnectFailed`, `Schemas`, `Objects`, `Catalog`, `Page`, `Done`, `Failed`, `TxOpen`). A metadata session answers `Execute` with `Failed`. With `Capabilities::structure` the metadata session also answers `LoadStructure` with a `driver::structure::TableStructure` (`DbEvent::Structure`), which the explorer shows under an open table.
   - `ping()`: the test connection. It honours the time limit and cancellation (the future being dropped).
   - Export the driver type only (`pub use connect::MySqlDriver;`).
4. Add the dependency to `datarig-tui`'s `Cargo.toml` and register the name in `driver_for` in `src/drivers.rs` (`"mysql" | "mariadb" => Some(Box::new(MySqlDriver))`). No UI code changes.
5. Tests: unit tests such as value decoding go to `src/<module>/tests.rs`, real-database tests to `crates/drivers/mysql/tests/`. Take the connection URL from an environment variable, print `SKIPPED` without it, and fail when the matching `DATARIG_REQUIRE_*` variable is set (as `integration_pg.rs` does with `DATARIG_REQUIRE_PG`). Add a service container to the CI `integration` job.
6. If core seems to need driver-specific code (a dialect's statement splitting, say), do not branch on the driver's name: add a trait or a capability to core and let the driver implement it.
