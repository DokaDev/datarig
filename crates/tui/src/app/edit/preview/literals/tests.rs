use super::*;
use datarig_core::sql::dialect::MySqlMode;

const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });

fn text(c: &str, v: &str) -> (String, Decoded) {
    (c.to_string(), Decoded::Text(v.to_string()))
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
    assert_eq!(text_literals(&sql, MY), [text("note", "it's \\ x"), text("applicant_name", name)]);
}

#[test]
fn an_inserts_values_are_named_by_their_place() {
    let sql = format!("INSERT INTO `shop`.`zz` (`id`, `a``b`, `c`) VALUES (1, 'x', {})", lit("p\\q"));
    assert_eq!(text_literals(&sql, MY), [text("c", "p\\q")]);
}

#[test]
fn bytes_that_are_not_utf8_say_how_many_and_bare_hex_is_not_text() {
    let sql = "SELECT 1 FROM `t` WHERE `a` = _utf8mb4 X'FF00FE' AND `raw` <=> X'0AFF' FOR UPDATE";
    assert_eq!(text_literals(sql, MY), [("a".to_string(), Decoded::Bytes(3))]);
    // A plain string, a bare hex literal and an odd one: nothing to read back.
    let sql = "UPDATE `t` SET `a` = 'x', `b` = X'41', `c` = _utf8mb4 X'414' WHERE `id` <=> 1";
    assert_eq!(text_literals(sql, MY), []);
}

#[test]
fn a_literal_compared_with_something_else_is_left_out() {
    let sql = "SELECT 1 FROM `t` WHERE `a` > _utf8mb4 X'41' AND f(`a`) = _utf8mb4 X'41'";
    assert_eq!(text_literals(sql, MY), []);
}
