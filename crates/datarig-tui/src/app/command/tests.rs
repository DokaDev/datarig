use super::*;
use datarig_core::secret::{DefaultSource, SourceKind};

fn name(p: Parsed) -> Option<(&'static str, String, bool)> {
    match p {
        Parsed::Command { spec, arg, arg_started } => Some((spec.name, arg.to_string(), arg_started)),
        _ => None,
    }
}

#[test]
fn names_and_aliases_are_unique() {
    let mut all: Vec<&str> =
        COMMANDS.iter().flat_map(|c| std::iter::once(c.name).chain(c.aliases.iter().copied())).collect();
    let n = all.len();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n, "a name or alias is used twice");
    assert!(COMMANDS.iter().all(|c| !c.name.is_empty() && !c.name.contains(char::is_whitespace)));
}

#[test]
fn parses_commands_by_name_and_alias() {
    assert_eq!(parse(""), Parsed::Empty);
    assert_eq!(parse("   "), Parsed::Empty);
    assert_eq!(name(parse("conn")), Some(("conn", String::new(), false)));
    assert_eq!(name(parse("conn ")), Some(("conn", String::new(), true)), "a space starts the argument");
    assert_eq!(name(parse("  connect  prod db ")), Some(("conn", "prod db".into(), true)), "alias, argument trimmed");
    assert_eq!(name(parse("q")), Some(("quit", String::new(), false)));
    assert_eq!(name(parse("quit")), Some(("quit", String::new(), false)));
    assert_eq!(name(parse("h")), Some(("help", String::new(), false)));
    assert_eq!(name(parse("set language=ko")), Some(("set", "language=ko".into(), true)));
    assert_eq!(name(parse("run ")), Some(("run", String::new(), true)), "trailing space only");
}

#[test]
fn text_that_does_not_fit_a_command_is_a_search() {
    // No such command.
    assert_eq!(parse("korean"), Parsed::Text { word: "korean" });
    assert_eq!(parse("new connection"), Parsed::Text { word: "new" });
    // Commands are matched as a whole word, case-sensitively like vim.
    assert_eq!(parse("con"), Parsed::Text { word: "con" });
    assert_eq!(parse("Quit"), Parsed::Text { word: "Quit" });
    // A command without an argument followed by more words: an action name (`Run statement`).
    assert_eq!(parse("run statement"), Parsed::Text { word: "run" });
    assert_eq!(parse("quit now"), Parsed::Text { word: "quit" });
}

#[test]
fn set_accepts_known_settings_and_values_only() {
    assert_eq!(parse_set("language=ko"), Ok(Setting::Language(LangSetting::Ko)));
    assert_eq!(parse_set("language = auto"), Ok(Setting::Language(LangSetting::Auto)), "spaces around =");
    assert_eq!(parse_set("language=EN"), Ok(Setting::Language(LangSetting::En)), "values ignore case");
    assert_eq!(parse_set("editor=standard"), Ok(Setting::Editor(EditorMode::Standard)));
    assert_eq!(parse_set("editor=vim"), Ok(Setting::Editor(EditorMode::Vim)));
    assert_eq!(parse_set(""), Err(SetError::Usage));
    assert_eq!(parse_set("language"), Err(SetError::Usage), "no value");
    assert_eq!(parse_set("=ko"), Err(SetError::Usage));
    assert_eq!(parse_set("colour=red"), Err(SetError::UnknownKey("colour".into())));
    assert_eq!(
        parse_set("language=fr"),
        Err(SetError::BadValue { key: "language", value: "fr".into(), values: "en|ko|auto".into() })
    );
    assert_eq!(
        parse_set("editor="),
        Err(SetError::BadValue { key: "editor", value: String::new(), values: "vim|standard".into() })
    );
    assert_eq!(
        setting_keys(),
        "language|editor|icons|secrets.default_source|commands.position|detail_view|clipboard|copy_header|editor.cursor_shape"
    );
    assert_eq!(parse_set("editor.cursor_shape=off"), Ok(Setting::CursorShape(CursorShape::Off)));
    assert_eq!(parse_set("commands.position=bottom"), Ok(Setting::CommandsPosition(CommandsPosition::Bottom)));
    assert_eq!(parse_set("clipboard=OSC52"), Ok(Setting::Clipboard(ClipboardSetting::Osc52)));
    assert_eq!(parse_set("detail_view=statusbar"), Ok(Setting::DetailView(DetailView::Statusbar)));
    assert_eq!(parse_set("copy_header=off"), Ok(Setting::CopyHeader(CopyHeader::Off)));
    assert_eq!(
        parse_set("clipboard=pbcopy"),
        Err(SetError::BadValue { key: "clipboard", value: "pbcopy".into(), values: "auto|system|osc52".into() })
    );
    assert_eq!(
        parse_set("secrets.default_source=file"),
        Ok(Setting::DefaultSource(DefaultSource::Kind(SourceKind::File)))
    );
    assert_eq!(
        parse_set("secrets.default_source=vault"),
        Err(SetError::BadValue {
            key: "secrets.default_source",
            value: "vault".into(),
            values: "auto|keychain|file|command|env|prompt".into()
        })
    );
}

