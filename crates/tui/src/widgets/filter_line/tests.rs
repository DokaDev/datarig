use super::*;
use datarig_core::query::OrderKey;

#[test]
fn fields_follow_what_is_applied_until_edited() {
    let mut s = LineState::default();
    s.follow(("id > 1", ""));
    assert_eq!(s.where_.text(), "id > 1");
    // Edited: what is applied later does not replace the text, and the field says so.
    s.where_.set("id > 2");
    s.follow(("id > 3", "name ASC"));
    assert_eq!((s.where_.text(), s.order.text()), ("id > 2", "name ASC"));
    assert!(s.edited(Field::Where, "id > 3") && !s.edited(Field::Order, "name ASC"));
    // Applied as typed: not edited any more, and it follows again.
    s.follow(("id > 2", "name ASC"));
    s.follow(("", ""));
    assert_eq!(s.where_.text(), "");
    // A field with the focus keeps its text; a blank text is no text.
    s.focus = Some(Field::Order);
    s.order.set("  ");
    s.follow(("", "id DESC"));
    assert_eq!(s.order.text(), "  ");
    assert!(!s.edited(Field::Order, ""));
    s.restore(("a = 1", "id DESC"));
    assert_eq!((s.where_.text(), s.order.text()), ("a = 1", "id DESC"));
}

#[test]
fn only_an_empty_line_without_the_focus_leaves_a_short_pane() {
    assert!(hidden(true, false, SHORT - 1));
    assert!(!hidden(true, false, SHORT));
    assert!(!hidden(true, true, 5), "focused, it shows");
    assert!(!hidden(false, false, 5), "applied, it shows");
}

#[test]
fn the_order_takes_a_second_row_while_edited_on_a_narrow_line() {
    assert_eq!(rows(28, Some(Field::Order)), 2);
    assert_eq!(rows(28, Some(Field::Where)), 1);
    assert_eq!(rows(54, Some(Field::Order)), 1);
    assert_eq!(rows(28, None), 1);
}

#[test]
fn the_order_shrinks_to_its_first_key_and_the_count_of_the_others() {
    let keys = vec![OrderKey::user("id", Dir::Desc), OrderKey::user("name", Dir::Asc)];
    assert_eq!(collapsed(&keys), "\u{2193}id+1");
    assert_eq!(collapsed(&keys[1..]), "\u{2191}name");
    assert_eq!(collapsed(&[]), "");
}
