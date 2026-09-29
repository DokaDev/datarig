use super::*;
use crate::text::width;

#[test]
fn every_glyph_is_one_private_use_char_one_column_wide() {
    let glyphs = CURATED.iter().map(|c| c.2).chain(DRIVERS.iter().map(|d| d.2)).chain([
        UNKNOWN_DRIVER,
        SCRIPT,
        WARNING,
        KEY_PK,
        KEY_FK,
        KEY_UQ,
    ]);
    for g in glyphs {
        let mut chars = g.chars();
        let c = chars.next().unwrap();
        assert!(chars.next().is_none(), "{g:?}: one char");
        assert!(('\u{e000}'..='\u{f8ff}').contains(&c), "{g:?}: in the Private Use Area");
        assert_eq!(width(g), 1, "{g:?}: one column (the cell pads it to {WIDTH})");
    }
}

#[test]
fn names_are_unique_and_stable() {
    assert_eq!(CURATED.len(), 24);
    let mut names: Vec<&str> = CURATED.iter().map(|c| c.0).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CURATED.len());
    // The plan's list, by name.
    for n in ["database", "server", "cloud", "leaf", "rocket", "chart", "warning"] {
        assert!(by_name(n).is_some(), "{n}");
    }
    assert_eq!(by_name("database"), Some("\u{f1c0}"));
    assert_eq!(for_driver("postgres"), "\u{e76e}");
    assert_eq!(for_driver("pg"), "\u{e76e}");
    assert_eq!(for_driver("elasticsearch"), "\u{e7ca}");
    assert_eq!(for_driver("oracle"), UNKNOWN_DRIVER);
}

#[test]
fn profile_icon_falls_back_to_the_driver_and_off_keeps_alignment() {
    let mut p = ConnectionConfig { name: "a".into(), ..ConnectionConfig::default() };
    assert_eq!(glyph(&p), for_driver("postgres"));
    p.icon = Some("rocket".into());
    assert_eq!(glyph(&p), "\u{f135}");
    p.icon = Some("no-such-icon".into());
    assert_eq!(glyph(&p), for_driver("postgres"), "unknown names use the driver's icon");
    assert_eq!(width(&cell(&p, true)), WIDTH);
    assert_eq!(cell(&p, false), "  ", "off: blank but the same width");
    assert_eq!(width(&preview()), 7);
}

#[test]
fn warning_has_a_text_form_without_icons() {
    // nf-fa-warning (Nerd Fonts 3.5.1 glyphnames.json).
    assert_eq!((warning(true), warning(false)), ("\u{f071}", "!"));
}

#[test]
fn key_marks_are_glyphs_or_letters_primary_key_first() {
    use datarig_core::driver::KeyMarks;
    // nf-fa-key, nf-fa-link, nf-fa-fingerprint (Nerd Fonts 3.5.1 glyphnames.json).
    assert_eq!((KEY_PK, KEY_FK, KEY_UQ), ("\u{f084}", "\u{f0c1}", "\u{ee40}"));
    let all = KeyMarks { pk: true, fk: true, unique: true };
    assert_eq!(key_marks(all), [KeyMark::Pk, KeyMark::Fk, KeyMark::Uq]);
    assert_eq!(key_marks_text(all, false), "PK FK UQ ");
    assert_eq!(key_marks_text(KeyMarks { fk: true, ..KeyMarks::default() }, true), "\u{f0c1} ");
    assert_eq!(key_marks_text(KeyMarks::default(), true), "");
    assert_eq!(width(&key_marks_text(all, true)), 6, "one column each, and a space");
}

