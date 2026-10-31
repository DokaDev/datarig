# Getting started

## Start datarig

```sh
datarig                          # open the workspace
datarig my-profile               # connect to a profile and open a console
datarig --config path/to/config.toml
```

Without a profile on the command line nothing connects until you pick one. The full list of
options is on the [command line](reference/command-line.md) page.

## The welcome panel

With no connection profile yet, the workspace shows a welcome panel with four first steps: add
a connection, run a query, open the action menu, and open the keyboard help. Each is marked
once you have done it, however you did it. `X` hides the steps for good.

```text
{{#include screens/welcome-first-steps.txt}}
```

## Add a connection

Press `n` (or `Enter`) on the welcome panel, or `Space c n` anywhere, to open the profile
form. Fill in the host, port, user and database, or paste a connection URL such as
`postgres://me@127.0.0.1:5432/app` into it. **Test** connects once and shows the server
version, the latency and how the connection is encrypted. Saving writes the profile to the
config file; the password goes to the password store you chose, never into the file. See
[Connections](guide/connections.md).

## Connect and run a query

1. `Ctrl+O` opens quick connect: type part of a profile's name and press `Enter`. A console
   tab on that connection opens.
2. Press `i` to type (the editor has vim keys; `Esc` goes back to Normal mode).
3. `Ctrl+E` runs the statement under the cursor. `Ctrl+C` cancels a running statement.
4. The result opens below the editor. `n` and `p` in the grid move to the next and previous
   page; `Tab` and `F6` move the focus between the panes.

A statement that may do harm (a `DROP`, an `UPDATE` without a real `WHERE`, …) asks first,
with Cancel focused. See [Environments and safety](guide/environments.md).

## Find your way

- Press `Space` and wait: a popup lists the keys that may follow.
- `F1` or `Space ?` opens the keyboard help for the current pane, with search.
- `Space Space` opens the action menu: what you can do with the selected thing, each with its
  key. A right click opens it too.
- `:` (outside text input) or `Ctrl+K` (everywhere) opens the command line. It runs
  [commands](reference/commands.md) and finds every action by name.
- The hint line at the bottom, the menus, the tips and the messages all name the same key for
  an action: a key of your own when you bound one, else one that needs no function key
  (`Space ?` rather than `F1`), except where that key types text (in insert mode the help is
  `F1`).

## The workspace

```text
{{#include screens/main-layout.txt}}
```

**Tabs.** `Ctrl+T` (or `:tabnew`) opens a new console, `Ctrl+W` closes the tab, `Space 1` ..
`Space 9` go to a tab, `Ctrl+PageDown` / `Ctrl+PageUp` (or `g t` / `g T`) to the next and
previous one.

- `Space t t` (or `:tabs`, `:ls`, `:buffers`) lists the open tabs, the most recently used first
  with the previous one selected, then the tabs closed in this session. Type to filter by name,
  kind, connection, database or schema; `Enter` goes to a tab or brings a closed one back,
  `Ctrl+D` closes the selected tab. `Space t u` reopens the last closed tab.
- Reorder tabs by dragging one with the mouse, with `Space t g` and then `h` / `l` (`Enter`
  keeps, `Esc` puts it back), with `Space t <` / `Space t >`, or with `:tabmove` as in Vim
  (`:tabmove 0` first, `:tabmove` last, `:tabmove +1`). The order is kept across restarts.

**Panes.** `Space z` zooms the focused pane (explorer, editor, results or inspector) to the
whole workspace and back, as tmux does (`ZOOM` in the status bar). `Space b` hides or shows the
explorer, `Space <` / `Space >` or a drag of its border resize it, and a drag of the results'
top border resizes them.

**The action menu.** A right click on an explorer node, the result grid, a tab or the editor
opens it at the pointer; `Space Space`, `Shift+F10` or the `Menu` key open it next to the
selection, and `Space t m` under the active tab. It lists what you can do with the selected
thing, then with the pane, each with its key. Type to filter it; when nothing in it matches,
every action is searched.

**The mouse.** Besides the menu, the mouse works in the dialogs (the profile and tunnel forms,
the settings, confirmations, the password prompt, pick lists, quick connect, the command line).
A button or a row acts when it is pressed and released on it. The pointer only highlights; it
never moves the focus or the selection, so `Enter` in a confirmation still keeps. Text inputs
have no mouse selection. Under the pointer, the tab bar's `×` turns red, tabs, result tabs,
paging arrows and plan view names light up, and a grid cell and the inspector entry of its
column light each other faintly.

**Settings.** `Space ,` (or `:settings`) opens the settings screen; every setting there is also
`:set <setting>=<value>` (see the [commands reference](reference/commands.md)) and a key in the
[config file](reference/configuration.md). The UI is in English or Korean:
`:set language=ko` (or `auto` for the system locale).

**Remapping keys.** `[keymap.<context>]` tables in the config file bind keys to action ids, or
unbind them with `"none"`; see the [keybindings reference](reference/keybindings.md).

### Tips

With `[ui] tips` on (the default; `:set ui.tips=off` turns it off), a dim line about a key shows
where the screen is empty anyway: the workspace without tabs, an empty console, a result with
nothing to show or still running, and the status bar's spare room. The key named is the one
bound now, remapped keys included. A tip about a key you have pressed once is not shown again.
An action you ran three times from the action menu or the command line without its key gets one
status line naming its key, once. Nothing is sent anywhere; see
[where datarig keeps its files](faq.md#where-does-datarig-keep-its-files).

## Quit

`:q` closes the tab (on the last tab, it quits). `:qa` or `Ctrl+Q` quits. When a tab holds an
uncommitted transaction, datarig asks first.

The workspace (tabs, their text, cursors, open folders) is saved and restored at the next
start. A second datarig started while the first runs restores no tabs and writes none of the
workspace files.

## Try it against a test database

The repository's `dev/` directory has Docker Compose services for development. Start its
PostgreSQL (see the
[development notes](https://github.com/DokaDev/datarig/blob/main/dev/README.md)) and paste
`postgres://datarig@127.0.0.1:55432/datarig` into the new-connection form; the password is
`datarig`.
