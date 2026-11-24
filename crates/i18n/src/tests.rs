use super::*;

/// The Korean catalog's text for `key`, read from `locales/ko/` when the test runs: the tests
/// hold no Korean text of their own.
fn ko_text(key: &str) -> String {
    let table: toml::Table = files::catalog("ko").parse().expect("the Korean catalog");
    table[key].as_str().expect("a string").to_string()
}

/// `query.done_rows` in Korean (one form for every count).
fn ko_rows(count: &str, elapsed: &str) -> String {
    ko_text("query.done_rows").replace("{count}", count).replace("{elapsed}", elapsed)
}

#[test]
fn labels_and_messages_render_in_each_language() {
    let mut i = I18n::new(Lang::Ko);
    assert_eq!(i.label(Label::PaneTreeTitle), ko_text("pane.tree.title").as_str());
    assert_eq!(
        i.msg(&Msg::QueryDoneRows { count: 1234, elapsed: Duration::from_millis(1234) }),
        ko_rows("1,234", "1.2s").as_str()
    );
    i.set_lang(Lang::En);
    assert_eq!(i.label(Label::PaneTreeTitle), "Explorer");
    assert_eq!(i.msg(&Label::QueryCancelled.into()), "Query cancelled");
    assert_eq!(Msg::ConnFailed { error: "x".into() }.key(), "conn.failed");
}

#[test]
fn plural_messages_pick_their_form_by_count() {
    let en = I18n::new(Lang::En);
    let ms = Duration::from_millis(3);
    assert_eq!(en.msg(&Msg::QueryDoneRows { count: 1, elapsed: ms }), "1 row · 3ms");
    assert_eq!(en.msg(&Msg::QueryDoneRows { count: 0, elapsed: ms }), "0 rows · 3ms");
    assert_eq!(en.msg(&Msg::QueryDoneRows { count: 2, elapsed: ms }), "2 rows · 3ms");
    assert_eq!(en.msg(&Msg::ProfilesDeletedTabs { name: "b".into(), count: 1 }), "Deleted b; 1 tab closed");
    assert_eq!(en.msg(&Msg::SecretMigrated { count: 1 }), "Moved 1 password from the config file to the OS keychain");
    // Korean has one form.
    let ko = I18n::new(Lang::Ko);
    assert_eq!(ko.msg(&Msg::QueryDoneRows { count: 1, elapsed: ms }), ko_rows("1", "3ms").as_str());
    assert_eq!(ko.msg(&Msg::QueryDoneRows { count: 2, elapsed: ms }), ko_rows("2", "3ms").as_str());
}

#[test]
fn every_message_fills_every_placeholder() {
    // The build checks that en and ko have the same placeholders per key; this checks the
    // generated rendering: every argument shows up formatted and no `{name}` is left over.
    for lang in [Lang::En, Lang::Ko] {
        for &l in Label::ALL {
            assert!(!l.text(lang).is_empty(), "{lang:?} {}", l.key());
        }
        for m in Msg::samples() {
            let text = m.render(lang);
            let en = m.render(Lang::En);
            for arg in ["1,234,567", "1.2s"].iter().copied().chain(sample_texts(&en)) {
                assert_eq!(text.matches(arg).count(), en.matches(arg).count(), "{lang:?} {}: {text}", m.key());
            }
            let leftover = text.split('{').skip(1).any(|t| {
                t.split_once('}')
                    .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            });
            assert!(!leftover, "{lang:?} {}: unfilled placeholder in {text}", m.key());
        }
    }
}

/// The `<name>` texts the samples pass for string placeholders.
fn sample_texts(s: &str) -> Vec<&str> {
    s.match_indices('<').filter_map(|(i, _)| s[i..].find('>').map(|j| &s[i..=i + j])).collect()
}

#[test]
fn count_and_elapsed() {
    assert_eq!(fmt_count(0), "0");
    assert_eq!(fmt_count(999), "999");
    assert_eq!(fmt_count(1000), "1,000");
    assert_eq!(fmt_count(4_000_000), "4,000,000");
    assert_eq!(fmt_elapsed(Duration::from_millis(123)), "123ms");
    assert_eq!(fmt_elapsed(Duration::from_millis(1234)), "1.2s");
}

