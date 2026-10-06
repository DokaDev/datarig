# datarig key bindings

<!-- Generated from crates/datarig-tui/src/keymap/defaults.rs and the action registry. Do not edit;
     run `DATARIG_BLESS=1 cargo test -p datarig-tui --test keybindings_doc` at the repository root. -->

Keys resolve from the current context outwards to `root`; the first binding found wins. Dialogs (`overlay.*`) sit directly under `root`, so only `root` keys pass through them. The cell viewer is not modal: it sits under `workspace`, so the workspace keys (run, …) keep working.

- **[text]** text input: letters, digits, symbols and `Space` are typed. No leader, no Hangul mapping.
- **[editor]** the query editor: its keys may hide keys of the contexts around it, except protected keys.
- **Hangul**: outside text input a Korean 2-set jamo or syllable counts as the QWERTY key(s) at its place (U+3153 = `j`, U+D558 = `g k`); the status bar then shows the Korean input-source mark. The character vim's `f t F T r` wait for is taken as typed.
- **R**: repeats while the key is held (terminal auto-repeat).
- **Key guide**: after `Space` a which-key popup lists the keys that may follow (after 300 ms; `Backspace` goes up a level, `Esc` closes). `Space ?` or `F1` open the keyboard help, the same way everywhere (`?` alone is left to vim's backward search). The help is one list: the sections of the current context first and open, every other context closed below (`Enter`/`l`/`h` or a click opens and closes a section); `/` searches all of them. The status bar shows the most relevant keys of the current context.
- **Commands**: `:` (outside text input) or `Ctrl+K` (everywhere) opens the command line: a popup near the top of the screen, or the last line with `[commands] position = "bottom"` (the settings screen, `:set commands.position=bottom`). It runs the commands listed at the end (`:conn <profile>`, `:set <setting>=<value>`, …) and finds every action by name; see the tables at the end.
- **Vim**: the editor has vim keys. `i` starts typing and `Esc` stops (the hint line says so). Implemented, all with counts: the motions `h j k l w b e W B E ge gE 0 ^ $ gg G`, `f F t T ; ,` (the character is taken as typed, Hangul included), `%` (the matching bracket of `( ) [ ] { }`, brackets in strings and comments left out; `50%` goes to the middle line), `{ }`, `H M L`, the arrows, `Home` and `End`; the operators `d c y`, `gu gU g~` (case) and `> <` (indent by 4 columns, written with spaces as `Tab` types them) with any motion or text object, doubled for whole lines (`dd cc yy guu gUU g~~ >> <<`), counts before and after (`3dd`, `d2w`, `2d3w`); the text objects `iw aw iW aW`, `i( a(` (`ib ab`), `i[ a[`, `i{ a{` (`iB aB`), `i< a<`, `i" a" i' a'`, ``i` a` `` and `ip ap`; `x X s S D C Y p P r J gJ ~ u Ctrl+R`; `.` (the last change again, what was typed in Insert mode included; `3.` with a new count, except for a Visual mode operator, which keeps its own as in Vim); `i a I A o O` (a count types the text that many times); scrolling with `Ctrl+D Ctrl+U Ctrl+F Ctrl+B zz zt zb`; search with `/` and `?` (a prompt on the editor's last line: the cursor shows the match while the pattern is typed, `Enter` searches, `Esc` or any key of the app goes back, `Ctrl+W` deletes a word; an empty pattern takes the last one), `n` `N`, `*` `#` (the word under the cursor, as a whole word) and `g*` `g#` (also inside longer words), with counts and after an operator (`d/from` `Enter`, `yn`: up to the match, without it; a match at the end of a line stops on its last character, as in Vim); the matches on screen stay highlighted until `:nohlsearch` (`:noh`) and the next search shows them again. Search patterns are Rust regular expressions (<https://docs.rs/regex/latest/regex/#syntax>), not Vim's: case-sensitive (`(?i)` at the start ignores case), `\b` for a word boundary (Vim's `\<` `\>`), `.` `*` `+` `?` `(` `)` `|` without a backslash; a match lies within one line, and search offsets (`/foo/e`) are not supported; marks: `m{a-z}` sets one at the cursor, `'{a-z}` goes to its line (the first non-blank) and `` `{a-z} `` to its place, also after an operator (`d'a` takes whole lines, `` y`a `` the characters up to it); `''` and ``` `` ``` go back to where the last jump left from (`G`, `gg`, `%`, `{`, `}`, `H`, `M`, `L`, a search, a mark; `m'` sets it), `'<` `'>` (`` `< `` `` `> ``) to the ends of the last Visual selection and `'.` to the last change; marks move with their lines as lines are added or deleted above them and go with a deleted line, an undo puts them back, and `'A`-`'Z`, `'0`-`'9`, `'[` `']` `'^` are not kept (they say so); `gc` comments lines out with `-- ` or back in, as Neovim's built-in commenting does: `gcc` (with a count), `gc` with a motion or text object and `gc` in Visual mode take whole lines; when every line that is not blank starts with `--` they lose it, otherwise each gets `-- ` after the smallest indent (a blank line `--`), and `.` repeats it (the action `editor.comment_toggle`, `Space e c`, is the same as `gcc`, or `gc` in Visual mode); editor commands on the `:` line (see the end); `&` runs the last `:s` again on the cursor's line with its flags (as Neovim's `&`, Vim's `:&&`) and `g&` on every line with the last search pattern; Visual mode by character (`v`) and by line (`V`) with the motions and text objects, `y d x c`, `Y D X C S` (whole lines), `r J gJ u U ~ > <`, `o` (the other end) and `p P` (a register in place of the selection; `P` keeps the replaced text out of the registers); Visual mode by block (`Ctrl+V`; `v`, `V` and `Ctrl+V` switch between the three, the same key again leaves): screen columns, a wide character or a tab partly inside handled as Vim does, `$` to the end of every line, `o` and `O` (the other corners), `y Y d x X D` (`D` to the ends of the lines), `c s C`, `I` and `A` (what Insert mode types goes on every line of the block when it ends; `A` fills short lines with blanks first, `$A` appends at each line's end; a count types it that many times), `r` (`r Enter` breaks the lines), `~ u U gu gU g~`, `> <` with a count, `J gJ`, `S R` (whole lines) and `p P` (a block register as a block, lines below or above, text of one line on every line of the block); `.` repeats a block operator on a block of the same size from the cursor (after `p` only the delete); a paste from the terminal replaces the block; in Insert mode `Ctrl+W` (the word before the cursor), `Ctrl+U` (the line before the cursor), `Ctrl+R {register}` (its text) and `Enter` keeping the indent; with `[editor] auto_pairs = "on"` (off by default, as Vim types) `( [ { ' " `` ` `` also put their closing character after the cursor, typing it there steps over it and `Backspace` deletes the empty pair (only pairs it put in; never inside strings, quoted names, comments or dollar bodies, before a word, or for a paste; `.` repeats what it did). One command is one undo step. Registers as in Vim: `"x` before a command in Normal and Visual mode (`"ayy`, `2"ap`), the unnamed one, `"a`-`"z` (`"A`-`"Z` append), `"0` (the last yank), `"1`-`"9` (the last deletes of lines; `.` after `"1p` puts `"2`), `"-` (small deletes), `"_` (nothing kept), `".` (the text last typed in Insert mode), `"/` (the last search pattern) and `"+` `"*` (the system clipboard); `":` `"%` `"#` are not kept (a put from them says the register is empty). As in Vim with `clipboard=unnamedplus`, a yank, delete or change without a register also goes to the system clipboard, through the `clipboard` setting's way (`[editor] clipboard = "off"` keeps them in the editor); `"a` and `"_` never do. `"+p` reads the system clipboard only when typed, and not over OSC 52 (the terminal's own paste works there). A paste from the terminal goes in at the cursor in every mode (over the selection in Visual mode). Unlike Vim, a tab is 4 columns wide and a completion taken from the popup is not part of what `.` repeats. The other reserved vim keys do nothing yet.
- **Your editor and suspending**: `Ctrl+G` opens the tab's query in your own editor (`$VISUAL`, else `$EDITOR`, else `vi`; split into words like a shell does, `code -w` included, but no shell runs it) on a private file in the state directory, removed afterwards. What it saves replaces the text as one undo step; quitting without saving changes nothing, and an editor that fails (Vim's `:cq`) leaves the text as it was. A table tab's query goes as a copy, and an edited copy opens in a new console. `Ctrl+Z` (and `:suspend`, `:sus`, `:stop`) stops datarig as `Ctrl+Z` stops Vim; the shell's `fg` brings it back (Unix only; on Windows `:suspend` says it is not supported). `Ctrl+C` stays the query cancel. Meanwhile a running query goes on on the server: with the editor open its results arrive and wait; while suspended the whole program is stopped, so they are read after `fg`.
- **Reserved** keys belong to a widget (vim, a dialog) or to an action that is not implemented yet. They cannot be remapped.

