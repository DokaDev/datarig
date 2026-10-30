# Editor

Each console and table tab has a SQL editor with vim keys, syntax highlighting, completion and
a formatter you run yourself. The statement under the cursor is marked in the gutter.

```text
{{#include ../screens/editor.txt}}
```

## Vim keys

The editor has vim keys only. `i` starts typing and `Esc` stops; the status bar says so. What
is implemented, all with counts:

- **Normal mode**: the usual motions (`w` `b` `e`, `0` `^` `$`, `gg` `G`, `f` `t` `;`, `%`,
  `{` `}`, `H` `M` `L`, …), the operators `d` `c` `y`, `gu` `gU` `g~` and `>` `<` with any
  motion or text object (`iw` `aw`, `i(` `a(`, `i"`, `ip`, …), doubled for lines (`dd`, `yy`),
  `x` `s` `D` `C` `p` `P` `r` `J` `~`, `u` and `Ctrl+R`, and `.` repeating the last change.
  One command is one undo step.
- **Visual mode** by character (`v`), by line (`V`) and by block (`Ctrl+V`). The block works
  in screen columns, so wide characters line up, and `I`/`A` type on every line of the block.
- **Insert mode** keeps the usual editing keys, plus `Ctrl+W`, `Ctrl+U` and `Ctrl+R` with a
  register. `Enter` keeps the line's indent. `Tab` types 4 spaces, and `Backspace` after spaces
  deletes back to the tab stop in one press, as Vim with `softtabstop=4` does
  (`[editor] soft_tab_backspace = "off"` always deletes one character).
- **Registers** as in Vim (`"a`-`"z`, `"0`, the `"1`-`"9` delete ring, `"-`, `"_`, `".`, `"/`,
  `"+`/`"*`). As with `clipboard=unnamedplus`, every yank, delete or change without a register
  also goes to the system clipboard, or through OSC 52 over SSH; `[editor] clipboard = "off"`
  keeps them in the editor. `"+p` reads the system clipboard only when you type it.
