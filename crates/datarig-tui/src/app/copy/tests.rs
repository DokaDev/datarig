use super::*;

#[test]
fn a_range_is_the_rectangle_between_anchor_and_cursor() {
    assert_eq!(range((2, 1), (4, 3)), (2..5, 1..4));
    assert_eq!(range((4, 3), (2, 1)), (2..5, 1..4), "either direction");
    assert_eq!(range((0, 0), (0, 0)), (0..1, 0..1), "one cell");
}

#[test]
fn formats_have_names_keys_and_labels() {
    let names: Vec<&str> = CopyFormat::MENU.iter().map(|f| f.name()).collect();
    assert_eq!(
        names,
        [
            "tsv",
            "tsv_header",
            "list",
            "csv",
            "json",
            "json_pretty",
            "markdown",
            "html",
            "xml",
            "in",
            "insert",
            "update"
        ]
    );
    let keys: String = CopyFormat::MENU.iter().map(|f| f.key()).collect();
    assert_eq!(keys, "tTlcjJmhxniu", "one key each");
    for f in CopyFormat::MENU {
        assert_eq!(CopyFormat::parse(f.name()), Some(f));
    }
    assert_eq!(CopyFormat::parse("SQL"), Some(CopyFormat::SqlInsert), "the older name");
    assert_eq!(CopyFormat::parse("json-pretty"), Some(CopyFormat::JsonPretty));
    assert_eq!(CopyFormat::parse("yaml"), None);
    let i18n = I18n::new(Lang::En);
    assert_eq!(i18n.label(CopyFormat::SqlInsert.label()), "SQL INSERT");
    assert_eq!(i18n.label(CopyFormat::TsvHeader.menu_label()), "With headers (TSV)");
}