#[test]
fn language_detection() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    };
    assert_eq!(detect_lang("auto", env(&[("LANG", "ko_KR.UTF-8")])), Lang::Ko);
    assert_eq!(detect_lang("auto", env(&[("LANG", "en_US.UTF-8")])), Lang::En);
    assert_eq!(detect_lang("en", env(&[("LANG", "ko_KR.UTF-8")])), Lang::En);
    assert_eq!(detect_lang("auto", env(&[("LC_ALL", "en_US.UTF-8"), ("LANG", "ko_KR.UTF-8")])), Lang::En);
    assert_eq!(detect_lang("auto", env(&[("LC_ALL", ""), ("LC_MESSAGES", "ko_KR"), ("LANG", "C")])), Lang::Ko);
    assert_eq!(detect_lang("auto", env(&[])), Lang::En);
}

#[test]
fn io_errors_read_as_friendly_localized_reasons() {
    use std::io::{Error, ErrorKind};
    let (en, ko) = (I18n::new(Lang::En), I18n::new(Lang::Ko));
    // ENAMETOOLONG / ERROR_FILENAME_EXCED_RANGE.
    let code = if cfg!(target_os = "linux") {
        36
    } else if cfg!(windows) {
        206
    } else {
        63
    };
    let too_long = Error::from_raw_os_error(code);
    let r = io_reason(too_long.kind());
    assert_eq!(en.msg(&r), "the name is too long for the file system");
    assert_eq!(ko.msg(&r), ko_text("io.name_too_long").as_str());
    let denied = Error::from(ErrorKind::PermissionDenied);
    assert_eq!(en.msg(&io_reason(denied.kind())), "permission denied");
    assert_eq!(en.msg(&io_reason(ErrorKind::StorageFull)), "the disk is full");
    // Other kinds get a general reason; the OS's text never reaches a message.
    assert_eq!(ko.msg(&io_reason(ErrorKind::Other)), ko_text("io.other").as_str());
    assert!(!en.msg(&io_reason(ErrorKind::Other)).contains("os error"));
}

/// Why a time with a time zone offset is not written as SQL, and why rows read on another
/// engine are not, in both languages.
#[test]
fn the_offset_literal_refusals_read_in_both_languages() {
    let (en, ko) = (I18n::new(Lang::En), I18n::new(Lang::Ko));
    let words = [
        (
            Label::CopySqlWhyVersionUnknown,
            "copy.sql_why.version_unknown",
            "The server version is not known, so a time with a time zone offset cannot be written safely as SQL. \
             Export as CSV or JSON still works, and so does copying from a session whose server version is known.",
        ),
        (
            Label::CopySqlWhyVersionNotKnownYet,
            "copy.sql_why.version_not_known_yet",
            "The server version was not known yet when these rows were read, so a time with a time zone offset \
             cannot be written safely as SQL. Run the query again, or export as CSV or JSON.",
        ),
        (
            Label::CopySqlWhyReadElsewhere,
            "copy.sql_why.read_elsewhere",
            "These rows were read on another database engine than the one the tab uses now; run the query again.",
        ),
        (
            Label::CopySqlWhyNoOffsetLiteral,
            "copy.sql_why.no_offset_literal",
            "This server does not read a time zone offset in a TIMESTAMP value (MariaDB, MySQL before 8.0.19), so \
             it cannot be copied or exported as SQL. Export as CSV or JSON still works.",
        ),
    ];
    for (label, key, english) in words {
        assert_eq!(en.label(label), english, "{key}");
        assert_eq!(ko.label(label), ko_text(key).as_str(), "{key}");
        assert_ne!(ko.label(label).to_string(), english, "{key}");
    }
    let reason = "R".to_string();
    let no_offset = Msg::CopySqlNoOffset { column: "at".into(), reason: reason.clone() };
    assert_eq!(en.msg(&no_offset), "Not copied as SQL: column “at” is a TIMESTAMP. R");
    let ko_no_offset = ko_text("copy.sql_no_offset").replace("{column}", "at").replace("{reason}", "R");
    assert_eq!(ko.msg(&no_offset), ko_no_offset.as_str());
    let refused = Msg::CopySqlRefused { reason };
    assert_eq!(en.msg(&refused), "Not copied as SQL. R");
    assert_eq!(ko.msg(&refused), ko_text("copy.sql_refused").replace("{reason}", "R").as_str());
}
