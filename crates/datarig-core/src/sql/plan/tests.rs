use super::json::{Value, parse as json};
use super::*;

#[test]
fn json_reads_every_kind_of_value() {
    let d = json(r#" {"a": [1, -2.50, 3e2], "b": "x\"\\\/\n\u00e9\ud83d\ude00", "c": true, "d": false, "e": null, "f": {}, "g": []} "#)
        .unwrap();
    let a = d.field(d.root, "a").unwrap();
    let nums: Vec<&Value> = d.items(a).iter().map(|&i| d.get(i)).collect();
    assert_eq!(
        nums,
        [&Value::Number("1".into(), 1.0), &Value::Number("-2.50".into(), -2.5), &Value::Number("3e2".into(), 300.0)]
    );
    assert_eq!(d.str(d.field(d.root, "b").unwrap()), Some("x\"\\/\n\u{e9}\u{1F600}"));
    assert_eq!(d.get(d.field(d.root, "c").unwrap()), &Value::Bool(true));
    assert_eq!(d.get(d.field(d.root, "e").unwrap()), &Value::Null);
    assert_eq!(d.get(d.field(d.root, "f").unwrap()), &Value::Object(Vec::new()));
    assert_eq!(d.get(d.field(d.root, "g").unwrap()), &Value::Array(Vec::new()));
    assert_eq!(d.scalar_text(a).as_deref(), Some("1, -2.50, 3e2"));
    assert_eq!(json("3").unwrap().num(0), Some(3.0));
    assert_eq!(json("[[], [[]]]").unwrap().items(0).len(), 2);
}

#[test]
fn json_refuses_what_is_not_json() {
    for bad in ["", "{", "[1,]", "{\"a\":1,}", "{\"a\" 1}", "[1] x", "\"\\q\"", "\"a\nb\"", "nul", "{1: 2}", "[}"] {
        assert!(json(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn explain_wraps_the_statement_and_knows_one() {
    assert_eq!(explain_sql("SELECT 1", false), "EXPLAIN (FORMAT JSON) SELECT 1");
    assert_eq!(explain_sql("DELETE FROM t", true), "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) DELETE FROM t");
    assert!(is_explain("  -- why\n explain select 1"));
    assert!(is_explain("/* c */ EXPLAIN ANALYZE DELETE FROM t"));
    assert!(!is_explain("SELECT 'EXPLAIN'"));
    assert!(!is_explain(""));
}

#[test]
fn buffers_add_up_and_say_their_hit_ratio() {
    let b = Buffers { shared_hit: 90, shared_read: 5, local_hit: 0, local_read: 5, ..Buffers::default() };
    assert_eq!(b.hit_ratio(), Some(0.9));
    assert_eq!(Buffers::default().hit_ratio(), None);
    assert!(Buffers::default().is_empty() && !b.is_empty());
}