#[test]
fn every_setting_value_applies_to_its_own_setting() {
    for s in SETTINGS {
        for (value, setting, _) in s.values {
            assert_eq!(parse_set(&format!("{}={value}", s.key)), Ok(*setting));
        }
    }
}

#[test]
fn completes_profile_names() {
    let profiles = ["local-pg", "prod", "prod-replica", "分析-replica"];
    let got = |arg: &str| -> Vec<&str> {
        complete_arg(ArgKind::Profile, arg, &profiles)
            .into_iter()
            .map(|c| match c {
                ArgCompletion::Profile(i) => profiles[i],
                other => panic!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(got(""), profiles, "everything, in order");
    assert_eq!(got("prod"), ["prod", "prod-replica"], "exact first, then prefixes");
    assert_eq!(got("PROD-R"), ["prod-replica"], "ignores case");
    let mut fuzzy = got("rep");
    fuzzy.sort();
    assert_eq!(fuzzy, ["prod-replica", "分析-replica"], "then fuzzy matches");
    assert_eq!(got("分析"), ["分析-replica"]);
    assert!(got("zzz").is_empty());
}

#[test]
fn completes_setting_keys_then_values() {
    let got = |arg: &str| complete_arg(ArgKind::Setting, arg, &[]);
    assert_eq!(got(""), [0, 1, 2, 3, 4, 5, 6, 7, 8].map(ArgCompletion::SetKey));
    assert_eq!(got("c"), [4, 6, 7].map(ArgCompletion::SetKey));
    assert_eq!(got("commands.position="), [ArgCompletion::SetValue(4, 0), ArgCompletion::SetValue(4, 1)]);
    assert_eq!(got("sec"), [ArgCompletion::SetKey(3)]);
    assert_eq!(got("secrets.default_source=p"), [ArgCompletion::SetValue(3, 5)]);
    assert_eq!(got("icons=o"), [ArgCompletion::SetValue(2, 0), ArgCompletion::SetValue(2, 1)]);
    assert_eq!(parse_set("icons=OFF"), Ok(Setting::Icons(datarig_core::config::IconsSetting::Off)));
    assert_eq!(got("l"), [ArgCompletion::SetKey(0)]);
    assert_eq!(got("ed"), [ArgCompletion::SetKey(1), ArgCompletion::SetKey(8)]);
    assert_eq!(got("editor.cursor_shape="), [ArgCompletion::SetValue(8, 0), ArgCompletion::SetValue(8, 1)]);
    assert!(got("x").is_empty());
    assert_eq!(got("language="), [0, 1, 2].map(|v| ArgCompletion::SetValue(0, v)), "every value");
    assert_eq!(got("language=k"), [ArgCompletion::SetValue(0, 1)]);
    assert_eq!(got("editor=s"), [ArgCompletion::SetValue(1, 1)]);
    assert!(got("language=fr").is_empty(), "values are prefixes, not fuzzy");
    assert!(got("colour=").is_empty(), "no values for an unknown setting");
}

#[test]
fn use_reads_a_database_and_a_schema() {
    let s = |x: &str| Some(x.to_string());
    assert_eq!(parse_context("sales"), Some((s("sales"), None)));
    assert_eq!(parse_context("sales.shop"), Some((s("sales"), s("shop"))));
    assert_eq!(parse_context(".shop"), Some((None, s("shop"))));
    assert_eq!(parse_context("\"a.b\".\"My \"\"x\"\"\""), Some((s("a.b"), s("My \"x\""))));
    for bad in ["a.b.c", "\"open", ".", "a.", "\"\""] {
        assert_eq!(parse_context(bad), None, "{bad}");
    }
    // Unquoted names fold to lower case as in SQL; quoted ones keep their case.
    assert_eq!(parse_context("Sales.Shop"), Some((s("sales"), s("shop"))));
    assert_eq!(parse_context("\"Sales\".\"Shop\""), Some((s("Sales"), s("Shop"))));
    assert_eq!(parse_context(".\"MiXed\""), Some((None, s("MiXed"))));
}
