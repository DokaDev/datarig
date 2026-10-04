//! `docs/keybindings.md`, generated from the action registry, the default bindings and the
//! English catalog. `tests/keybindings_doc.rs` fails when the committed file is out of date.

use super::{Bound, Ctx, Keymap, PROTECTED, Target, keys, parse_keys};
use crate::app::action;
use crate::app::command::{self, ArgKind};
use datarig_core::i18n::{Label, Lang};

/// The whole document (LF line endings).
pub fn render() -> String {
    let km = Keymap::default();
    let mut s = String::new();
    s.push_str("# datarig key bindings\n\n");
    s.push_str(
        "<!-- Generated from crates/datarig-tui/src/keymap/defaults.rs and the action registry. Do not edit;\n     \
         run `DATARIG_BLESS=1 cargo test -p datarig-tui --test keybindings_doc` at the repository root. -->\n\n",
    );
    s.push_str(
        "Keys resolve from the current context outwards to `root`; the first binding found wins. \
         Dialogs (`overlay.*`) sit directly under `root`, so only `root` keys pass through them. The cell \
         viewer is not modal: it sits under `workspace`, so the workspace keys (run, …) keep working.\n\n\
         - **[text]** text input: letters, digits, symbols and `Space` are typed. No leader, no Hangul mapping.\n\
         - **[editor]** the query editor: its keys may hide keys of the contexts around it, except protected keys.\n\
         - **Hangul**: outside text input a Korean 2-set jamo or syllable counts as the QWERTY key(s) at its \
         place (U+3153 = `j`, U+D558 = `g k`); the status bar then shows the Korean input-source mark. The \
         character vim's `f t F T r` wait for is taken as typed.\n\
         - **R**: repeats while the key is held (terminal auto-repeat).\n\
         - **Key guide**: after `Space` a which-key popup lists the keys that may follow (after 300 ms; \
         `Backspace` goes up a level, `Esc` closes). `Space ?` or `F1` open the keyboard help, the same \
         way everywhere (`?` alone is left to vim's backward search). The help is one list: the sections of \
         the current context first and open, every other context closed below (`Enter`/`l`/`h` or a click \
         opens and closes a section); `/` searches all of them. The status bar shows the most relevant keys \
         of the current context.\n\
         - **Commands**: `:` (outside text input) or `Ctrl+K` (everywhere) opens the command line: a popup \
         near the top of the screen, or the last line with `[commands] position = \"bottom\"` (the settings \
         screen, `:set commands.position=bottom`). It runs the commands listed at the end (`:conn <profile>`, `:set <setting>=<value>`, \
         …) and finds every action by name; see the tables at the end.\n\
         - **Vim**: the editor has vim keys. `i` starts typing and `Esc` stops (the hint line says so). \
         Implemented, all with counts: the motions `h j k l w b e W B E ge gE 0 ^ $ gg G`, `f F t T ; ,` \
         (the character is taken as typed, Hangul included), `%` (the matching bracket of `( ) [ ] { }`, \
         brackets in strings and comments left out; `50%` goes to the middle line), `{ }`, `H M L`, the \
         arrows, `Home` and `End`; the operators `d c y`, `gu gU g~` (case) and `> <` (indent by 4 \
         columns, written with spaces as `Tab` types them) with any motion or text object, doubled for \
         whole lines (`dd cc yy guu gUU g~~ >> <<`), counts before and after (`3dd`, `d2w`, `2d3w`); the \
         text objects `iw aw iW aW`, `i( a(` (`ib ab`), `i[ a[`, `i{ a{` (`iB aB`), `i< a<`, `i\" a\" \
         i' a'`, ``i` a` `` and `ip ap`; `x X s S D C Y p P r J gJ ~ u Ctrl+R`; `.` (the last change \
         again, what was typed in Insert mode included; `3.` with a new count, except for a Visual mode \
         operator, which keeps its own as in Vim); `i a I A o O` (a count \
         types the text that many times); scrolling with `Ctrl+D Ctrl+U Ctrl+F Ctrl+B zz zt zb`; search \
         with `/` and `?` (a prompt on the editor's last line: the cursor shows the match while the \
         pattern is typed, `Enter` searches, `Esc` or any key of the app goes back, `Ctrl+W` deletes a \
         word; an empty pattern takes the last one), `n` `N`, `*` `#` (the word under the cursor, as a \
         whole word) and `g*` `g#` (also inside longer words), with counts and after an operator \
         (`d/from` `Enter`, `yn`: up to the match, without it; a match at the end of a line stops on \
         its last character, as in Vim); \
         the matches on screen stay highlighted until `:nohlsearch` (`:noh`) and the next search shows \
         them again. Search patterns are Rust regular expressions \
         (<https://docs.rs/regex/latest/regex/#syntax>), not Vim's: case-sensitive (`(?i)` at the start \
         ignores case), `\\b` for a word boundary (Vim's `\\<` `\\>`), `.` `*` `+` `?` `(` `)` `|` without a \
         backslash; a match lies within one line, and search offsets (`/foo/e`) are not supported; marks: \
         `m{a-z}` sets one at the cursor, `'{a-z}` goes to its line (the first non-blank) and `` `{a-z} `` \
         to its place, also after an operator (`d'a` takes whole lines, `` y`a `` the characters up to \
         it); `''` and ``` `` ``` go back to where the last jump left from (`G`, `gg`, `%`, `{`, `}`, `H`, \
         `M`, `L`, a search, a mark; `m'` sets it), `'<` `'>` (`` `< `` `` `> ``) to the ends of the last \
         Visual selection and `'.` to the last change; marks move with their lines as lines are added \
         or deleted above them and go with a deleted line, an undo puts them back, and `'A`-`'Z`, `'0`-`'9`, \
         `'[` `']` `'^` are not kept (they say so); `gc` comments lines out with `-- ` or back in, as \
         Neovim's built-in commenting does: `gcc` (with a count), `gc` with a motion or text object and \
         `gc` in Visual mode take whole lines; when every line that is not blank starts with `--` they \
         lose it, otherwise each gets `-- ` after the smallest indent (a blank line `--`), and `.` \
         repeats it; editor commands on the `:` line (see the end); `&` runs the last `:s` again on \
         the cursor's line with its flags (as Neovim's `&`, Vim's `:&&`) and `g&` on every line with \
         the last search pattern; Visual \
         mode by character (`v`) and by line (`V`) with the motions and text objects, `y d x c`, `Y D X \
         C S` (whole lines), `r J gJ u U ~ > <`, `o` (the other end) and `p P` (a register in place of the \
         selection; `P` keeps the replaced text out of the registers); Visual mode by block (`Ctrl+V`; \
         `v`, `V` and `Ctrl+V` switch between the three, the same key again leaves): screen columns, a \
         wide character or a tab partly inside handled as Vim does, `$` to the end of every line, `o` \
         and `O` (the other corners), `y Y d x X D` (`D` to the ends of the lines), `c s C`, `I` and `A` \
         (what Insert mode types goes on every line of the block when it ends; `A` fills short lines \
         with blanks first, `$A` appends at each line's end; a count types it that many times), `r` \
         (`r Enter` breaks the lines), `~ u U gu gU g~`, `> <` with a count, `J gJ`, `S R` (whole \
         lines) and `p P` (a block register as a block, lines below or above, text of one line on \
         every line of the block); `.` repeats a block operator on a block of the same size from the \
         cursor (after `p` only the delete); a paste from the terminal replaces the block; in Insert \
         mode `Ctrl+W` (the word before the cursor), `Ctrl+U` (the line before the cursor), `Ctrl+R \
         {register}` (its text) and `Enter` keeping the indent. One command is one undo step. Registers as in Vim: `\"x` before a \
         command in Normal and Visual mode (`\"ayy`, `2\"ap`), the unnamed one, `\"a`-`\"z` (`\"A`-`\"Z` \
         append), `\"0` (the last yank), `\"1`-`\"9` (the last deletes of lines; `.` after `\"1p` puts \
         `\"2`), `\"-` (small deletes), `\"_` (nothing kept), `\".` (the text last typed in Insert mode), `\"/` \
         (the last search pattern) and `\"+` `\"*` (the system clipboard); `\":` `\"%` `\"#` are not kept (a put \
         from them says the register is empty). As in \
         Vim with `clipboard=unnamedplus`, a yank, delete or change without a register also goes to the \
         system clipboard, through the `clipboard` setting's way (`[editor] clipboard = \"off\"` keeps them \
         in the editor); `\"a` and `\"_` never do. `\"+p` reads the system clipboard only when typed, and \
         not over OSC 52 (the terminal's own paste works there). A paste from the \
         terminal goes in at the cursor in every mode (over the selection in Visual mode). Unlike Vim, a \
         tab is 4 columns wide and a completion taken from the popup is not part of what `.` repeats. \
         The other reserved vim keys do nothing yet.\n\
         - **Reserved** keys belong to a widget (vim, a dialog) or to an action that is not \
         implemented yet. They cannot be remapped.\n\n",
    );
    let protected: Vec<String> =
        PROTECTED.iter().filter_map(|k| parse_keys(k).ok()).map(|k| format!("`{}`", keys::label(&k))).collect();
    s.push_str(&format!("Protected keys (no inner context may hide them): {}.\n\n", protected.join(" ")));
    s.push_str(
        "Remap in `config.toml` with `[keymap.<context>]` tables: `\"<keys>\" = \"<action id>\"`, or `\"none\"` \
         to unbind. Key notation: `ctrl+e`, `shift+tab`, `f4`, `pagedown`, `space c n` (a space separates the \
         keys of a sequence), `G` = `shift+g`. Invalid entries are skipped and reported at startup.\n\n",
    );
    s.push_str("```toml\n[keymap.explorer]\n\"x\" = \"explorer.refresh\"\n\"q\" = \"none\"\n```\n");
    for ctx in Ctx::ALL {
        section(&mut s, &km, ctx);
    }
    commands(&mut s);
    s.push_str("\n## Command line only\n\nActions without a default key (the command line finds them by name).\n\n");
    s.push_str("| Action | Description |\n|---|---|\n");
    for spec in action::REGISTRY {
        if !km.bindings().iter().any(|b| b.target == Target::Action(spec.action)) {
            s.push_str(&format!("| `{}` | {} |\n", spec.id, en(spec.label)));
        }
    }
    s
}

