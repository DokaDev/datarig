use super::*;

/// `COLUMN_TYPE` of an `ENUM` as MySQL 8.4 writes it: a quote doubled, a backslash doubled.
#[test]
fn enum_values_from_the_column_type() {
    let v = |xs: &[&str]| Some(xs.iter().map(|x| x.to_string()).collect::<Vec<_>>());
    assert_eq!(enum_values("enum('pending','paid')"), v(&["pending", "paid"]));
    assert_eq!(enum_values("enum('a','it''s','b\\\\c','d\"q','x,y','')"), v(&["a", "it's", "b\\c", "d\"q", "x,y", ""]));
    assert_eq!(enum_values("ENUM('a')"), v(&["a"]));
    for other in ["set('p','q')", "varchar(10)", "enum()", "enum('a',)", "enum('a' 'b')", "enum('a'", "enu", ""] {
        assert_eq!(enum_values(other), None, "{other}");
    }
}
