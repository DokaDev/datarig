use super::*;

/// Rows of the result as read: row `r` is `[r, "n<r>", NULL]`.
fn orig(r: usize) -> Vec<Cell> {
    vec![Some(r.to_string()), Some(format!("n{r}")), None]
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

#[test]
fn a_cell_set_is_pending_and_one_set_back_to_its_value_is_not() {
    let mut b = EditBatch::new();
    assert!(b.set(&[(3, 1)], &text("x"), orig));
    assert_eq!(b.value(3, 1), Some(&text("x")));
    assert_eq!(b.row(3).unwrap().orig, orig(3), "the row as read is kept");
    assert_eq!(b.counts(), Counts { updates: 1, inserts: 0, deletes: 0 });
    assert!(b.set(&[(3, 1)], &text("n3"), orig), "back to the value as read");
    assert!(b.is_empty());
    // NULL is the value as read of a NULL cell; an empty text is not.
    assert!(!b.set(&[(3, 2)], &Value::Null, orig), "nothing changed");
    assert!(b.set(&[(3, 2)], &text(""), orig));
    assert_eq!(b.value(3, 2), Some(&text("")));
    assert!(b.set(&[(3, 2)], &Value::Default, orig), "DEFAULT is never the value as read");
}

#[test]
fn a_row_keeps_the_values_it_was_first_changed_from() {
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    b.set(&[(1, 0)], &text("9"), |_| vec![None, None, None]);
    assert_eq!(b.row(1).unwrap().orig, orig(1));
    assert_eq!(b.row(1).unwrap().set.len(), 2, "several edits of a row are one change");
}

#[test]
fn every_action_undoes_and_redoes_as_one_step() {
    let mut b = EditBatch::new();
    b.set(&[(1, 1), (2, 1), (3, 1)], &text("x"), orig);
    b.set(&[(1, 0)], &text("9"), orig);
    assert!(b.undo());
    assert_eq!((b.value(1, 0), b.value(2, 1)), (None, Some(&text("x"))));
    assert!(b.undo());
    assert!(b.is_empty(), "the three cells were one action");
    assert!(!b.undo());
    assert!(b.redo() && b.redo());
    assert_eq!((b.value(1, 0), b.value(3, 1)), (Some(&text("9")), Some(&text("x"))));
    assert!(!b.redo());
    // A new action after an undo drops what could be redone; one that changes nothing is no step.
    b.undo();
    b.set(&[(5, 1)], &text("y"), orig);
    assert!(!b.can_redo());
    assert!(!b.set(&[(5, 1)], &text("y"), orig));
    b.undo();
    assert_eq!(b.value(5, 1), None);
    assert_eq!(b.value(3, 1), Some(&text("x")));
}

#[test]
fn deleting_marks_rows_drops_what_they_set_and_toggles_back() {
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    assert!(b.delete(&[1, 2], orig));
    assert!(b.deleted(1) && b.deleted(2));
    assert_eq!(b.value(1, 1), None, "a deleted row sets nothing");
    assert_eq!(b.counts(), Counts { updates: 0, inserts: 0, deletes: 2 });
    assert!(!b.set(&[(1, 1)], &text("y"), orig), "nor can it");
    assert!(b.delete(&[1, 2], orig), "every one was marked: unmarked");
    assert!(b.is_empty());
    b.undo();
    b.undo();
    assert_eq!(b.value(1, 1), Some(&text("x")), "undo brings the change back");
    // Some marked, some not: all are marked.
    b.delete(&[1], orig);
    b.delete(&[1, 2], orig);
    assert!(b.deleted(1) && b.deleted(2));
}

#[test]
fn rows_marked_and_new_rows_removed_together_are_one_step() {
    let mut b = EditBatch::new();
    let a = b.add(usize::MAX, None, vec![Value::Default; 3]);
    let c = b.add(usize::MAX, None, vec![Value::Default; 3]);
    assert!(b.delete_with_new(&[1, 2], &[a], orig));
    assert!(b.deleted(1) && b.deleted(2));
    assert_eq!(b.drafts().iter().map(|d| d.id).collect::<Vec<_>>(), [c]);
    assert!(b.undo());
    assert!(!b.deleted(1) && !b.deleted(2) && b.drafts().len() == 2, "one step");
    assert!(b.redo());
    assert_eq!(b.counts(), Counts { updates: 0, inserts: 1, deletes: 2 });
    // Every row marked: unmarked, the new row removed all the same.
    assert!(b.delete_with_new(&[1, 2], &[c], orig));
    assert!(b.is_empty());
    assert!(b.undo());
    assert_eq!(b.counts(), Counts { updates: 0, inserts: 1, deletes: 2 });
    // New rows only.
    assert!(b.delete_with_new(&[], &[c], orig));
    assert!(b.drafts().is_empty() && b.deleted(1));
    assert!(!b.delete_with_new(&[], &[c], orig), "nothing changed: no step");
    b.undo();
    assert_eq!(b.drafts().len(), 1);
}

#[test]
fn new_rows_are_placed_after_their_row_and_undo_keeps_their_ids() {
    let mut b = EditBatch::new();
    let a = b.add(5, None, vec![Value::Default; 3]);
    let c = b.add(2, None, vec![Value::Default; 3]);
    let d = b.add(5, None, vec![Value::Default; 3]);
    let e = b.add(5, Some(a), vec![Value::Default; 3]);
    let order: Vec<u64> = b.drafts().iter().map(|x| x.id).collect();
    assert_eq!(order, [c, a, e, d]);
    // Next to a new row, it is shown after that row's row, whatever row it was asked after.
    let f = b.add(9, Some(c), vec![Value::Default; 3]);
    assert_eq!((b.drafts()[1].id, b.drafts()[1].after), (f, 2));
    b.undo();
    assert!(b.set_new(&[(e, 1)], &text("x")));
    assert_eq!(b.drafts()[2].cells[1], text("x"));
    assert!(b.remove_new(&[a]));
    assert_eq!(b.counts().inserts, 3);
    b.undo();
    b.undo();
    let ids: Vec<u64> = b.drafts().iter().map(|x| x.id).collect();
    assert_eq!(ids, [c, a, e, d]);
    assert_eq!(b.drafts()[2].cells[1], Value::Default);
    b.redo();
    assert_eq!(b.drafts()[2].cells[1], text("x"));
}

#[test]
fn reverting_drops_the_changes_of_cells_and_new_rows() {
    let mut b = EditBatch::new();
    b.set(&[(1, 0), (1, 1)], &text("7"), orig);
    b.delete(&[2], orig);
    let n = b.add(1, None, vec![Value::Default; 3]);
    assert!(b.revert(&[(1, 0), (2, 0)], &[n]));
    assert_eq!((b.value(1, 0), b.value(1, 1)), (None, Some(&text("7"))));
    assert!(!b.deleted(2), "a row's deletion goes with any of its cells");
    assert!(b.drafts().is_empty());
    b.undo();
    assert!(b.deleted(2) && b.value(1, 0).is_some() && b.drafts().len() == 1, "one step");
}

#[test]
fn discarding_is_one_step_and_clearing_forgets_the_history() {
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    let n = b.add(1, None, vec![Value::Default; 3]);
    assert!(b.discard());
    assert!(b.is_empty());
    b.undo();
    assert_eq!(b.counts(), Counts { updates: 1, inserts: 1, deletes: 0 });
    b.clear();
    assert!(b.is_empty() && !b.can_undo() && !b.can_redo());
    assert_ne!(b.add(1, None, vec![]), n, "new ids stay new");
}
