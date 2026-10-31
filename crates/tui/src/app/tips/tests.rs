use super::*;
use crate::app::command::COMMANDS;
use crate::keymap::{Bound, Keymap, Target, parse_keys};
use datarig_core::i18n::Lang;

const EN: &str = include_str!("../../../../../locales/en.toml");
const KO: &str = include_str!("../../../../../locales/ko.toml");

/// The `"key" = "text"` entries of a catalog whose key starts with one of `prefixes`.
fn entries<'a>(catalog: &'a str, prefixes: &[&str]) -> Vec<(&'a str, &'a str)> {
    catalog
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(" = ")?;
            let k = k.strip_prefix('"')?.strip_suffix('"')?;
            let v = v.strip_prefix('"')?.strip_suffix('"')?;
            prefixes.iter().any(|p| k.starts_with(p)).then_some((k, v))
        })
        .collect()
}

fn app() -> App {
    let mut a = App::new(&datarig_core::config::Config::default(), None, Lang::En);
    a.prefs.tips = Tips::On;
    a
}

#[test]
fn every_tip_names_a_known_subject_and_has_its_own_text() {
    let defaults = Keymap::default();
    let mut ids: Vec<&str> = catalog().iter().map(|t| t.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), catalog().len(), "tip ids are unique");
    let mut keys = Vec::new();
    for t in catalog() {
        match t.subject {
            Subject::Action(a, ctx) => {
                let spec = action::spec(a);
                assert!(action::by_id(spec.id).is_some(), "{}: in the registry", t.id);
                assert!(
                    defaults.hint_keys(a, ctx, false).is_some(),
                    "{}: {} has a default key in {ctx:?}",
                    t.id,
                    spec.id
                );
            }
            Subject::Command(name) => {
                assert!(COMMANDS.iter().any(|c| c.name == name), "{}: no command {name}", t.id);
            }
            Subject::Setting(s) => assert!(setting_assignment(s).is_some(), "{}: not a value of the settings", t.id),
        }
        let key = (t.text)(String::new()).key();
        assert_eq!(key, format!("tips.{}", t.id), "the text of {} is its own catalog entry", t.id);
        keys.push(key);
    }
    // Every `tips.*` entry belongs to exactly one tip.
    for (k, _) in entries(EN, &["tips."]) {
        assert_eq!(keys.iter().filter(|x| **x == k).count(), 1, "{k}: used by one tip");
    }
    // The placeholder is the subject's kind.
    for t in catalog() {
        let (_, text) = entries(EN, &["tips."]).into_iter().find(|(k, _)| *k == format!("tips.{}", t.id)).unwrap();
        let want = match t.subject {
            Subject::Action(..) => "{key}",
            Subject::Command(_) => "{command}",
            Subject::Setting(_) => "{setting}",
        };
        assert!(text.contains(want), "tips.{}: {want}", t.id);
    }
    // The first steps and the key that hides them.
    for s in &STEPS {
        assert!(defaults.hint_keys(s.action, s.ctx, false).is_some(), "{:?} has a key in {:?}", s.action, s.ctx);
    }
    for ctx in [Ctx::Welcome, Ctx::Explorer] {
        assert!(defaults.hint_keys(Action::DismissOnboarding, ctx, false).is_some(), "{ctx:?}");
    }
}

/// Key names, lower case, that are words of their own.
const KEY_WORDS: &[&str] = &[
    "space",
    "spacebar",
    "esc",
    "escape",
    "enter",
    "return",
    "tab",
    "backspace",
    "del",
    "delete",
    "ins",
    "insert",
    "home",
    "end",
    "pgup",
    "pgdn",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
    "arrow",
    "arrows",
    "ctrl",
    "shift",
    "alt",
    "cmd",
    "meta",
    "super",
    "press",
];

/// Modifiers, lower case, that start a chord with `+` (and, but for the long words, with `-`).
const MODIFIERS: &[&str] = &["ctrl", "control", "shift", "alt", "option", "opt", "cmd", "command", "meta", "super"];