- **Search** with `/` and `?` (a prompt on the editor's last line; the cursor shows the match as
  you type), `n` `N`, `*` `#`, `g*` `g#`, also after an operator (`d/from`). Matches on screen
  stay highlighted until `:noh`. Patterns are
  [Rust regular expressions](https://docs.rs/regex/latest/regex/#syntax), not Vim's: case
  sensitive unless the pattern starts with `(?i)`, `\b` for a word boundary, and a match never
  spans two lines.
- **Marks**: `m{a-z}`, `'a` and `` `a ``, `''`, `'<` `'>` and `'.`. Marks follow their lines
  through edits and come back on undo.
- **Comments**: `gc` comments lines out with `-- ` or back in, as Neovim does (`gcc`,
  `gc{motion}`, `gc` in Visual mode; also `Space e c`).
- **Auto-pairs** are off by default, as in Vim. `[editor] auto_pairs = "on"` (or
  `:set editor.auto_pairs=on`) makes brackets and quotes put their closing character after the
  cursor (not inside strings or comments, not before a word, never for a paste).
- A paste from the terminal goes in at the cursor in every mode.
- Commands keep working with a Korean (2-set) input source: the jamo are read as the QWERTY
  keys they sit on.

The complete list, with every key's exact behaviour, is in the
[keybindings reference](../reference/keybindings.md). Not yet: macros (their keys are
reserved), `gv`, and Ex commands other than line numbers, `:s` and `:format` (such as :d and :g).

## Ex commands

On the `:` line the editor takes:

- A line range alone goes to its last line: `:12`, `:$`, `:'a`, `:+3`.
- `:[range]s/pattern/replacement/[flags]` replaces in the range (the cursor's line without
  one), as one undo step, for example `:%s/old/new/g` or `:'<,'>s/^/-- /`. The pattern is a Rust
  regular expression, the replacement Vim's (`&`, `\1`, `\r`, `\u`, `~`; `$1` is plain text).
  Flags: `g`, `i`, `I`, `e`; `c` (confirm) is not supported. `:&&` and `&` / `g&` in Normal
  mode repeat the last substitution.
- `:[range]format` formats the range's lines (see below).

In Visual mode `:` starts the line with `'<,'>`, as Vim does. The full syntax is on the
[commands reference](../reference/commands.md).

## Completion

`Ctrl+N` or `F4` in Insert mode shows completions of schemas, tables and columns (aliases
included).

- Names go in as SQL: a name that is not a plain lower-case word (`MixedCase`, `Order Lines`) or
  is a reserved word goes in quoted.
- The statement's `WITH` queries are offered as tables, with their columns when the query names
  them.
- Names that start with what you typed come first, then names that contain its letters in
  order (`oi` finds `order_items`); keywords only by their start.

**Values in a string literal** compared with a column (`status = '`, `<>`, `!=`,
`status IN ('paid', '`, `NOT IN`, `UPDATE … SET status = '`) complete to the column's allowed
values. They come from its type (PostgreSQL enums, also through domains; MySQL `ENUM`), then
from its table's CHECK constraints of the forms `col IN ('a', 'b')`, `col = 'a'` and an `OR` of
those. The checks are read with the table's structure (PostgreSQL 12+, MySQL 8.0.16+), so their
values show a moment after the first use. A value goes in escaped as the literal reads it, and
an unclosed literal gets its closing quote. A value inside a function call
(`lower(status) = '`) gets none; `INSERT … VALUES` is not covered.

## Formatting

`Space e f`, `:format` or `:'<,'>format` lays out the statement under the cursor or the
selection, as one undo step. It never runs on its own.

- It changes only the layout: line breaks, the indent (`[editor] format_indent`, 4 or 2), and
  keyword case if you ask for it (`[editor] format_keyword_case = "upper"` or `"lower"`). Names,
  strings, comments and dollar-quoted bodies stay exactly as written.
- Every result is checked token by token against the original. When anything else would change
  (an operator split, strings joined across a line break, `U&'...'`, psql meta-commands) it
  refuses, says on which line, and leaves the text alone.

## Running statements

- `Ctrl+E` (or `Ctrl+Enter` where the terminal reports it) runs the statement under the cursor,
  or the selection. Several statements run in a row: each outcome is listed in a Messages tab,
  and every row result gets a result tab of its own (see [Results](results.md)).
- `Ctrl+C` cancels. When a PostgreSQL cancel cannot reach the server (a tunnel that refuses it,
  a network failure), datarig closes the tab's connection and says why. What became of the
  statement is then unknown: it may run to its end and commit if it was not in a transaction
  block; an open block is rolled back when the server notices the closed connection.
- While a run goes on, the statement it executes has a tint of its own and a spinner in the
  gutter, wherever you move or edit.
- When it ends, a dim hint after each statement's last line says what it did (rows and time,
  rows affected, the error, rolled back, cancelled). The hint is not part of the text and goes
  when you edit the statement. `[editor] run_hints = "off"` turns the hints off.

## Saved queries and the workspace

- Saved queries are plain `.sql` files in folders in the data directory: `Space s s` or `:w`
  saves (a console asks for a name), `Space s a` saves as, `Space s o` or `:e` opens one.
- Every tab is autosaved, and the workspace (tabs, cursors, open folders) is restored at the
  next start. Closed consoles go to a trash: `:recover` brings one back.

## Your own editor, and suspending

- `Ctrl+G` opens the query in your own editor (`$VISUAL`, else `$EDITOR`, else `vi`; for example
  `EDITOR="code -w"`), run directly without a shell on a private temporary file. What you save
  replaces the text as one undo step; quitting without saving (or Vim's :cq) changes nothing.
  A table tab's query opens as a copy that comes back in a new console.
- `Ctrl+Z` (or `:suspend`) suspends datarig as Vim does, and the shell's `fg` brings it back
  (not on Windows). A running query keeps running on the server meanwhile.
