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

/// The kinds the lookup finds, each by the server's word; a kind the driver does not know (MariaDB's
/// package) is an error, never read as a table.
#[test]
fn the_kind_of_a_found_object() {
    for (word, kind) in [
        ("BASE TABLE", ObjectKind::Table),
        ("SYSTEM VERSIONED", ObjectKind::Table),
        ("SEQUENCE", ObjectKind::Table),
        ("VIEW", ObjectKind::View),
        ("SYSTEM VIEW", ObjectKind::View),
        ("PROCEDURE", ObjectKind::Procedure),
        ("FUNCTION", ObjectKind::Function),
        ("TRIGGER", ObjectKind::Trigger),
        ("EVENT", ObjectKind::Event),
    ] {
        assert_eq!(kind_of(word), Ok(kind), "{word}");
    }
    for word in ["PACKAGE", "PACKAGE BODY", "", "table"] {
        assert_eq!(kind_of(word), Err(DbError::KindUnsupported(word.into())), "{word:?}");
    }
}

/// `:ddl` takes a kind word before the name (any case); a word that is no kind is part of the
/// name (and a name with a blank is no name). A name of three parts, or none, is no name; a part
/// with a character above U+FFFF names nothing, and is said so before anything is asked.
#[test]
fn a_typed_name_may_start_with_its_kind() {
    let ok = |k: Option<ObjectKind>, db: Option<&str>, n: &str| Ok((k, db.map(str::to_string), n.to_string()));
    assert_eq!(typed_name("users", MY), ok(None, None, "users"));
    assert_eq!(typed_name(" shop.users ", MY), ok(None, Some("shop"), "users"));
    assert_eq!(typed_name("procedure shop.p", MY), ok(Some(ObjectKind::Procedure), Some("shop"), "p"));
    assert_eq!(typed_name("PROCEDURE  p", MY), ok(Some(ObjectKind::Procedure), None, "p"));
    assert_eq!(typed_name("Table `order items`", MY), ok(Some(ObjectKind::Table), None, "order items"));
    for (word, kind) in [
        ("view", ObjectKind::View),
        ("function", ObjectKind::Function),
        ("trigger", ObjectKind::Trigger),
        ("event", ObjectKind::Event),
    ] {
        assert_eq!(typed_name(&format!("{word} x"), MY), ok(Some(kind), None, "x"));
    }
    // A kind word alone is a name.
    assert_eq!(typed_name("procedure", MY), ok(None, None, "procedure"));
    for bad in ["a.b.c", "", "procedure a b", "index x", "`a"] {
        assert_eq!(typed_name(bad, MY), Err(DbError::NotAName), "{bad:?}");
    }
    assert_eq!(typed_name("zz_\u{1F600}", MY), Err(DbError::NotFound));
    assert_eq!(typed_name("`a\u{1F600}`.b", MY), Err(DbError::NotFound));
    assert_eq!(typed_name("caf\u{E9}", MY), ok(None, None, "caf\u{E9}"), "within the BMP");
}

/// A routine's own `sql_mode` comes first, in a comment, when it is said; the flags are the same
/// in any order.
#[test]
fn an_objects_own_sql_mode_is_said_first() {
    let body = "CREATE PROCEDURE `p`()\nSELECT 1";
    let text = runnable(ObjectKind::Procedure, body, Some("NO_BACKSLASH_ESCAPES"));
    assert_eq!(text, format!("-- sql_mode: NO_BACKSLASH_ESCAPES\nDELIMITER ;;\n{body}\n;;\nDELIMITER ;\n"));
    assert_eq!(split_in(&text, MY).iter().map(|x| x.body(&text)).collect::<Vec<_>>(), [body]);
    assert!(runnable(ObjectKind::Event, body, Some("")).starts_with("-- sql_mode: ''\nDELIMITER ;;\n"));
    assert!(same_mode("STRICT_TRANS_TABLES,ONLY_FULL_GROUP_BY", "ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES"));
    assert!(same_mode("", ""));
    assert!(!same_mode("STRICT_TRANS_TABLES", "STRICT_TRANS_TABLES,NO_BACKSLASH_ESCAPES"));
}

