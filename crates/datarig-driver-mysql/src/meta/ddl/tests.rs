use super::*;
use datarig_core::sql::split::split_in;

const MY: Dialect = Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });
const ANSI: Dialect =
    Dialect::MySql(MySqlMode { ansi_quotes: true, no_backslash_escapes: false, dollar_quotes: false });

fn parts(name: &str, d: Dialect) -> Option<Vec<String>> {
    name_parts(name, d)
}

fn v(p: &[&str]) -> Option<Vec<String>> {
    Some(p.iter().map(|s| s.to_string()).collect())
}

/// A name as SQL writes it: bare or quoted parts, as the server reads them (MySQL keeps a bare
/// name's case), a backtick doubled inside a quoted one, `"` a quote only under `ANSI_QUOTES`.
#[test]
fn a_typed_name_is_read_as_sql_reads_it() {
    assert_eq!(parts("users", MY), v(&["users"]));
    assert_eq!(parts(" Shop.Users ", MY), v(&["Shop", "Users"]));
    assert_eq!(parts("shop . users", MY), v(&["shop", "users"]));
    assert_eq!(parts("`my db`.`a.b`", MY), v(&["my db", "a.b"]));
    assert_eq!(parts("`a``b`", MY), v(&["a`b"]));
    assert_eq!(parts("shop.`order items`", MY), v(&["shop", "order items"]));
    assert_eq!(parts("\"x y\".t", ANSI), v(&["x y", "t"]));
    assert_eq!(parts("a.b.c", MY), v(&["a", "b", "c"]), "read, then refused as a name of three parts");
    for bad in ["", ".", "a.", ".a", "a b", "`a", "`a`b", "\"x\"", "a..b"] {
        assert_eq!(parts(bad, MY), None, "{bad:?}");
    }
}

#[test]
fn the_kind_of_a_found_object() {
    assert_eq!(kind_of("BASE TABLE"), ObjectKind::Table);
    assert_eq!(kind_of("VIEW"), ObjectKind::View);
    assert_eq!(kind_of("SYSTEM VIEW"), ObjectKind::View);
    assert_eq!(kind_of("PROCEDURE"), ObjectKind::Procedure);
    assert_eq!(kind_of("FUNCTION"), ObjectKind::Function);
    assert_eq!(kind_of("TRIGGER"), ObjectKind::Trigger);
    assert_eq!(kind_of("EVENT"), ObjectKind::Event);
    // MariaDB's sequence is a table to SHOW CREATE TABLE.
    assert_eq!(kind_of("SEQUENCE"), ObjectKind::Table);
}

/// A table's or a view's text is one statement ended with `;`; a routine's body (statements of
/// its own, a `;` in a string and a comment) goes between `DELIMITER` lines, and the editor's
/// splitter reads it as the one statement the server wrote, the `DELIMITER` lines as none.
#[test]
fn the_text_runs_again_as_one_statement() {
    let table = "CREATE TABLE `t` (\n  `id` int NOT NULL,\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB";
    let text = runnable(ObjectKind::Table, table);
    assert_eq!(text, format!("{table};\n"));
    let s = split_in(&text, MY);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].body(&text), table);
    let body =
        "CREATE DEFINER=`u`@`%` PROCEDURE `p`(IN a INT)\nBEGIN\n  SELECT 'x;y'; -- one; two\n  SELECT a + 1;\nEND";
    for kind in [ObjectKind::Procedure, ObjectKind::Function, ObjectKind::Trigger, ObjectKind::Event] {
        let text = runnable(kind, body);
        assert_eq!(text, format!("DELIMITER ;;\n{body} ;;\nDELIMITER ;\n"));
        let s = split_in(&text, MY);
        assert_eq!(s.len(), 1, "{kind:?}: {s:?}");
        assert_eq!(s[0].body(&text), body);
        // A statement after it ends at `;` again.
        let more = format!("{text}SELECT 1;\nSELECT 2;\n");
        let s = split_in(&more, MY);
        assert_eq!(s.iter().map(|x| x.body(&more)).collect::<Vec<_>>(), [body, "SELECT 1", "SELECT 2"]);
    }
}