Protected keys (no inner context may hide them): `Ctrl+Q` `Ctrl+K` `Ctrl+E` `Ctrl+Enter` `Ctrl+S` `Ctrl+T` `Ctrl+O` `Ctrl+G` `F1` `F6` `Shift+F6` `Ctrl+PageDown` `Ctrl+PageUp` `Space`.

Remap in `config.toml` with `[keymap.<context>]` tables: `"<keys>" = "<action id>"`, or `"none"` to unbind. Key notation: `ctrl+e`, `shift+tab`, `f4`, `pagedown`, `space c n` (a space separates the keys of a sequence), `G` = `shift+g`. Invalid entries are skipped and reported at startup.

```toml
[keymap.explorer]
"x" = "explorer.refresh"
"q" = "none"
```

## `root`

Works everywhere, also while a dialog is open.

| Keys | Action | Description | R |
|---|---|---|---|
| `Ctrl+Q` | `app.quit` | Quit |  |
| `Ctrl+C` | `query.cancel` | Cancel running query |  |
| `Ctrl+K` | `commands.open` | Commands |  |

## `workspace`

The workspace without a dialog, including text input in the editor. Inside `root`.

| Keys | Action | Description | R |
|---|---|---|---|
| `Ctrl+Enter` | `query.execute_current` | Run statement under cursor |  |
| `Ctrl+E` | `query.execute_current` | Run statement under cursor |  |
| `Ctrl+O` | `conn.quick_connect` | Quick connect (go to a profile's tab) |  |
| `Ctrl+G` | `editor.open_external` | Edit the query in your own editor ($VISUAL, $EDITOR) |  |
| `Ctrl+Z` | `app.suspend` | Suspend datarig (the shell's fg brings it back) |  |
| `Ctrl+T` | `tab.new_console` | New console tab |  |
| `Ctrl+W` | `tab.close` | Close tab |  |
| `Ctrl+S` | `script.save` | Save query |  |
| `Ctrl+PageDown` | `tab.next` | Next tab | ✓ |
| `Ctrl+PageUp` | `tab.prev` | Previous tab | ✓ |
| `F6` | `pane.next` | Focus next pane |  |
| `Shift+F6` | `pane.prev` | Focus previous pane |  |
| `Shift+Tab` | `pane.prev` | Focus previous pane |  |
| `F1` | `help.context` | Keyboard help |  |

## `nav`

Every pane that is not text input (explorer, results, vim Normal/Visual). Inside `workspace`.

| Keys | Action | Description | R |
|---|---|---|---|
| `Tab` | `pane.next` | Focus next pane |  |
| `:` | `commands.open` | Commands |  |
| `g t` | `tab.next` | Next tab | ✓ |
| `g T` | `tab.prev` | Previous tab | ✓ |
| `Space t n` | `tab.new_console` | New console tab |  |
| `Space t c` | `tab.close` | Close tab |  |
| `Space t u` | `tab.reopen_closed` | Reopen closed tab |  |
| `Space 1` | `tab.goto.1` | Go to tab 1 |  |
| `Space 2` | `tab.goto.2` | Go to tab 2 |  |
| `Space 3` | `tab.goto.3` | Go to tab 3 |  |
| `Space 4` | `tab.goto.4` | Go to tab 4 |  |
| `Space 5` | `tab.goto.5` | Go to tab 5 |  |
| `Space 6` | `tab.goto.6` | Go to tab 6 |  |
| `Space 7` | `tab.goto.7` | Go to tab 7 |  |
| `Space 8` | `tab.goto.8` | Go to tab 8 |  |
| `Space 9` | `tab.goto.9` | Go to tab 9 |  |
| `Space c c` | `conn.quick_connect` | Quick connect (go to a profile's tab) |  |
| `Space c n` | `conn.new` | New connection profile |  |
| `Space c e` | `conn.edit_current` | Edit this tab's connection profile |  |
| `Space c t` | `conn.test_current` | Test this tab's connection |  |
| `Space c x` | `conn.disconnect_current` | Disconnect this tab's connection |  |
| `Space c r` | `conn.reconnect_current` | Reconnect this tab's connection |  |
| `Space c s` | `tab.set_connection` | Change this tab's connection |  |
| `Space c d` | `tab.set_context` | Choose this tab's database and schema |  |
| `Space s s` | `script.save` | Save query |  |
| `Space s a` | `script.save_as` | Save query as… |  |
| `Space s r` | `script.rename` | Rename this saved query |  |
| `Space s o` | `script.open` | Open a saved query |  |
| `Space s d` | `script.delete` | Delete this saved query |  |
| `Space e f` | `editor.format` | Format the statement under the cursor or the selection (only its layout) |  |
| `Space e c` | `editor.comment_toggle` | Comment the line or the selected lines out with -- (or back in) |  |
| `Space e x` | `query.explain` | Explain: the statement's plan (nothing runs) |  |
| `Space e a` | `query.explain_analyze` | Explain analyze: run and measure the statement (writes rolled back) |  |
| `Space Space` | `menu.open` | Action menu |  |
| `Shift+F10` | `menu.open` | Action menu |  |
| `Menu` | `menu.open` | Action menu |  |
| `Space t m` | `tab.menu` | Tab menu |  |
| `Space z` | `pane.zoom` | Toggle pane zoom |  |
| `Space b` | `explorer.toggle` | Toggle explorer |  |
| `Space >` | `explorer.wider` | Widen explorer | ✓ |
| `Space <` | `explorer.narrower` | Narrow explorer | ✓ |
| `Space ,` | `settings.open` | Settings |  |
| `Space /` | `commands.open` | Commands |  |
| `Space ?` | `help.context` | Keyboard help |  |
| `Space r y t` | `results.copy.selection.tsv` | Copy selection: Without headers (TSV) |  |
| `Space r y T` | `results.copy.selection.tsv_header` | Copy selection: With headers (TSV) |  |
| `Space r y l` | `results.copy.selection.list` | Copy selection: Comma list |  |
| `Space r y c` | `results.copy.selection.csv` | Copy selection: CSV |  |
| `Space r y j` | `results.copy.selection.json` | Copy selection: JSON |  |
| `Space r y J` | `results.copy.selection.json_pretty` | Copy selection: JSON (pretty) |  |
| `Space r y m` | `results.copy.selection.markdown` | Copy selection: Markdown |  |
| `Space r y h` | `results.copy.selection.html` | Copy selection: HTML table |  |
| `Space r y x` | `results.copy.selection.xml` | Copy selection: XML |  |
| `Space r y n` | `results.copy.selection.in` | Copy selection: SQL IN clause |  |
| `Space r y i` | `results.copy.selection.insert` | Copy selection: SQL INSERT |  |
| `Space r y u` | `results.copy.selection.update` | Copy selection: SQL UPDATE |  |
| `Space r a t` | `results.copy.all.tsv` | Copy all fetched rows: Without headers (TSV) |  |
| `Space r a T` | `results.copy.all.tsv_header` | Copy all fetched rows: With headers (TSV) |  |
| `Space r a l` | `results.copy.all.list` | Copy all fetched rows: Comma list |  |
| `Space r a c` | `results.copy.all.csv` | Copy all fetched rows: CSV |  |
| `Space r a j` | `results.copy.all.json` | Copy all fetched rows: JSON |  |
| `Space r a J` | `results.copy.all.json_pretty` | Copy all fetched rows: JSON (pretty) |  |
| `Space r a m` | `results.copy.all.markdown` | Copy all fetched rows: Markdown |  |
| `Space r a h` | `results.copy.all.html` | Copy all fetched rows: HTML table |  |
| `Space r a x` | `results.copy.all.xml` | Copy all fetched rows: XML |  |
| `Space r a n` | `results.copy.all.in` | Copy all fetched rows: SQL IN clause |  |
| `Space r a i` | `results.copy.all.insert` | Copy all fetched rows: SQL INSERT |  |
| `Space r a u` | `results.copy.all.update` | Copy all fetched rows: SQL UPDATE |  |
| `Space r i` | `results.detail` | Result detail → show / hide |  |
| `Space r I` | `results.detail_tab` | Result detail → Cell / Row |  |
| `Space r h` | `results.panel.toggle` | Results pane → hide / show |  |
| `Space r z` | `results.panel.maximize` | Results pane → zoom (maximise) / restore |  |
| `Space r +` | `results.panel.grow` | Results pane → taller | ✓ |
| `Space r -` | `results.panel.shrink` | Results pane → shorter | ✓ |
| `Space r n` | `results.page.next` | Results → next page | ✓ |
| `Space r p` | `results.page.prev` | Results → previous page | ✓ |
| `Space r #` | `results.count` | Results → count every row (runs a count query) |  |
| `Space r ]` | `results.tab.next` | Result tabs → next |  |
| `Space r [` | `results.tab.prev` | Result tabs → previous |  |

Leader groups (the which-key popup lists what follows):

| Keys | Group |
|---|---|
| `Space` | Leader |
| `Space c` | Connection |
| `Space t` | Tabs |
| `Space s` | Saved queries |
| `Space r` | Results |
| `Space r y` | Copy selection |
| `Space r a` | Copy all fetched rows |
| `Space e` | Editor |

## `explorer`

The explorer: folders, connection profiles, their databases and schema trees. Inside `nav`.

| Keys | Action | Description | R |
|---|---|---|---|
| `j` | `explorer.down` | Explorer: next item | ✓ |
| `Down` | `explorer.down` | Explorer: next item | ✓ |
| `k` | `explorer.up` | Explorer: previous item | ✓ |
| `Up` | `explorer.up` | Explorer: previous item | ✓ |
| `l` | `explorer.expand` | Explorer: expand |  |
| `Right` | `explorer.expand` | Explorer: expand |  |
| `h` | `explorer.collapse` | Explorer: collapse or go to parent |  |
| `Left` | `explorer.collapse` | Explorer: collapse or go to parent |  |
| `Enter` | `explorer.activate` | Explorer: open or toggle |  |
| `g g` | `explorer.top` | Explorer: first item |  |
| `G` | `explorer.bottom` | Explorer: last item |  |
| `r` | `explorer.refresh` | Explorer: reload |  |
| `/` | `explorer.filter` | Explorer: filter profiles |  |
| `n` | `conn.new` | New connection profile |  |
| `e` | `conn.edit` | Edit connection profile |  |
| `c` | `conn.duplicate` | Duplicate connection profile |  |
| `d` | `explorer.delete` | Explorer: delete profile or empty folder |  |
| `t` | `conn.test` | Test connection |  |
| `x` | `conn.disconnect` | Disconnect |  |
| `o` | `conn.open_console` | New console on this connection |  |
| `m` | `explorer.move` | Explorer: move profile to a folder |  |
| `N` | `folder.new` | New folder |  |
| `R` | `explorer.rename` | Explorer: rename folder |  |
| `O` | `explorer.new_console_here` | Explorer: new console in this database or schema |  |
| `D` | `explorer.show_ddl` | Explorer: show the DDL of the table, view, index or trigger |  |
| `F` | `explorer.show_function_ddl` | Explorer: show the DDL of the trigger's function |  |
| `Esc` | `explorer.back` | Explorer: back (cancel a test or connecting, clear the filter) |  |
| `q` | `app.quit` | Quit |  |

## `explorer.filter` [text]

The `/` filter of the explorer (profile names). Inside `workspace`.

| Keys | Action | Description | R |
|---|---|---|---|
| `Esc` | `explorer.filter_clear` | Explorer filter: clear |  |
| `Enter` | `explorer.filter_accept` | Explorer filter: done |  |
| `Down` | `explorer.down` | Explorer: next item | ✓ |
| `Up` | `explorer.up` | Explorer: previous item | ✓ |

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `grid`

The result grid. Inside `nav`.

| Keys | Action | Description | R |
|---|---|---|---|
| `h` | `grid.left` | Results: previous column | ✓ |
| `Left` | `grid.left` | Results: previous column | ✓ |
| `j` | `grid.down` | Results: next row | ✓ |
| `Down` | `grid.down` | Results: next row | ✓ |
| `k` | `grid.up` | Results: previous row | ✓ |
| `Up` | `grid.up` | Results: previous row | ✓ |
| `l` | `grid.right` | Results: next column | ✓ |
| `Right` | `grid.right` | Results: next column | ✓ |
| `PageDown` | `grid.page_down` | Results: scroll one screen down | ✓ |
| `PageUp` | `grid.page_up` | Results: scroll one screen up | ✓ |
| `Ctrl+D` | `grid.half_down` | Results: scroll half a screen down | ✓ |
| `Ctrl+U` | `grid.half_up` | Results: scroll half a screen up | ✓ |
| `g g` | `grid.top` | Results: first row of the page |  |
| `Home` | `grid.top` | Results: first row of the page |  |
| `G` | `grid.bottom` | Results: last row of the page |  |
| `End` | `grid.bottom` | Results: last row of the page |  |
| `0` | `grid.first_col` | Results: first column |  |
| `$` | `grid.last_col` | Results: last column |  |
| `Enter` | `grid.view_cell` | Results: view cell value |  |
| `y` | `grid.copy_cell` | Copy the cell (a selected range as TSV) |  |
| `Y` | `grid.copy_row` | Copy the row (the selected rows) as TSV |  |
| `v` | `grid.select` | Select a range of cells (again: cancel) |  |
| `V` | `grid.select_rows` | Select whole rows (again: cancel) |  |
| `i` | `results.detail` | Result detail → show / hide |  |
| `I` | `results.detail_tab` | Result detail → Cell / Row |  |
| `z` | `results.panel.maximize` | Results pane → zoom (maximise) / restore |  |
| `+` | `results.panel.grow` | Results pane → taller | ✓ |
| `-` | `results.panel.shrink` | Results pane → shorter | ✓ |
| `n` | `results.page.next` | Results → next page | ✓ |
| `p` | `results.page.prev` | Results → previous page | ✓ |
| `#` | `results.count` | Results → count every row (runs a count query) |  |
| `L` | `results.tab.next` | Result tabs → next |  |
| `H` | `results.tab.prev` | Result tabs → previous |  |
| `Esc` | `pane.back` | Back to the editor |  |
| `q` | `pane.back` | Back to the editor |  |

## `inspector`

The result inspector next to the grid, after a click on it. Inside `nav`.

| Keys | Action | Description | R |
|---|---|---|---|
| `Tab` | `results.detail_tab` | Result detail → Cell / Row |  |
| `Shift+Tab` | `results.detail_tab` | Result detail → Cell / Row |  |
| `I` | `results.detail_tab` | Result detail → Cell / Row |  |
| `i` | `results.detail` | Result detail → show / hide |  |
| `Esc` | `pane.back` | Back to the editor |  |
| `q` | `pane.back` | Back to the editor |  |

## `plan`

The Plan tab of the results pane (an `EXPLAIN (FORMAT JSON)` result): `j`/`k` select a node, `h`/`l` close and open its children, `Enter` shows its detail, `v`/`V` and the digits pick a view, `y`/`Y` copy the plan as text or JSON. Inside `nav`.

| Keys | Action | Description | R |
|---|---|---|---|
| `j` | `plan.down` | Plan: next node | ✓ |
| `Down` | `plan.down` | Plan: next node | ✓ |
| `k` | `plan.up` | Plan: previous node | ✓ |
| `Up` | `plan.up` | Plan: previous node | ✓ |
| `g g` | `plan.top` | Plan: first node |  |
| `Home` | `plan.top` | Plan: first node |  |
| `G` | `plan.bottom` | Plan: last node |  |
| `End` | `plan.bottom` | Plan: last node |  |
| `PageDown` | `plan.page_down` | Plan: a page down | ✓ |
| `Ctrl+D` | `plan.page_down` | Plan: a page down | ✓ |
| `PageUp` | `plan.page_up` | Plan: a page up | ✓ |
| `Ctrl+U` | `plan.page_up` | Plan: a page up | ✓ |
| `l` | `plan.expand` | Plan: show the node's children (or go to the first) |  |
| `Right` | `plan.expand` | Plan: show the node's children (or go to the first) |  |
| `h` | `plan.collapse` | Plan: hide the node's children (or go to its parent) |  |
| `Left` | `plan.collapse` | Plan: hide the node's children (or go to its parent) |  |
| `Enter` | `plan.detail` | Plan: show or hide the selected node's detail |  |
| `i` | `plan.detail` | Plan: show or hide the selected node's detail |  |
| `v` | `plan.view.next` | Plan: next view |  |
| `V` | `plan.view.prev` | Plan: previous view |  |
| `1` | `plan.view.tree` | Plan view: tree |  |
| `2` | `plan.view.summary` | Plan view: summary cards |  |
| `3` | `plan.view.icicle` | Plan view: icicle (width = time) |  |
| `4` | `plan.view.flame` | Plan view: flame graph (width = time) |  |
| `5` | `plan.view.timeline` | Plan view: timeline (to the first and last row, needs ANALYZE) |  |
| `6` | `plan.view.rows` | Plan view: row flow (rows between nodes) |  |
| `7` | `plan.view.treemap` | Plan view: treemap (area = own time) |  |
| `8` | `plan.view.boxes` | Plan view: box diagram |  |
| `9` | `plan.view.raw` | Plan view: raw text (as psql shows it) |  |
| `<` | `plan.pan_left` | Plan: move the view left | ✓ |
| `>` | `plan.pan_right` | Plan: move the view right | ✓ |
| `y` | `plan.copy_text` | Plan: copy as text (as psql shows it) |  |
| `Y` | `plan.copy_json` | Plan: copy its JSON |  |
| `z` | `results.panel.maximize` | Results pane → zoom (maximise) / restore |  |
| `+` | `results.panel.grow` | Results pane → taller | ✓ |
| `-` | `results.panel.shrink` | Results pane → shorter | ✓ |
| `L` | `results.tab.next` | Result tabs → next |  |
| `H` | `results.tab.prev` | Result tabs → previous |  |
| `Esc` | `pane.back` | Back to the editor |  |
| `q` | `pane.back` | Back to the editor |  |

## `welcome`

The welcome panel shown while there is no connection profile. Inside `nav`.

| Keys | Action | Description | R |
|---|---|---|---|
| `n` | `conn.new` | New connection profile |  |
| `Enter` | `conn.new` | New connection profile |  |
| `q` | `app.quit` | Quit |  |

## `editor.vim.normal` [editor]

Query editor, vim Normal mode (`i` starts typing). Inside `nav`.

Reserved — vim: `h` `j` `k` `l` `w` `W` `b` `B` `e` `E` `g e` `g E` `0` `^` `$` `g g` `G` `f` `F` `t` `T` `;` `,` `%` `{` `}` `(` `)` `[` `]` `H` `M` `L` `Left` `Right` `Up` `Down` `Home` `End` `d` `c` `y` `>` `<` `=` `g ~` `g u` `g U` `1` `2` `3` `4` `5` `6` `7` `8` `9` `.` `"` `x` `X` `D` `C` `Y` `s` `S` `r` `R` `p` `P` `u` `Ctrl+R` `J` `g J` `~` `i` `a` `I` `A` `o` `O` `v` `V` `Ctrl+V` `Esc` `/` `?` `n` `N` `*` `#` `g *` `g #` `m` `'` `` ` `` `g c` `&` `g &` `Ctrl+D` `Ctrl+U` `Ctrl+F` `Ctrl+B` `z z` `z t` `z b` `z Enter` `q`

## `editor.vim.visual` [editor]

Query editor, vim Visual mode (`v` by character, `V` by line, `Ctrl+V` by block). Inside `nav`.

Reserved — vim: `h` `j` `k` `l` `w` `W` `b` `B` `e` `E` `g e` `g E` `0` `^` `$` `g g` `G` `f` `F` `t` `T` `;` `,` `%` `{` `}` `(` `)` `[` `]` `H` `M` `L` `Left` `Right` `Up` `Down` `Home` `End` `d` `c` `y` `>` `<` `=` `g ~` `g u` `g U` `1` `2` `3` `4` `5` `6` `7` `8` `9` `.` `"` `x` `X` `D` `C` `Y` `s` `S` `r` `R` `p` `P` `u` `Ctrl+R` `J` `g J` `~` `i` `a` `I` `A` `o` `O` `v` `V` `Ctrl+V` `Esc` `/` `?` `n` `N` `*` `#` `g *` `g #` `m` `'` `` ` `` `g c` `&` `g &` `Ctrl+D` `Ctrl+U` `Ctrl+F` `Ctrl+B` `z z` `z t` `z b` `z Enter` `q`

## `editor.vim.insert` [editor] [text]

Query editor, vim Insert mode: typing (`Esc` goes back to Normal). Inside `workspace`.

| Keys | Action | Description | R |
|---|---|---|---|
| `Ctrl+N` | `editor.complete` | Show completions |  |
| `F4` | `editor.complete` | Show completions |  |

Reserved — vim Insert: `Esc` `Enter` `Tab` `Backspace` `Delete` `Left` `Right` `Up` `Down` `Home` `End` `Ctrl+W` `Ctrl+U` `Ctrl+R` `Ctrl+P`

## `editor.vim.search` [editor] [text]

The search prompt of `/` and `?` on the editor's last line: `Enter` searches, `Esc` goes back to where the cursor was. Inside `workspace`.

Reserved — vim search: `Esc` `Enter` `Ctrl+W`

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `editor.ddl` [editor]

A DDL tab in vim Normal mode: its text is read-only (moving, selecting, searching and yanking work; edits are refused); `r` reads it again, `o` opens it in a new console. Inside `editor.vim.normal`.

| Keys | Action | Description | R |
|---|---|---|---|
| `r` | `ddl.reload` | DDL tab: read the DDL again |  |
| `o` | `ddl.open_console` | DDL tab: open the DDL in a new console |  |

## `overlay.commands` [text]

The `:` command line: commands with arguments, and a search over every action. Inside `root`.

Reserved — command line: `Esc` `Enter` `Up` `Down` `Ctrl+P` `Ctrl+N` `Tab` `Shift+Tab`

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `overlay.quick_connect` [text]

Quick connect: a fuzzy list of the connection profiles. Inside `root`.

Reserved — quick connect: `Esc` `Enter` `Up` `Down` `Ctrl+P` `Ctrl+N`

Reserved — quick connect: databases and schemas: `Right` `Left`

Reserved — text input: `Backspace` `Delete` `Home` `End` `Ctrl+U` `Ctrl+A`

## `overlay.profile_form` [text]

Connection profile form. Inside `root`.

| Keys | Action | Description | R |
|---|---|---|---|
| `Ctrl+O` | `form.pick_key_file` | Pick the SSH key file |  |
| `Ctrl+B` | `form.save_as_tunnel` | Profile form: save this profile's SSH tunnel as a tunnel preset |  |

Reserved — profile form: `Tab` `Shift+Tab` `Down` `Up` `Enter` `Esc` `Ctrl+S` `Ctrl+T` `Ctrl+N` `Ctrl+P`

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `overlay.settings`

The settings screen: one list by category, each setting with its description. Inside `root`.

Reserved — settings screen: `j` `k` `Down` `Up` `h` `l` `Left` `Right` `Enter` `Space` `Esc` `q`

## `overlay.chooser`

A list to pick from: a profile's color, icon or folder. Inside `root`.

Reserved — chooser: `j` `k` `Down` `Up` `Enter` `/` `Esc` `q`

## `overlay.chooser.filter` [text]

The `/` filter of a list to pick from. Inside `root`.

Reserved — chooser filter: `Esc` `Enter` `Down` `Up`

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `overlay.name_input` [text]

A name to type: a new or renamed folder. Inside `root`.

Reserved — name input: `Enter` `Esc`

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `overlay.script_tree`

The folder tree of the saved queries (save as, open): the tree has the keyboard. Inside `root`.

Reserved — saved-queries tree: `j` `k` `h` `l` `Up` `Down` `Left` `Right` `Enter` `n` `Tab` `Shift+Tab` `Esc`

## `overlay.script_tree.name` [text]

The folder tree of the saved queries: its name field (save as) or filter (open). Inside `root`.

Reserved — saved-queries tree: name or filter: `Up` `Down` `Enter` `Tab` `Shift+Tab` `Esc`

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `overlay.context_menu` [text]

The action menu: a right click (an explorer node, the grid, a tab, the editor) opens it at the pointer; `Space Space`, `Shift+F10` or the Menu key next to the selection, `Space t m` under the active tab. The selection's actions come first, then the pane's, each with its key in that pane. Typing filters it (when nothing in it matches, every action is searched); the arrows, `Ctrl+N`/`Ctrl+P` and `Tab` move, `Enter` runs, `Esc` closes (`Backspace` on an empty filter does not); a shown key that is not a character (`Ctrl+…`, `F…`) runs its item; the pointer selects the item under it. Inside `root`.

Reserved — action menu: `Down` `Up` `Ctrl+N` `Ctrl+P` `Tab` `Shift+Tab` `Enter` `Esc`

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `overlay.password` [text]

Password prompt. Inside `root`.

Reserved — password prompt: `Esc` `Enter` `Tab` `Shift+Tab`

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `overlay.confirm`

Yes/no confirmation: `y` does it; Enter keeps what the answer would lose (quit, delete, disconnect, close, switch, replace); for a copy it says yes. Inside `root`.

Reserved — confirmation (Enter keeps what would be lost): `y` `n` `Enter` `Esc`

## `overlay.run_confirm`

Before statements that may do harm run: each is listed; Cancel has the focus, `y` or Enter on Run runs them. Inside `root`.

Reserved — run confirmation (Cancel has the focus): `y` `n` `Enter` `Esc` `Tab` `Shift+Tab` `Left` `Right` `h` `l`

## `overlay.icons_ask`

Whether the terminal shows the Nerd Font icons (asked once, with a preview); No has the focus. Inside `root`.

Reserved — the icons question (No has the focus): `y` `n` `Enter` `Esc` `Tab` `Shift+Tab` `Left` `Right` `h` `l`

## `overlay.busy`

A notice that waits for background work (the launch-time move of passwords to the keychain); `Ctrl+C` quits there too. Inside `root`.

| Keys | Action | Description | R |
|---|---|---|---|
| `q` | `app.quit` | Quit |  |

## `overlay.cell_viewer`

Cell value viewer. Inside `workspace`.

Reserved — cell viewer: `j` `k` `Down` `Up` `PageDown` `PageUp` `Space` `g g` `G` `Esc` `q`

## `overlay.completion` [text]

Completion popup in the editor (keys it does not use go to the editor). Inside `root`.

Reserved — completion popup: `Up` `Down` `Ctrl+P` `Ctrl+N` `Tab` `Enter` `Esc`

## `overlay.which_key`

Which-key popup of an unfinished leader sequence (other keys continue the sequence). Inside `root`.

Reserved — which-key popup: `Esc` `Backspace`

## `overlay.help`

Keyboard help: one list of every context; the sections of the context it was opened from come first, open; the pointer selects the row under it. Inside `root`.

Reserved — keyboard help: `j` `k` `Down` `Up` `PageDown` `PageUp` `Enter` `l` `Right` `h` `Left` `/` `Esc` `q`

## `overlay.help.filter` [text]

The `/` filter of the keyboard help. Inside `root`.

Reserved — keyboard help filter: `Esc` `Enter` `Down` `Up`

Reserved — text input: `Backspace` `Delete` `Left` `Right` `Home` `End` `Ctrl+U` `Ctrl+A`

## `:` commands

Type them after `:` (or `Ctrl+K`). `Tab`/`Shift+Tab`, `↑`/`↓` or `Ctrl+N`/`Ctrl+P` pick an entry, `Enter` runs it (a command that still needs its argument is completed instead), `Esc` or `Backspace` on an empty line closes. Text that is not a command searches the actions by name. A command that cannot run shows an error and the line stays open.

Editor commands (from the editor, `:` starts the line with `'<,'>` in Visual mode and with `.,.+2` after a count of 3, as Vim does; the command runs in the active tab's editor):

- A line range alone goes to its last line (the first non-blank): `:12`, `:$`, `:+3`, `:-`, `:'a`. Addresses are a number, `.`, `$`, `'x` (a mark: `'a`-`'z`, `'<`, `'>`, `''`, `'.`), each with `+n` / `-n`; `a,b`, `a;b` (the second counted from the first) and `%` (every line).
- `:[range]s/pattern/replacement/[flags]` (`:substitute`) replaces in the range's lines (the cursor's line without one) as one undo step; the cursor goes to the last line changed. The pattern is a Rust regular expression, as in search (an empty one is the last search's), and becomes the last search. The replacement is Vim's: `&` or `\0` the match, `\1`-`\9` a group (an absent one is empty), `~` the previous replacement, `\r` a line break (`\n` too; in Vim it is a NUL), `\t` a tab, `\u` `\l` the next character upper or lower case, `\U` `\L` up to `\E` or `\e`, a backslash before any other character that character (`\&`, `\~`, `\\`, the delimiter); `$1` is plain text. Flags: `g` (every match in a line), `i` / `I` (ignore case, or not), `e` (no error when nothing matches), `&` first (the last flags again); `c` (confirm) is not supported and says so. Any delimiter that is not a letter, digit, blank, `\`, `"` or `|` works (`:s#a/b#c#`). `:s` alone and `:&` repeat the last `:s` without its flags, `:&&` with them. Nothing matched: "Pattern not found".
- `:[range]format` formats the range's lines (`:'<,'>format`, `:%format`); `:format` without a range, and `Space e f` (`editor.format`), format the statement under the cursor or the Visual selection. The formatter is never run on its own. It changes only the layout: where lines break, the indent (`[editor] format_indent`) and, if asked, the case of keywords (`[editor] format_keyword_case`). Names, strings, comments and dollar-quoted bodies stay as written. When its result would differ in anything else (an operator written apart, strings joined across a line break, `U&'…'`, a psql `\command`) it changes nothing and says on which line. One format is one undo step.

An editor command closes the line; what went wrong is said in the status bar.

| Command | Aliases | Description |
|---|---|---|
| `:conn <profile>` | `:connect` | Connect to a profile |
| `:set <setting>=<value>` |  | Change a setting |
| `:help` | `:h` | Keyboard help |
| `:run` |  | Run statement under cursor |
| `:cancel` |  | Cancel running query |
| `:quit` | `:q` | Close the tab (on the last tab: quit) |
| `:qall` | `:qa` `:quitall` | Quit |
| `:w [name]` | `:write` | Save (a console asks for a name; with a name: save as) |
| `:wq [name]` | `:x` | Save, then close the tab (on the last tab: quit) |
| `:e [name]` | `:edit` | Open a saved query |
| `:tabnew` |  | New console tab |
| `:tabclose` |  | Close tab |
| `:recover` |  | Bring back a closed console (also from earlier runs) |
| `:settings` |  | Open the settings |
| `:copy <format>` |  | Copy the selected range, or every fetched row, as… (:copy <format> [selection|all]) |
| `:use [db][.schema]` |  | Choose this tab's database and schema |
| `:nohlsearch` | `:noh` `:nohl` | Clear the search highlight in the editor (until the next search) |
| `:suspend` | `:sus` `:stop` | Suspend datarig (the shell's fg brings it back) |
| `:format` |  | Format the statement under the cursor or the selection (only its layout) |
| `:ddl [schema.name]` |  | Show the DDL of the explorer's object, the table tab's table, or a name (:ddl [schema.name]) |
| `:explain [analyze]` |  | The plan of the statement under the cursor (:explain analyze runs and measures it; a write is rolled back) |

Settings of `:set` (saved to `config.toml` like the matching actions):

| Setting | Values | Description |
|---|---|---|
| `language` | `en` `ko` `auto` | UI language |
| `icons` | `on` `off` `auto` | Nerd Font icons |
| `secrets.default_source` | `auto` `keychain` `file` `command` `env` `prompt` | Password storage of new profiles |
| `commands.position` | `popup` `bottom` | Command line position |
| `detail_view` | `panel` `statusbar` | Cell detail |
| `clipboard` | `auto` `system` `osc52` | Clipboard |
| `copy_header` | `auto` `on` `off` | Column names in copies |
| `editor.cursor_shape` | `on` `off` | Cursor shape |
| `editor.clipboard` | `on` `off` | Yanks to the clipboard |
| `theme` | `terminal` `dark` `light` `high-contrast` `catppuccin` `catppuccin-latte` `catppuccin-mocha` `tokyo-night` `tokyo-night-day` `tokyo-night-night` `gruvbox` `gruvbox-light` `gruvbox-dark` `nord` `dracula` or the name of a theme file | Theme |
| `editor.format_keyword_case` | `preserve` `upper` `lower` | Formatter: keywords |
| `editor.format_indent` | `4` `2` | Formatter: indent |
| `editor.auto_pairs` | `off` `on` | Auto-pairs |

## Command line only

Actions without a default key (the command line finds them by name).

| Action | Description |
|---|---|
| `tunnel.new` | New SSH tunnel (shared by profiles) |
| `ui.language.en` | Change language → English |
| `ui.language.ko` | Change language → Korean |
| `ui.language.auto` | Change language → Auto (system locale) |
| `ui.icons.toggle` | Nerd Font icons → on/off |
| `ui.icons.on` | Nerd Font icons → on |
| `ui.icons.off` | Nerd Font icons → off |
| `ui.icons.auto` | Nerd Font icons → ask again (with a preview) |
| `secrets.default.auto` | New profiles' password → auto (keychain when available) |
| `secrets.default.keychain` | New profiles' password → OS keychain |
| `secrets.default.file` | New profiles' password → secrets file |
| `secrets.default.command` | New profiles' password → command |
| `secrets.default.env` | New profiles' password → environment variable |
| `secrets.default.prompt` | New profiles' password → prompt on every connect |
| `help.all` | Keyboard help: expand all |
| `explorer.context_menu` | Explorer: actions of this node (menu) |
| `ddl.copy` | DDL tab: copy the whole DDL |
