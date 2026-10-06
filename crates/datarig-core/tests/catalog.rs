//! The locale checks of `build.rs` (`build/catalog.rs`), fed with fixtures: each problem the
//! build must reject is reported with its file, key and kind.

#[path = "../build/catalog.rs"]
mod catalog;

use catalog::{ArgType, Locale, Piece, Policy, Problem, ProblemKind};

fn locale(code: &str, policy: Policy, src: &str) -> Locale {
    let file = format!("{code}.toml");
    let (entries, one) = catalog::parse(&file, src).unwrap_or_else(|e| panic!("{e:?}"));
    Locale { code: code.to_string(), file, policy, entries, one }
}

// The translated fixtures below are in any language other than English: the checks look at
// keys, placeholders and plural forms, not at the words.
const EN: &str = r#"
"pane.title" = "Explorer"
"conn.failed" = "Connection failed: {error}"
"query.done" = { one = "{count} row · {elapsed}", other = "{count} rows · {elapsed}" }
"#;

fn en() -> Locale {
    locale("en", Policy::Source, EN)
}

fn problem(file: &str, key: &str, kind: ProblemKind) -> Problem {
    Problem { file: file.to_string(), key: key.to_string(), kind }
}

#[test]
fn complete_translation_passes() {
    let ko = locale(
        "ko",
        Policy::Required,
        r#"
"pane.title" = "探索機"
"conn.failed" = "接続失敗: {error}"
"query.done" = "{count}行 · {elapsed}"
"#,
    );
    assert_eq!(catalog::check(&[en(), ko]), (vec![], vec![]));
}

#[test]
fn missing_key_fails_a_required_locale() {
    let ko = locale(
        "ko",
        Policy::Required,
        r#""conn.failed" = "接続失敗: {error}"
"query.done" = "{count}行 · {elapsed}""#,
    );
    let (errors, warnings) = catalog::check(&[en(), ko]);
    assert_eq!(errors, vec![problem("ko.toml", "pane.title", ProblemKind::Missing)]);
    assert!(warnings.is_empty());
    assert_eq!(
        errors[0].to_string(),
        "ko.toml: key `pane.title`: missing (every key of the source catalog must be translated)"
    );
}

#[test]
fn extra_key_fails() {
    let ko = locale(
        "ko",
        Policy::Optional,
        r#""pane.title" = "探索機"
"conn.failed" = "接続失敗: {error}"
"query.done" = "{count}行 · {elapsed}"
"pane.titel" = "誤字""#,
    );
    let (errors, _) = catalog::check(&[en(), ko]);
    assert_eq!(errors, vec![problem("ko.toml", "pane.titel", ProblemKind::Extra)]);
    assert!(errors[0].to_string().contains("not in the source catalog"), "{}", errors[0]);
}

#[test]
fn placeholder_mismatch_fails() {
    let ko = locale(
        "ko",
        Policy::Required,
        r#""pane.title" = "探索機"
"conn.failed" = "接続失敗: {err}"
"query.done" = "{count}行""#,
    );
    let (errors, _) = catalog::check(&[en(), ko]);
    let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        errors,
        vec![
            problem(
                "ko.toml",
                "conn.failed",
                ProblemKind::Placeholders { expected: strings(&["error"]), found: strings(&["err"]) }
            ),
            problem(
                "ko.toml",
                "query.done",
                ProblemKind::Placeholders { expected: strings(&["count", "elapsed"]), found: strings(&["count"]) }
            ),
        ]
    );
    assert_eq!(
        errors[0].to_string(),
        "ko.toml: key `conn.failed`: placeholders differ from the source catalog (expected {error}, found {err})"
    );
}