#[test]
fn tree_and_type_icons_are_pinned_by_name() {
    // Checked against glyphnames.json of Nerd Fonts 3.5.1: each name and its code point.
    let tree: Vec<(&str, u32)> = TREE.iter().map(|t| (t.1, t.2.chars().next().unwrap() as u32)).collect();
    assert_eq!(
        tree,
        [
            ("nf-md-folder_table", 0xf12e3),
            ("nf-md-table_multiple", 0xf13c8),
            ("nf-md-table", 0xf04eb),
            ("nf-md-eye_outline", 0xf06d0),
            ("nf-md-table_eye", 0xf1094),
            ("nf-md-table_refresh", 0xf13a0),
        ]
    );
    let types: Vec<(&str, u32)> = TYPES.iter().map(|t| (t.1, t.2.chars().next().unwrap() as u32)).collect();
    assert_eq!(
        types,
        [
            ("nf-md-format_text", 0xf0284),
            ("nf-md-numeric", 0xf03a0),
            ("nf-md-calendar_clock", 0xf00f0),
            ("nf-md-toggle_switch_outline", 0xf0a1a),
            ("nf-md-code_json", 0xf0626),
            ("nf-md-identifier", 0xf0efe),
            ("nf-md-hexadecimal", 0xf12a7),
            ("nf-md-code_brackets", 0xf016a),
            ("nf-md-shape_outline", 0xf0832),
        ]
    );
    let groups: Vec<(&str, u32)> = STRUCTURE.iter().map(|t| (t.1, t.2.chars().next().unwrap() as u32)).collect();
    assert_eq!(
        groups,
        [
            ("nf-md-table_column", 0xf0835),
            ("nf-md-key", 0xf0306),
            ("nf-md-key_link", 0xf119f),
            ("nf-md-format_list_numbered", 0xf027b),
            ("nf-md-fingerprint", 0xf0237),
            ("nf-md-checkbox_marked_outline", 0xf0135),
            ("nf-md-lightning_bolt", 0xf140b),
        ]
    );
    // Every node, category and structure group has its own glyph, one column wide, in the
    // Supplementary Private Use Area-A (where Nerd Fonts v3 puts the Material Design icons).
    let mut all: Vec<&str> =
        TREE.iter().map(|t| t.2).chain(TYPES.iter().map(|t| t.2)).chain(STRUCTURE.iter().map(|t| t.2)).collect();
    for g in &all {
        let mut chars = g.chars();
        let c = chars.next().unwrap();
        assert!(chars.next().is_none(), "{g:?}: one char");
        assert!(('\u{f0000}'..='\u{ffffd}').contains(&c), "{g:?}: in the Supplementary Private Use Area-A");
        assert_eq!(width(g), 1, "{g:?}: one column");
    }
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), TREE.len() + TYPES.len() + STRUCTURE.len(), "no glyph twice");
    assert_eq!(structure(StructureGroup::Triggers), "\u{f140b}");
    assert_eq!(TreeIcon::MaterializedView.glyph(), "\u{f13a0}");
    assert_ne!(TreeIcon::View.glyph(), TreeIcon::MaterializedView.glyph());
}

#[test]
fn type_categories_follow_the_type_name() {
    use TypeCategory::*;
    for (name, want) in [
        ("text", Text),
        ("character varying(20)", Text),
        ("character(3)", Text),
        ("\"char\"", Text),
        ("name", Text),
        ("varchar(255)", Text),
        ("integer", Number),
        ("bigint", Number),
        ("smallint", Number),
        ("numeric(10,2)", Number),
        ("double precision", Number),
        ("real", Number),
        ("money", Number),
        ("int(11)", Number),
        ("timestamp with time zone", DateTime),
        ("timestamp(3) without time zone", DateTime),
        ("time without time zone", DateTime),
        ("date", DateTime),
        ("interval", DateTime),
        ("datetime", DateTime),
        ("boolean", Boolean),
        ("json", Json),
        ("JSONB", Json),
        ("uuid", Uuid),
        ("bytea", Binary),
        ("bit varying(8)", Binary),
        ("integer[]", Array),
        ("text[]", Array),
        ("character varying(20)[]", Array),
        ("order_status", Other),
        ("point", Other),
        ("tsvector", Other),
        ("", Other),
        ("timeline", Other),
    ] {
        assert_eq!(TypeCategory::of(name), want, "{name:?}");
    }
}
