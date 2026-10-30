use super::*;
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
            Subject::Command(name) => assert!(command_exists(name), "{}: no command {name}", t.id),
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

/// A key-looking literal in `text` (keys come from the key bindings, never from the text).
fn literal_key(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for m in ["ctrl+", "shift+", "alt+", "cmd+", "meta+"] {
        if lower.contains(m) {
            return Some(m.into());
        }
    }
    let words: Vec<&str> = text.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    for w in &words {
        let f = w.strip_prefix('F').and_then(|n| n.parse::<u8>().ok());
        if ["Space", "Esc", "Escape", "Enter", "Return", "Tab", "Backspace"].contains(w)
            || f.is_some_and(|n| (1..=12).contains(&n))
        {
            return Some((*w).into());
        }
    }
    if text.contains('`') {
        return Some("`".into());
    }
    let chars: Vec<char> = text.chars().collect();
    for w in chars.windows(3) {
        if matches!((w[0], w[2]), ('“', '”') | ('"', '"') | ('\'', '\'') | ('‘', '’')) && !w[1].is_whitespace()
        {
            return Some(w.iter().collect());
        }
    }
    for w in chars.windows(2) {
        if w[0] == ':' && w[1].is_ascii_alphabetic() {
            return Some(w.iter().collect());
        }
    }
    None
}

#[test]
fn no_tip_names_a_key_itself() {
    for (name, catalog) in [("en", EN), ("ko", KO)] {
        let texts = entries(catalog, &["tips.", "onboarding.", "nudge.", "tip_mark"]);
        assert!(texts.len() > catalog_len(), "{name}: tips, steps and nudge");
        for (k, v) in texts {
            assert_eq!(literal_key(v), None, "{name} {k}: {v}");
        }
    }
    // The scan finds what it looks for.
    for bad in ["Ctrl+O opens", "press Space", "F5 runs", "`q` quits", "“n” adds", "run :ddl here", "Esc closes"] {
        assert!(literal_key(bad).is_some(), "{bad}");
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
    assert!(app.tip_open(t));
    assert!(off.setting_is(Setting::Tips(Tips::Off)) && !off.tip_open(t));
}
