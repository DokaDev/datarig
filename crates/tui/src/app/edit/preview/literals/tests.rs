use super::*;
use datarig_core::sql::dialect::MySqlMode;

const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });

fn text(part: Part, c: &str, v: &str) -> (Part, String, Decoded) {
    (part, c.to_string(), Decoded::Text(v.to_string()))
}

/// `s` as MySQL's literal of its UTF-8 bytes.
fn lit(s: &str) -> String {
    let hex: String = s.bytes().map(|b| format!("{b:02X}")).collect();
    format!("_utf8mb4 X'{hex}'")
}

#[test]
fn an_exact_text_compare_is_read_back_once_per_column() {
    // A Korean name and a note (Hangul as escapes).
    let name = "\u{AE40}\u{B3D9}\u{D718}";
    let (a, b) = (lit(name), lit("it's \\ x"));
    let sql = format!(
        "UPDATE `shop`.`zz` SET `note` = {b} WHERE `id` <=> 1 AND `applicant_name` = {a} AND \
         CONVERT(`applicant_name` USING utf8mb4) COLLATE utf8mb4_0900_bin <=> {a}"
    );
    assert_eq!(
        text_literals(&sql, MY),
        [text(Part::Set, "note", "it's \\ x"), text(Part::Where, "applicant_name", name)]
    );
}

#[test]
fn an_inserts_values_are_named_by_their_place() {
    let sql = format!("INSERT INTO `shop`.`zz` (`id`, `a``b`, `c`) VALUES (1, 'x', {})", lit("p\\q"));
    assert_eq!(text_literals(&sql, MY), [text(Part::Values, "c", "p\\q")]);
}

#[test]
fn bytes_that_are_not_utf8_say_how_many_and_bare_hex_is_not_text() {
    let sql = "SELECT 1 FROM `t` WHERE `a` = _utf8mb4 X'FF00FE' AND `raw` <=> X'0AFF' FOR UPDATE";
    assert_eq!(text_literals(sql, MY), [(Part::Where, "a".to_string(), Decoded::Bytes(3))]);
    // A plain string, a bare hex literal and an odd one: nothing to read back.
    let sql = "UPDATE `t` SET `a` = 'x', `b` = X'41', `c` = _utf8mb4 X'414' WHERE `id` <=> 1";
    assert_eq!(text_literals(sql, MY), []);
}

#[test]
fn a_literal_compared_with_something_else_is_left_out() {
    let sql = "SELECT 1 FROM `t` WHERE `a` > _utf8mb4 X'41' AND f(`a`) = _utf8mb4 X'41'";
    assert_eq!(text_literals(sql, MY), []);
}

#[test]
fn a_pair_is_one_line_of_bounded_width() {
    // A name with a line break and an escape, a value of a megabyte.
    let column = "a\nb\u{1b}[31m";
    let long = "x".repeat(1 << 20);
    let said = text_pair(column, &long, 40);
    assert!(!said.chars().any(char::is_control), "{said:?}");
    assert!(said.starts_with("a\u{21B5}b\u{FFFD}[31m = 'x"), "{said:?}");
    assert!(said.ends_with("x\u{2026}'"), "{said:?}");
    assert!(crate::text::width(&said) < 60);
    assert_eq!(text_pair("c", "line\none", 40), "c = 'line\u{21B5}one'");
    // Cut where only the start was looked at, however its characters measure.
    let marks = "\u{301}".repeat(1000);
    assert!(text_pair("c", &marks, 4).ends_with("\u{2026}'"));
}

#[test]
fn one_column_set_and_found_by_is_told_apart() {
    let (new, old) = (lit("new \\ one"), lit("\u{AC00}"));
    let sql = format!(
        "UPDATE `t` SET `note` = {new} WHERE `id` <=> 1 AND `note` = {old} AND CONVERT(`note` USING utf8mb4) \
         COLLATE utf8mb4_0900_bin <=> {old}"
    );
    assert_eq!(text_literals(&sql, MY), [text(Part::Set, "note", "new \\ one"), text(Part::Where, "note", "\u{AC00}")]);
    // A row check and a delete only find rows.
    for verb in ["SELECT 1 FROM `t`", "DELETE FROM `t`"] {
        let sql = format!("{verb} WHERE `note` = {old}");
        assert_eq!(text_literals(&sql, MY), [text(Part::Where, "note", "\u{AC00}")], "{verb}");
    }
}