/// Key symbols: command, shift, option, control, return, enter, the arrows, tab, delete, escape,
/// page up and down.
const KEY_SYMBOLS: &[char] = &[
    '\u{2318}', '\u{21e7}', '\u{2325}', '\u{2303}', '\u{23ce}', '\u{21b5}', '\u{2190}', '\u{2192}', '\u{2191}',
    '\u{2193}', '\u{21e5}', '\u{232b}', '\u{2326}', '\u{238b}', '\u{21de}', '\u{21df}',
];

/// Korean key names: enter, space, tab, escape, control, shift, backspace, arrow key.
const KO_KEY_WORDS: &[&str] = &[
    "\u{c5d4}\u{d130}",
    "\u{c2a4}\u{d398}\u{c774}\u{c2a4}",
    "\u{d0ed}",
    "\u{c774}\u{c2a4}\u{cf00}\u{c774}\u{d504}",
    "\u{cee8}\u{d2b8}\u{b864}",
    "\u{c2dc}\u{d504}\u{d2b8}",
    "\u{bc31}\u{c2a4}\u{d398}\u{c774}\u{c2a4}",
    "\u{bc29}\u{d5a5}\u{d0a4}",
];

/// A key-looking literal in `text` (keys come from the key bindings, never from the text).
fn literal_key(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    // Chords: `Ctrl+O`, `Ctrl-O`, `C-x`, `M-x`, `^C`.
    for (i, w) in chars.windows(2).enumerate() {
        if w[0] == '^' && !w[1].is_whitespace() {
            return Some(w.iter().collect());
        }
        if !matches!(w[1], '+' | '-') || chars.get(i + 2).is_none_or(|c| c.is_whitespace()) {
            continue;
        }
        let word: String = chars[..=i].iter().rev().take_while(|c| c.is_ascii_alphabetic()).collect();
        let word: String = word.chars().rev().collect();
        let long = ["control", "option", "command"].contains(&word.as_str());
        if (MODIFIERS.contains(&word.as_str()) && (w[1] == '+' || !long))
            || (word.len() == 1 && "csma".contains(word.as_str()))
        {
            return Some(format!("{word}{}", w[1]));
        }
    }
    // Words: key names, function keys, a letter of its own (`press X`; `a` is a word).
    let words = text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '\'' || c == '\u{2019}'));
    for w in words.map(|w| w.trim_matches(|c| c == '\'' || c == '\u{2019}')) {
        let l = w.to_lowercase();
        let f = l.strip_prefix('f').and_then(|n| n.parse::<u8>().ok());
        let letter = w.len() == 1 && w.chars().all(|c| c.is_ascii_alphabetic()) && w != "a";
        if KEY_WORDS.contains(&l.as_str()) || f.is_some_and(|n| (1..=24).contains(&n)) || letter {
            return Some(w.into());
        }
    }
    if let Some(c) = text.chars().find(|c| *c == '`' || KEY_SYMBOLS.contains(c)) {
        return Some(c.into());
    }
    if let Some(k) = KO_KEY_WORDS.iter().find(|k| text.contains(**k)) {
        return Some((*k).into());
    }
    let chars: Vec<char> = text.chars().collect();
    // A quoted character: `"q"`, `'q'`.
    for w in chars.windows(3) {
        if matches!((w[0], w[2]), ('\u{201c}', '\u{201d}') | ('"', '"') | ('\'', '\'') | ('\u{2018}', '\u{2019}'))
            && !w[1].is_whitespace()
        {
            return Some(w.iter().collect());
        }
    }
    // A command: `:ddl`, `:1`, a full-width colon.
    for w in chars.windows(2) {
        if (w[0] == ':' && w[1].is_ascii_alphanumeric()) || (w[0] == '\u{ff1a}' && !w[1].is_whitespace()) {
            return Some(w.iter().collect());
        }
    }
    None
}