/// A table's or a view's text is one statement ended with `;`; a routine's body (statements of
/// its own, a `;` in a string and a comment) goes between `DELIMITER` lines, and the editor's
/// splitter reads it as the one statement the server wrote, the `DELIMITER` lines as none.
#[test]
fn the_text_runs_again_as_one_statement() {
    let table = "CREATE TABLE `t` (\n  `id` int NOT NULL,\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB";
    let text = runnable(ObjectKind::Table, table, None);
    assert_eq!(text, format!("{table};\n"));
    let s = split_in(&text, MY);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].body(&text), table);
    let body =
        "CREATE DEFINER=`u`@`%` PROCEDURE `p`(IN a INT)\nBEGIN\n  SELECT 'x;y'; -- one; two\n  SELECT a + 1;\nEND";
    for kind in [ObjectKind::Procedure, ObjectKind::Function, ObjectKind::Trigger, ObjectKind::Event] {
        let text = runnable(kind, body, None);
        assert_eq!(text, format!("DELIMITER ;;\n{body}\n;;\nDELIMITER ;\n"));
        let s = split_in(&text, MY);
        assert_eq!(s.len(), 1, "{kind:?}: {s:?}");
        assert_eq!(s[0].body(&text), body);
        // A statement after it ends at `;` again.
        let more = format!("{text}SELECT 1;\nSELECT 2;\n");
        let s = split_in(&more, MY);
        assert_eq!(s.iter().map(|x| x.body(&more)).collect::<Vec<_>>(), [body, "SELECT 1", "SELECT 2"]);
    }
}

/// A body the server keeps with a comment at its end (`-- c`, `# c`) still ends: the end is on a
/// line of its own, so the splitter reads one statement, the server's text up to its end (the
/// statement it runs leaves the comment out, as every statement's last comment), and nothing
/// after it is swallowed.
#[test]
fn a_body_ending_in_a_comment_still_ends() {
    for (body, comment) in [
        ("CREATE DEFINER=`u`@`%` PROCEDURE `p`()\nBEGIN\n  SELECT 1;\nEND", " -- c"),
        ("CREATE DEFINER=`u`@`%` PROCEDURE `p`()\nBEGIN\n  SELECT 1;\nEND", " # c"),
        ("CREATE DEFINER=`u`@`%` TRIGGER `t` BEFORE INSERT ON `x` FOR EACH ROW SET NEW.a = 1", " -- c"),
    ] {
        let server = format!("{body}{comment}");
        let text = runnable(ObjectKind::Procedure, &server, None);
        let more = format!("{text}SELECT 2;\n");
        let s = split_in(&more, MY);
        assert_eq!(s.iter().map(|x| x.body(&more)).collect::<Vec<_>>(), [body, "SELECT 2"], "{text}");
        let whole = more[s[0].start..s[0].end].strip_suffix(";;").map(str::trim_end);
        assert_eq!(whole, Some(server.as_str()), "{text}");
    }
}

/// The server prints a table's or a view's strings with backslash escapes whatever the session's
/// mode, so text with a backslash says so first; text without one says nothing.
#[test]
fn a_tables_text_with_a_backslash_says_how_it_reads() {
    let table = "CREATE TABLE `t` (\n  `v` varchar(20) DEFAULT 'a\\\\b''c'\n) ENGINE=InnoDB";
    for kind in [ObjectKind::Table, ObjectKind::View] {
        let text = runnable(kind, table, None);
        assert_eq!(text, format!("{BACKSLASH_NOTE}{table};\n"));
        assert_eq!(split_in(&text, MY).iter().map(|x| x.body(&text)).collect::<Vec<_>>(), [table]);
    }
    let plain = "CREATE TABLE `t` (\n  `v` varchar(20) DEFAULT 'it''s'\n) ENGINE=InnoDB";
    assert_eq!(runnable(ObjectKind::Table, plain, None), format!("{plain};\n"));
}
