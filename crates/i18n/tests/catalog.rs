//! The locale checks of `build.rs` (`build/catalog.rs`), fed with fixtures: each problem the
//! build must reject is reported with its file, key and kind.

#[path = "../build/catalog.rs"]
mod catalog;

use catalog::{ArgType, Locale, Piece, Policy, Problem, ProblemKind};

fn locale(code: &str, policy: Policy, src: &str) -> Locale {
    let file = format!("{code}.toml");
    let parsed = catalog::parse(&file, src).unwrap_or_else(|e| panic!("{e:?}"));
    Locale::single(code, &file, policy, parsed)
}

/// A locale of area files (`locales/<code>/<area>.toml`): (area, text).
fn areas(code: &str, policy: Policy, files: &[(&str, &str)]) -> Result<Locale, Vec<Problem>> {
    let files: Vec<(String, String)> = files.iter().map(|(a, t)| (a.to_string(), t.to_string())).collect();
    catalog::merge(code, &format!("locales/{code}"), policy, &files)
}

/// The shipped catalog files of `code`: (area, text), by area.
fn shipped_files(code: &str) -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales").join(code);
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("a locale directory")
        .map(|e| e.expect("a directory entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .map(|p| {
            let area = p.file_stem().and_then(|s| s.to_str()).expect("a file name").to_string();
            (area, std::fs::read_to_string(&p).expect("a catalog file"))
        })
        .collect();
    files.sort();
    files
}