#[test]
fn optional_locale_warns_and_falls_back_to_the_source() {
    // The policy switch for a partial translation: missing keys warn instead of failing.
    let ja = locale("ja", Policy::Optional, r#""pane.title" = "エクスプローラー""#);
    let (errors, warnings) = catalog::check(&[en(), ja.clone()]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        warnings,
        vec![
            problem("ja.toml", "conn.failed", ProblemKind::Missing),
            problem("ja.toml", "query.done", ProblemKind::Missing)
        ]
    );
    let code = catalog::generate(&[en(), ja]);
    assert!(code.contains(r#"Lang::Ja => format!("Connection failed: {error}"),"#), "{code}");
    assert!(code.contains(r#"Label::PaneTitle => "エクスプローラー","#), "{code}");
}

#[test]
fn source_catalog_rules() {
    let src = locale(
        "en",
        Policy::Source,
        r#"
"Bad.Key" = "x"
"a.b_c" = "y"
"a.b.c" = "z"
"kw" = "{type}"
"#,
    );
    assert_eq!(
        catalog::check(&[src]).0,
        vec![
            problem("en.toml", "Bad.Key", ProblemKind::BadKey),
            problem("en.toml", "a.b_c", ProblemKind::NameClash { other: "a.b.c".to_string() }),
            problem("en.toml", "kw", ProblemKind::BadPlaceholder("type".to_string())),
        ]
    );
}

#[test]
fn parse_errors_name_the_file() {
    let e = catalog::parse("ko.toml", r#""a" = "#).unwrap_err();
    assert!(matches!(&e[0].kind, ProblemKind::Syntax(_)) && e[0].to_string().starts_with("ko.toml: not valid TOML"));
    let e = catalog::parse("ko.toml", "a = 1\n").unwrap_err();
    assert_eq!(e, vec![problem("ko.toml", "a", ProblemKind::NotAString)]);
    // A table is a plural entry: exactly `one` and `other`, both strings.
    for bad in [
        "[a]\nb = 1\n",
        "a = { one = \"x\" }\n",
        "a = { one = \"x\", other = 2 }\n",
        "a = { one = \"x\", other = \"y\", few = \"z\" }\n",
    ] {
        let e = catalog::parse("ko.toml", bad).unwrap_err();
        assert_eq!(e, vec![problem("ko.toml", "a", ProblemKind::BadPlural)], "{bad}");
    }
}

#[test]
fn plural_rules_of_the_source_catalog() {
    let src = locale(
        "en",
        Policy::Source,
        r#"
"a.count" = "{count} rows"
"b.nocount" = { one = "a row", other = "rows" }
"c.forms" = { one = "{count} row in {schema}", other = "{count} rows" }
"d.pseudo" = "Moved password(s)"
"e.ok" = { one = "its tab", other = "its {count} tabs" }
"#,
    );
    let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let errors = catalog::check(&[src]).0;
    assert_eq!(
        errors,
        vec![
            problem("en.toml", "a.count", ProblemKind::PluralMissing),
            problem("en.toml", "b.nocount", ProblemKind::PluralWithoutCount),
            problem(
                "en.toml",
                "c.forms",
                ProblemKind::PluralForms { one: strings(&["count", "schema"]), other: strings(&["count"]) }
            ),
            problem("en.toml", "d.pseudo", ProblemKind::PseudoPlural),
        ]
    );
    assert_eq!(
        errors[0].to_string(),
        r#"en.toml: key `a.count`: has {count}, so it must be plural: { one = "…", other = "…" }"#
    );
}

#[test]
fn plural_rules_of_a_translation() {
    // A plain string is fine for a plural entry (no plural forms, like Korean); forms for an
    // entry that is not plural in the source are not.
    let ko = locale(
        "ko",
        Policy::Required,
        r#"
"pane.title" = { one = "探索機 {count}", other = "探索機 {count}" }
"conn.failed" = "接続失敗: {error}"
"query.done" = "{count}行 · {elapsed}"
"#,
    );
    let (errors, _) = catalog::check(&[en(), ko]);
    assert_eq!(
        errors,
        vec![
            problem(
                "ko.toml",
                "pane.title",
                ProblemKind::Placeholders { expected: vec![], found: vec!["count".to_string()] }
            ),
            problem("ko.toml", "pane.title", ProblemKind::PluralNotInSource),
        ]
    );
    let ko = locale(
        "ko",
        Policy::Required,
        r#"
"pane.title" = "探索機"
"conn.failed" = "接続失敗: {error}"
"query.done" = { one = "{count}行 · {elapsed}", other = "{count}行 · {elapsed} {error}" }
"#,
    );
    let (errors, _) = catalog::check(&[en(), ko]);
    assert!(errors.iter().any(|p| matches!(p.kind, ProblemKind::Placeholders { .. })), "{errors:?}");
    assert!(errors.iter().any(|p| matches!(p.kind, ProblemKind::PluralForms { .. })), "{errors:?}");
}

#[test]
fn generated_code_picks_the_plural_form_by_count() {
    let ko = locale(
        "ko",
        Policy::Required,
        r#"
"pane.title" = "探索機"
"conn.failed" = "接続失敗: {error}"
"query.done" = "{count}行 · {elapsed}"
"#,
    );
    let code = catalog::generate(&[en(), ko]);
    assert!(code.contains("let plural_one = *count == 1;"), "{code}");
    assert!(
        code.contains(
            r#"Lang::En => if plural_one { format!("{count} row · {elapsed}") } else { format!("{count} rows · {elapsed}") },"#
        ),
        "{code}"
    );
    assert!(code.contains(r#"Lang::Ko => format!("{count}行 · {elapsed}"),"#), "{code}");
    // A `one` form without placeholders is a plain string; identical forms need no branch.
    let src = r#"
"a" = { one = "its tab", other = "its {count} tabs" }
"b" = { one = "{count}+ rows", other = "{count}+ rows" }
"#;
    let code = catalog::generate(&[locale("en", Policy::Source, src)]);
    assert!(
        code.contains(r#"Lang::En => if plural_one { "its tab".to_string() } else { format!("its {count} tabs") },"#),
        "{code}"
    );
    assert!(code.contains(r#"Lang::En => format!("{count}+ rows"),"#), "{code}");
    assert_eq!(code.matches("let plural_one").count(), 1, "{code}");
}

#[test]
fn templates_split_into_text_and_placeholders() {
    let text = |s: &str| Piece::Text(s.to_string());
    let ph = |s: &str| Piece::Placeholder(s.to_string());
    assert_eq!(catalog::pieces("Keys — {context}"), vec![text("Keys — "), ph("context")]);
    assert_eq!(catalog::pieces("{a}{b}"), vec![ph("a"), ph("b")]);
    // Braces that do not wrap a lowercase identifier are text.
    assert_eq!(catalog::pieces("{} {Name} {a-b} {"), vec![text("{} {Name} {a-b} {")]);
    assert_eq!(catalog::placeholders("{b} {a} {b}"), vec!["a", "b"]);
    assert_eq!(catalog::variant_name("conn.disconnected_tx"), "ConnDisconnectedTx");
    assert_eq!(catalog::arg_type("count"), ArgType::Count);
    assert_eq!(catalog::arg_type("latency"), ArgType::Elapsed);
    assert_eq!(catalog::arg_type("name"), ArgType::Text);
}

#[test]
fn generated_code_types_the_arguments() {
    let code = catalog::generate(&[en()]);
    assert!(code.contains("    ConnFailed { error: String },"), "{code}");
    assert!(code.contains("    QueryDone { count: u64, elapsed: std::time::Duration },"), "{code}");
    assert!(code.contains("    PaneTitle,"), "{code}");
    // Literal braces stay literal in the generated `format!`.
    let code = catalog::generate(&[locale("en", Policy::Source, r#""a" = "{} {n}""#)]);
    assert!(code.contains(r#"Lang::En => format!("{{}} {n}"),"#), "{code}");
}

#[test]
fn shipped_catalogs_pass() {
    let en = locale("en", Policy::Source, include_str!("../../../locales/en.toml"));
    let ko = locale("ko", Policy::Required, include_str!("../../../locales/ko.toml"));
    assert_eq!(catalog::check(&[en, ko]), (vec![], vec![]));
}

/// Keys whose Korean text is rightly the same as the English one. Anything else with the
/// English text in `ko.toml` is an entry nobody translated.
const SAME_IN_KO: &[&str] = &[
    // The product name.
    "app.title",
    // Placeholders and punctuation only.
    "results.title.page_short",
    "query.done_command",
    "setting.changed",
    "detail.preview",
    "ssh.failed",
    // SQL's own keyword, shown as the value.
    "results.null",
    // Short marks that fit a tab title or a tree row; the help explains them.
    "tab.read_only",
    "explorer.read_only",
    // The input mode indicator shows both scripts in either language.
    "status.hangul",
    // Protocol, tool and format names.
    "form.field.dsn",
    "tab_list.kind.ddl",
    "form.section.ssh",
    "form.ssh.auth_agent",
    "copy.method.osc52",
    "copy.format.tsv",
    "copy.format.csv",
    "copy.format.json",
    "copy.format.xml",
    "copy.format.sql",
    "copy.format.update",
    "copy.menu.csv",
    "copy.menu.json",
    "copy.menu.xml",
    "copy.menu.markdown",
    "copy.menu.insert",
    "copy.menu.update",
    // Kept in English, as the Korean hint of the same form (`form.ssh.on_hint`) writes it.
    "form.field.ssh_host",
    // A file, its line and the (already translated) problem, in the compilers' form.
    "theme.file_error",
    "theme.file_error_no_line",
    // The word typed after `:explain`, as SQL spells it.
    "commands.arg.analyze",
];

#[test]
fn shipped_korean_catalog_translates_every_entry() {
    let en = locale("en", Policy::Source, include_str!("../../../locales/en.toml"));
    let ko = locale("ko", Policy::Required, include_str!("../../../locales/ko.toml"));
    let english = |key: &str| [en.entries.get(key), en.one.get(key)].into_iter().flatten().collect::<Vec<_>>();
    let mut untranslated = vec![];
    let mut same = vec![];
    for (key, text) in &ko.entries {
        let copied = [Some(text), ko.one.get(key)].into_iter().flatten().any(|t| english(key).contains(&t));
        match (copied, SAME_IN_KO.contains(&key.as_str())) {
            (true, false) => untranslated.push(key.as_str()),
            (true, true) => same.push(key.as_str()),
            (false, _) => {}
        }
    }
    assert!(
        untranslated.is_empty(),
        "ko.toml has the English text for {untranslated:?}; translate it, or add the key to SAME_IN_KO with the reason"
    );
    // A key that got a translation leaves the list, so the list stays a list of exceptions.
    let stale: Vec<_> = SAME_IN_KO.iter().filter(|k| !same.contains(k)).collect();
    assert!(stale.is_empty(), "SAME_IN_KO lists keys that are translated or gone: {stale:?}");
}