#[test]
fn no_tip_names_a_key_itself() {
    let prefixes = ["tips.", "onboarding.", "nudge.", "tip_mark", "setting.tips", "action.tips."];
    for (name, catalog) in [("en", EN), ("ko", KO)] {
        let texts = entries(catalog, &prefixes);
        assert!(texts.len() > catalog_len() + STEPS.len() + 4, "{name}: tips, steps, nudges and the setting");
        for (k, v) in texts {
            assert_eq!(literal_key(v), None, "{name} {k}: {v}");
        }
    }
    // The scan finds what it looks for.
    for bad in [
        "Ctrl+O opens",
        "ctrl-o opens",
        "C-x saves",
        "M-x runs",
        "^C stops",
        "alt+enter",
        "Alt-Enter",
        "Cmd K",
        "Meta K",
        "shift+tab",
        "press Space",
        "space runs",
        "esc closes",
        "Enter opens",
        "tab moves",
        "Del removes",
        "Delete removes",
        "Home goes",
        "End goes",
        "PgUp pages",
        "PgDn pages",
        "PageUp pages",
        "Backspace deletes",
        "the up arrow",
        "F5 runs",
        "f12 opens",
        "F24 runs",
        "press X to hide",
        "x hides",
        "`q` quits",
        "\u{201c}n\u{201d} adds",
        "'n' adds",
        "\"n\" adds",
        "run :ddl here",
        ":1 goes to the first line",
        "run \u{ff1a}ddl here",
        "\u{2318}K",
        "\u{21e7} selects",
        "\u{2325} moves",
        "\u{23ce} runs",
        "\u{2190} moves",
        "\u{2192} moves",
        "\u{2191} moves",
        "\u{2193} moves",
        // Korean: enter runs, the tab key moves.
        "\u{c5d4}\u{d130}\u{b85c} \u{c2e4}\u{d589}",
        "\u{d0ed} \u{d0a4}\u{b85c} \u{c774}\u{b3d9}",
        "\u{c2a4}\u{d398}\u{c774}\u{c2a4}",
        "\u{c774}\u{c2a4}\u{cf00}\u{c774}\u{d504}",
        "\u{cee8}\u{d2b8}\u{b864}",
        "\u{c2dc}\u{d504}\u{d2b8}",
        "X\u{b97c} \u{b204}\u{b974}\u{ba74}",
    ] {
        assert!(literal_key(bad).is_some(), "{bad}");
    }
    // Prose stays prose.
    for good in ["{key} opens a table's DDL", "the command-line history", "{key}: \u{c124}\u{c815}"] {
        assert_eq!(literal_key(good), None, "{good}");
    }
}

fn catalog_len() -> usize {
    catalog().len()
}

#[test]
fn a_remapped_key_shows_in_its_tip() {
    let default = app();
    for t in catalog() {
        let Subject::Action(a, ctx) = t.subject else { continue };
        let old = default.key_for(a, ctx);
        let mut app = app();
        let mut bindings: Vec<Bound> =
            Keymap::default().bindings().iter().filter(|b| b.target != Target::Action(a)).cloned().collect();
        bindings.push(Bound { ctx, keys: parse_keys("ctrl+f9").unwrap(), target: Target::Action(a), user: true });
        app.keymap = Keymap::from_bindings(bindings);
        let text = app.i18n.msg(&app.tip_msg(t)).to_string();
        assert!(text.contains("Ctrl+F9"), "{}: {text}", t.id);
        assert!(!text.contains(&old), "{}: {text} still names {old}", t.id);
    }
}

#[test]
fn settings_and_commands_fill_in_what_sets_them() {
    let app = app();
    assert_eq!(setting_assignment(Setting::Tips(Tips::Off)), Some(("ui.tips", "off")));
    assert_eq!(app.subject_text(Subject::Setting(Setting::Tips(Tips::Off))), ":set ui.tips=off");
    assert_eq!(app.subject_text(Subject::Command("theme")), ":theme");
    // Already off: that tip has nothing to say.
    let mut off = self::app();
    off.prefs.tips = Tips::Off;
    let t = catalog().iter().find(|t| t.id == "tips_off").unwrap();
    assert!(app.tip_open(t, Ctx::Nav));
    assert!(off.setting_is(Setting::Tips(Tips::Off)) && !off.tip_open(t, Ctx::Nav));
}