/// `text` as Markdown code: a backtick in it takes two around it.
fn code(text: &str) -> String {
    if text.contains('`') { format!("`` {text} ``") } else { format!("`{text}`") }
}

/// The `:` commands and the settings of `:set`.
fn commands(s: &mut String) {
    s.push_str(
        "\n## `:` commands\n\nType them after `:` (or `Ctrl+K`). `Tab`/`Shift+Tab`, `↑`/`↓` or `Ctrl+N`/`Ctrl+P` \
         pick an entry, `Enter` runs it (a command that still needs its argument is completed instead), `Esc` or \
         `Backspace` on an empty line closes. Text that is not a command searches the actions by name. A command \
         that cannot run shows an error and the line stays open.\n\n",
    );
    s.push_str(
        "Editor commands (from the editor, `:` starts the line with `'<,'>` in Visual mode and with \
         `.,.+2` after a count of 3, as Vim does; the command runs in the active tab's editor):\n\n\
         - A line range alone goes to its last line (the first non-blank): `:12`, `:$`, `:+3`, `:-`, \
         `:'a`. Addresses are a number, `.`, `$`, `'x` (a mark: `'a`-`'z`, `'<`, `'>`, `''`, `'.`), each \
         with `+n` / `-n`; `a,b`, `a;b` (the second counted from the first) and `%` (every line).\n\
         - `:[range]s/pattern/replacement/[flags]` (`:substitute`) replaces in the range's lines (the \
         cursor's line without one) as one undo step; the cursor goes to the last line changed. The \
         pattern is a Rust regular expression, as in search (an empty one is the last search's), and \
         becomes the last search. The replacement is Vim's: `&` or `\\0` the match, `\\1`-`\\9` a group \
         (an absent one is empty), `~` the previous replacement, `\\r` a line break (`\\n` too; in Vim \
         it is a NUL), `\\t` a tab, `\\u` `\\l` the next character upper or lower case, `\\U` `\\L` up \
         to `\\E` or `\\e`, a backslash before any other character that character (`\\&`, `\\~`, `\\\\`, \
         the delimiter); `$1` is plain text. Flags: `g` (every match in a line), `i` / `I` (ignore \
         case, or not), `e` (no error when nothing matches), `&` first (the last flags again); `c` \
         (confirm) is not supported and says so. Any delimiter that is not a letter, digit, blank, \
         `\\`, `\"` or `|` works (`:s#a/b#c#`). `:s` alone and `:&` repeat the last `:s` without its \
         flags, `:&&` with them. Nothing matched: \"Pattern not found\". An editor command closes the line; \
         what went wrong is said in the status bar.\n\n",
    );
    s.push_str("| Command | Aliases | Description |\n|---|---|---|\n");
    for c in command::COMMANDS {
        let arg = match c.arg {
            Some(ArgKind::Profile) => " <profile>",
            Some(ArgKind::Setting) => " <setting>=<value>",
            Some(ArgKind::Script) => " [name]",
            Some(ArgKind::NewScript) => " [name]",
            Some(ArgKind::Format) => " <format>",
            Some(ArgKind::Context) => " [db][.schema]",
            None => "",
        };
        let aliases: Vec<String> = c.aliases.iter().map(|a| format!("`:{a}`")).collect();
        s.push_str(&format!("| `:{}{arg}` | {} | {} |\n", c.name, aliases.join(" "), en(c.label)));
    }
    s.push_str("\nSettings of `:set` (saved to `config.toml` like the matching actions):\n\n");
    s.push_str("| Setting | Values | Description |\n|---|---|---|\n");
    for st in command::SETTINGS {
        let values: Vec<String> = match st.values {
            command::Values::Fixed(v) => v.iter().map(|v| format!("`{}`", v.0)).collect(),
            command::Values::Themes => crate::theme::NAMES
                .iter()
                .map(|n| format!("`{n}`"))
                .chain(["or the name of a theme file".to_string()])
                .collect(),
        };
        s.push_str(&format!("| `{}` | {} | {} |\n", st.key, values.join(" "), en(st.label)));
    }
}