/// The shipped catalog of `code`, its area files merged as the build merges them.
fn shipped(code: &str, policy: Policy) -> Locale {
    catalog::merge(code, &format!("locales/{code}"), policy, &shipped_files(code)).unwrap_or_else(|e| panic!("{e:?}"))
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
    assert_eq!(catalog::pieces("Keys: {context}"), vec![text("Keys: "), ph("context")]);
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
fn area_files_merge_into_one_catalog() {
    let split =
        areas("en", Policy::Source, &[("conn", r#""conn.failed" = "Connection failed: {error}""#), ("pane", EN_PANE)])
            .unwrap();
    assert_eq!(split.entries.len(), 3);
    assert_eq!(split.one.len(), 1);
    assert_eq!(split.area["query.done"], "pane");
    let ko = areas("ko", Policy::Required, &[("conn", r#""conn.failed" = "接続失敗: {error}""#), ("pane", KO_PANE)])
        .unwrap();
    assert_eq!(catalog::check(&[split.clone(), ko]), (vec![], vec![]));
    // The generated code does not depend on how the keys are spread over files.
    assert_eq!(catalog::generate(&[split]), catalog::generate(&[en()]));
}

const EN_PANE: &str = r#"
"pane.title" = "Explorer"
"query.done" = { one = "{count} row · {elapsed}", other = "{count} rows · {elapsed}" }
"#;

const KO_PANE: &str = r#"
"pane.title" = "探索機"
"query.done" = "{count}行 · {elapsed}"
"#;

#[test]
fn a_key_defined_in_two_files_fails() {
    let errors =
        areas("en", Policy::Source, &[("a", r#""pane.title" = "Explorer""#), ("b", r#""pane.title" = "Tree""#)])
            .unwrap_err();
    let other = "locales/en/a.toml".to_string();
    assert_eq!(errors, vec![problem("locales/en/b.toml", "pane.title", ProblemKind::Duplicate { other })]);
    assert_eq!(
        errors[0].to_string(),
        "locales/en/b.toml: key `pane.title`: also defined in locales/en/a.toml; keep one of them"
    );
}

#[test]
fn a_translation_keeps_each_key_in_the_file_of_the_same_name() {
    let en =
        areas("en", Policy::Source, &[("conn", r#""conn.failed" = "Connection failed: {error}""#), ("pane", EN_PANE)])
            .unwrap();
    let ko = areas(
        "ko",
        Policy::Required,
        &[("conn", ""), ("pane", &format!("{KO_PANE}\"conn.failed\" = \"接続失敗: {{error}}\""))],
    )
    .unwrap();
    let (errors, _) = catalog::check(&[en, ko]);
    let expected = "conn.toml".to_string();
    assert_eq!(errors, vec![problem("locales/ko/pane.toml", "conn.failed", ProblemKind::OtherFile { expected })]);
    assert_eq!(
        errors[0].to_string(),
        "locales/ko/pane.toml: key `conn.failed`: the source catalog has it in conn.toml; keep each key in the file of the same name in every locale"
    );
}

#[test]
fn a_translation_file_the_source_lacks_fails() {
    let en = areas("en", Policy::Source, &[("pane", EN_PANE)]).unwrap();
    let ko = areas("ko", Policy::Required, &[("pane", KO_PANE), ("panes", "")]).unwrap();
    let (errors, _) = catalog::check(&[en, ko]);
    assert_eq!(errors, vec![problem("locales/ko/panes.toml", "", ProblemKind::FileNotInSource)]);
    assert_eq!(
        errors[0].to_string(),
        "locales/ko/panes.toml: the source catalog has no file of this name; move its keys to the files the source has them in"
    );
}

#[test]
fn a_missing_key_names_the_file_it_belongs_in() {
    let en =
        areas("en", Policy::Source, &[("conn", r#""conn.failed" = "Connection failed: {error}""#), ("pane", EN_PANE)])
            .unwrap();
    let ko = areas("ko", Policy::Required, &[("conn", ""), ("pane", KO_PANE)]).unwrap();
    let (errors, _) = catalog::check(&[en.clone(), ko]);
    assert_eq!(errors, vec![problem("locales/ko/conn.toml", "conn.failed", ProblemKind::Missing)]);
    // The same when the translation has no such file at all.
    let ko = areas("ko", Policy::Required, &[("pane", KO_PANE)]).unwrap();
    let (errors, _) = catalog::check(&[en, ko]);
    assert_eq!(errors, vec![problem("locales/ko/conn.toml", "conn.failed", ProblemKind::Missing)]);
}

#[test]
fn shipped_catalogs_pass() {
    let (en, ko) = (shipped("en", Policy::Source), shipped("ko", Policy::Required));
    assert!(en.areas.len() > 10 && en.entries.len() > 3000, "{:?}", en.areas);
    assert_eq!(catalog::check(&[en, ko]), (vec![], vec![]));
}

/// Hangul in the ranges of its jamo, compatibility jamo and syllables.
fn hangul(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF)
}

/// Korean appears only in the values of the Korean catalog: never in the English files (a
/// Korean mark in an English text is written with `\u` escapes), nor in a key or a comment.
#[test]
fn shipped_catalogs_have_hangul_only_in_korean_values() {
    let mut wrong = vec![];
    for code in ["en", "ko"] {
        for (area, text) in shipped_files(code) {
            for (n, line) in text.lines().enumerate() {
                // In a Korean entry, what follows the first `=` is the value.
                let outside =
                    if code == "ko" && line.starts_with('"') { line.split('=').next().unwrap_or("") } else { line };
                if outside.chars().any(hangul) {
                    wrong.push(format!("locales/{code}/{area}.toml:{}: {line}", n + 1));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "Hangul outside the Korean values:\n{}", wrong.join("\n"));
    // The scan finds what it looks for.
    assert!("\u{D55C}".chars().any(hangul) && !"\\uD55C/A".chars().any(hangul));
}

/// Keys whose Korean text is rightly the same as the English one. Anything else with the
/// English text in the Korean catalog is an entry nobody translated.
const SAME_IN_KO: &[&str] = &[
    // The product name.
    "app.title",
    // Placeholders and punctuation only.
    "results.title.page_short",
    "plan.card.share_short",
    "query.done_command",
    "editor.hint.command",
    "setting.changed",
    "detail.preview",
    "ssh.failed",
    "edit.paste.value_reason",
    "edit.badge.pending_invalid_short",
    "edit.preview.at_row",
    "edit.preview.invalid_why",
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
    "setting.copy_format.csv",
    "setting.copy_format.json",
    "setting.copy_format.xml",
    "copy.menu.markdown",
    "copy.menu.insert",
    "copy.menu.update",
    "export.format.parquet",
    // Kept in English, as the Korean hint of the same form (`form.ssh.on_hint`) writes it.
    "form.field.ssh_host",
    // A file, its line and the (already translated) problem, in the compilers' form.
    "theme.file_error",
    "theme.file_error_no_line",
    // The word typed after `:explain`, as SQL spells it.
    "commands.arg.analyze",
    // The words typed after `:theme`.
    "commands.arg.theme",
    // A filter by a cell's value: SQL's own operators and keywords around the value.
    "table.filter.by_value.eq",
    "table.filter.by_value.ne",
    "table.filter.by_value.lt",
    "table.filter.by_value.gt",
    "table.filter.by_value.le",
    "table.filter.by_value.ge",
    "table.filter.by_value.is_null",
    "table.filter.by_value.is_not_null",
    "table.filter.by_value.in",
    "table.filter.by_value.not_in",
];

#[test]
fn shipped_korean_catalog_translates_every_entry() {
    let (en, ko) = (shipped("en", Policy::Source), shipped("ko", Policy::Required));
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
        "the Korean catalog has the English text for {untranslated:?}; translate it, or add the key to SAME_IN_KO with the reason"
    );
    // A key that got a translation leaves the list, so the list stays a list of exceptions.
    let stale: Vec<_> = SAME_IN_KO.iter().filter(|k| !same.contains(k)).collect();
    assert!(stale.is_empty(), "SAME_IN_KO lists keys that are translated or gone: {stale:?}");
}