#[test]
fn command_and_setting_tips_show_only_where_the_colon_opens_the_command_line() {
    let app = app();
    for t in catalog() {
        if matches!(t.subject, Subject::Action(..)) {
            continue;
        }
        for ctx in [Ctx::Nav, Ctx::Explorer, Ctx::VimNormal, Ctx::Grid] {
            assert!(app.tip_open(t, ctx), "{} in {ctx:?}", t.id);
        }
        // Typing text, or a dialog: `:` does something else there.
        for ctx in [Ctx::VimInsert, Ctx::Confirm, Ctx::Settings, Ctx::Help, Ctx::ContextMenu, Ctx::Commands] {
            assert!(!app.tip_open(t, ctx), "{} in {ctx:?}", t.id);
        }
    }
}

#[test]
fn the_next_step_names_only_keys_that_work_where_it_shows() {
    let app = app();
    let text = |m: Msg| app.i18n.msg(&m).to_string();
    // The explorer (the workspace without tabs) hides the steps with its key; the editor has
    // none for it (there the key types or edits).
    let menu = STEPS.iter().find(|s| s.action == Action::OpenMenu).unwrap();
    let dismiss = app.key_for(Action::DismissOnboarding, Ctx::Explorer);
    assert!(text(app.step_msg(menu, Ctx::Explorer)).contains(&format!("({dismiss} hides the steps)")));
    for ctx in [Ctx::VimNormal, Ctx::VimInsert] {
        assert!(app.keymap.hint_keys(Action::DismissOnboarding, ctx, false).is_none(), "{ctx:?}");
        let run = STEPS.iter().find(|s| s.action == Action::RunStatement).unwrap();
        let t = text(app.step_msg(run, ctx));
        assert!(!t.contains("hides") && t.contains(&app.key_for(Action::RunStatement, ctx)), "{t}");
    }
    // No tab: running a query is not a step to take there.
    assert!(app.tabs.is_empty());
    let next = app.next_step(Ctx::Explorer).map(|i| STEPS[i].action);
    assert!(next.is_some_and(|a| a != Action::RunStatement), "{next:?}");
}

#[test]
fn a_remapped_key_shows_in_the_first_steps() {
    for s in &STEPS {
        let mut app = app();
        let mut bindings: Vec<Bound> =
            Keymap::default().bindings().iter().filter(|b| b.target != Target::Action(s.action)).cloned().collect();
        bindings.push(Bound {
            ctx: s.ctx,
            keys: parse_keys("ctrl+f9").unwrap(),
            target: Target::Action(s.action),
            user: true,
        });
        bindings.retain(|b| b.target != Target::Action(Action::DismissOnboarding));
        bindings.push(Bound {
            ctx: Ctx::Explorer,
            keys: parse_keys("ctrl+f8").unwrap(),
            target: Target::Action(Action::DismissOnboarding),
            user: true,
        });
        app.keymap = Keymap::from_bindings(bindings);
        // The welcome panel's row and the line of a place where its key works.
        assert_eq!(app.key_for(s.action, s.ctx), "Ctrl+F9", "{:?}", s.action);
        let t = app.i18n.msg(&app.step_msg(s, s.ctx)).to_string();
        assert!(t.contains("Ctrl+F9"), "{t}");
        let t = app.i18n.msg(&app.step_msg(s, Ctx::Explorer)).to_string();
        assert!(t.contains("Ctrl+F8 hides") && !t.contains("X hides"), "{t}");
    }
}

/// A use marks the usage file for writing only while its count can still change what the tips
/// do: up to the nudge, not on every later run.
#[test]
fn only_counts_that_matter_rewrite_the_usage_file() {
    let mut a = app();
    for n in 1..=NUDGE_AFTER + 2 {
        a.tips.dirty = false;
        a.dispatch_from(Action::Help, Source::Menu);
        a.overlays.close(crate::app::overlay::OverlayKind::Help);
        assert_eq!(a.tips.dirty, n <= NUDGE_AFTER, "use {n}");
    }
    assert_eq!(a.tips.usage.count(Via::Menu, action::spec(Action::Help).id), NUDGE_AFTER + 2);
}
