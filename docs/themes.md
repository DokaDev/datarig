# datarig themes

<!-- Generated from crates/tui/src/theme.rs and theme/authoring.rs. Do not edit;
     run `DATARIG_BLESS=1 cargo test -p datarig-tui --test themes_doc` at the repository root. -->

The setting `theme` (`:set theme=<name>`, or the settings screen, `Space ,`) names a built-in theme or a theme file. A theme file is `themes/<name>.toml` next to the config file (`$XDG_CONFIG_HOME/datarig/themes/`, else `~/.config/datarig/themes/`); its name is ASCII letters, digits, `-` and `_` (at most 64), and never a built-in theme's.

Built-in themes: `terminal`, `dark`, `light`, `high-contrast`, `catppuccin`, `catppuccin-latte`, `catppuccin-mocha`, `tokyo-night`, `tokyo-night-day`, `tokyo-night-night`, `gruvbox`, `gruvbox-light`, `gruvbox-dark`, `nord`, `dracula`.

## The file

`extends` names the built-in theme the file starts from (`terminal` without it). `[colors]` sets color tokens and `[styles]` style tokens; a token the file does not set keeps the value of the theme it extends, and a style it sets replaces that one whole. The dimming of the screen behind a dialog is not a token: it is the extended theme's. An unknown token, color or modifier is an error with its line, and the app then draws with `terminal` and says why.

- A color is `#rrggbb`, an ANSI name (`black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `bright-black`, `bright-red`, `bright-green`, `bright-yellow`, `bright-blue`, `bright-magenta`, `bright-cyan`, `bright-white`) or `default` (the terminal's own).
- A style is `{ fg = <color>, bg = <color>, modifiers = [...] }`, each part optional; what it leaves out is not set.
- Modifiers: `bold`, `dim`, `italic`, `underlined`, `reversed`, `crossed-out`.

```toml
# themes/mine.toml, used with theme = "mine"
extends = "dark"

[colors]
accent = "#89b4fa"
warning = "yellow"

[styles]
selection = { bg = "#45475a", modifiers = ["bold"] }
syn_comment = { fg = "bright-black", modifiers = ["italic"] }
```

## Writing a theme file

- `:theme copy <from> <name>` writes `themes/<name>.toml` with every token of the theme `<from>` (a built-in theme or a theme file; a family such as `catppuccin` as the variant drawn now) set to its value, each under a comment saying what it colors. `extends` names the built-in theme `<from>` starts from, for the dimming behind dialogs; every token is in the file, so it stands on its own.
- `:theme new <name>` writes `themes/<name>.toml` extending the current theme when it is built in, else `dark`, with every token commented out and showing the value it takes from there: remove the `# ` in front of a key to set it.
- On the settings screen, `c` on the theme row copies the theme shown there (the one being previewed, else the current one) and asks for the new name.

Each writes a new file and never replaces one: a name that is taken, that is not a valid file name, that is a built-in theme's or a Windows device name such as `nul` (ignoring case) is refused. The themes folder is created when it is missing. Then datarig draws with the new theme, saves `theme = "<name>"` to the config file and shows the file's path.

## Colors (`[colors]`)

| Token | What it colors |
|---|---|
| `bg` | The background of the panes: explorer, editor, results. |
| `surface` | The background of dialogs, popups, menus, the command line and the status bar. |
| `surface_alt` | A second surface: the active tab, every other row of the result grid, form fields and buttons. |
| `border` | Borders and rules between panes and around dialogs. |
| `accent` | The main accent: the focused pane's border, dialog borders, labels and names to notice. |
| `accent_warm` | The warm accent: values on the settings screen, an open transaction, keys in menus. |
| `fg` | Body text. |
| `fg_muted` | Secondary text: descriptions, labels and hints. |
| `fg_dim` | Faint text: placeholders, empty lists and notes such as a database's default. |
| `current_stmt_bar` | The bar in the editor's gutter next to the statement a run would take. |
| `running_stmt_bar` | The bar and the spinner in the editor's gutter next to the statement that runs now. |
| `success` | Success: rows a run returned, a connected tunnel, a success message. |
| `warning` | Warnings. |
| `error` | Errors. |
| `null_fg` | NULL in the result grid and the inspector. |
| `key_pk` | The mark of a primary key column. |
| `key_fk` | The mark of a foreign key column. |
| `key_uq` | The mark of a unique key column. |
| `mode_normal` | The background of the status bar's mode badge in Normal mode. |
| `mode_insert` | The background of the mode badge in Insert mode. |
| `mode_visual` | The background of the mode badge in Visual mode. |
| `mode_command` | The background of the mode badge while the command line is open. |
| `mode_fg` | The text of the mode badge (bold, on the mode's color). |
| `chart_1` | The first series of a chart. |
| `chart_2` | The second series of a chart. |
| `chart_3` | The third series of a chart. |
| `chart_4` | The fourth series of a chart. |
| `chart_5` | The fifth series of a chart. |
| `chart_6` | The sixth series of a chart. |

## Styles (`[styles]`)

| Token | What it styles |
|---|---|
| `selection` | The selected row or item, and the grid's cursor cell. |
| `range` | Cells and headers of a selected range in the result grid (the cursor's cell keeps selection). |
| `cursor_line` | The row under the cursor of an unfocused list or plan, the editor's line, and (its background) the chart's cursor while focused. |
| `hover` | What is under the mouse pointer: a list row, a grid cell, a tab, a button. |
| `yank_flash` | Cells copied from the result grid, for a moment after the copy was sent to the clipboard (set fg and bg: it covers the cursor's cell too). |
| `current_stmt` | A faint tint behind the statement a run would take. |
| `running_stmt` | A tint behind the statement that runs now. |
| `run_hint` | The hint after a statement's last line that says what its last run did. |
| `syn_keyword` | SQL keywords. |
| `syn_function` | SQL function names. |
| `syn_string` | SQL strings and dollar-quoted bodies. |
| `syn_number` | SQL numbers. |
| `syn_comment` | SQL comments. |
| `syn_operator` | SQL operators and punctuation. |
| `syn_identifier` | SQL names, parameters and variables. |
| `syn_quoted_ident` | SQL quoted names. |
| `syn_exec_comment` | The opening and closing of a MySQL executable comment (/*! ... */). |
| `search_match` | A match of the editor's search on the lines on screen. |
| `match_paren` | The bracket matching the one at the editor's cursor (not drawn yet). |
| `read_only_mark` | The mark of read-only rules (its fg): the lock on a profile's row and tabs, and the rule's words. |
| `danger_mark` | The mark of something that changes or deletes data, or of a production profile (not drawn yet). |
| `plan_hot` | The plan nodes that take the most time, in every view of a query plan and its summary. |
| `plan_misestimate` | How far a plan node's estimated rows are from the actual ones, in the plan, its summary, detail and funnel. |
| `size_warm` | Explorer size estimates of 1 MiB or more (with five levels: 100 MiB or more). |
| `size_hot` | Explorer size estimates of 1 GiB or more (with five levels: 10 GiB or more). |
| `size_mild` | With five size levels, explorer size estimates of 1 MiB or more. |
| `size_heated` | With five size levels, explorer size estimates of 1 GiB or more. |