fn en(l: Label) -> &'static str {
    l.text(Lang::En)
}

fn section(s: &mut String, km: &Keymap, ctx: Ctx) {
    let own: Vec<&Bound> = km.bindings().iter().filter(|b| b.ctx == ctx).collect();
    let mut flags = String::new();
    if ctx.is_editor() {
        flags.push_str(" [editor]");
    }
    if ctx.is_text_input() {
        flags.push_str(" [text]");
    }
    s.push_str(&format!("\n## `{}`{flags}\n\n{}", ctx.name(), ctx.describe()));
    if let Some(p) = ctx.parent() {
        s.push_str(&format!(" Inside `{}`.", p.name()));
    }
    s.push('\n');
    let actions: Vec<&&Bound> = own.iter().filter(|b| matches!(b.target, Target::Action(_))).collect();
    if !actions.is_empty() {
        s.push_str("\n| Keys | Action | Description | R |\n|---|---|---|---|\n");
        for b in actions {
            let Target::Action(a) = b.target else { continue };
            let spec = action::spec(a);
            s.push_str(&format!(
                "| `{}` | `{}` | {} | {} |\n",
                keys::label(&b.keys),
                spec.id,
                en(spec.label),
                if spec.repeatable { "✓" } else { "" }
            ));
        }
    }
    // Leader groups that lead to at least one action (the others are hidden in the UI too).
    let groups: Vec<(&&Bound, Label)> = own
        .iter()
        .filter_map(|b| if let Target::Group(l) = b.target { Some((b, l)) } else { None })
        .filter(|(g, _)| {
            km.bindings().iter().any(|b| {
                matches!(b.target, Target::Action(_)) && b.keys.len() > g.keys.len() && b.keys.starts_with(&g.keys)
            })
        })
        .collect();
    if !groups.is_empty() {
        s.push_str("\nLeader groups (the which-key popup lists what follows):\n\n| Keys | Group |\n|---|---|\n");
        for (g, l) in groups {
            s.push_str(&format!("| `{}` | {} |\n", keys::label(&g.keys), en(l)));
        }
    }
    let mut notes: Vec<&str> = Vec::new();
    for b in &own {
        if let Target::Reserved(n) = b.target
            && !notes.contains(&n)
        {
            notes.push(n);
        }
    }
    for n in notes {
        let list: Vec<String> =
            own.iter().filter(|b| b.target == Target::Reserved(n)).map(|b| code(&keys::label(&b.keys))).collect();
        s.push_str(&format!("\nReserved — {n}: {}\n", list.join(" ")));
    }
}
