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
         place (U+3153 = `j`, U+D558 = `g k`); the status bar then shows the Korean input-source mark.\n\
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
         - **Reserved** keys belong to a widget (vim, standard editing, a dialog) or to an action that is not \
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

/// The `:` commands and the settings of `:set`.
fn commands(s: &mut String) {
    s.push_str(
        "\n## `:` commands\n\nType them after `:` (or `Ctrl+K`). `Tab`/`Shift+Tab`, `↑`/`↓` or `Ctrl+N`/`Ctrl+P` \
         pick an entry, `Enter` runs it (a command that still needs its argument is completed instead), `Esc` or \
         `Backspace` on an empty line closes. Text that is not a command searches the actions by name. A command \
         that cannot run shows an error and the line stays open.\n\n",
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
        let list: Vec<String> = own
            .iter()
            .filter(|b| b.target == Target::Reserved(n))
            .map(|b| {
                let cond = if b.when.is_some() { " (with a selection)" } else { "" };
                format!("`{}`{cond}", keys::label(&b.keys))
            })
            .collect();
        s.push_str(&format!("\nReserved — {n}: {}\n", list.join(" ")));
    }
}
