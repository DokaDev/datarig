use super::*;

#[test]
fn the_reserved_words_are_sorted_and_lower_case() {
    assert!(RESERVED.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicates");
    assert!(RESERVED.iter().all(|w| *w == w.to_ascii_lowercase()));
    // One of each server's own: 8.0 only (`master_bind`), 9.x only (`library`), all three.
    for w in ["master_bind", "library", "select", "rank", "lead", "qualify", "tablesample"] {
        assert!(RESERVED.contains(&w), "{w}");
    }
}

#[test]
fn bare_names_are_plain_and_not_reserved() {
    for name in ["users", "Users", "user_id", "a$b", "_x", "status", "date", "text", "name", "first"] {
        assert!(!needs_quotes(name), "{name}");
    }
    for name in ["", "select", "SELECT", "Order", "rank", "1col", "1e5", "0x1F", "$x", "a b", "caf\u{e9}", "a-b", "a`b"]
    {
        assert!(needs_quotes(name), "{name:?}");
    }
}
