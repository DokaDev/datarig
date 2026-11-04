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
fn reverting_drops_the_changes_of_cells_and_sets_new_rows_cells_to_their_default() {
    let mut b = EditBatch::new();
    b.set(&[(1, 0), (1, 1)], &text("7"), orig);
    b.delete(&[2], orig);
    let n = b.add(1, None, vec![text("a"), text("b"), Value::Null]);
    assert!(b.revert(&[(1, 0), (2, 0)], &[(n, 0), (n, 2)]));
    assert_eq!((b.value(1, 0), b.value(1, 1)), (None, Some(&text("7"))));
    assert!(!b.deleted(2), "a row's deletion goes with any of its cells");
    assert_eq!(b.drafts()[0].cells, [Value::Default, text("b"), Value::Default], "the new row stays");
    b.undo();
    assert!(b.deleted(2) && b.value(1, 0).is_some(), "one step");
    assert_eq!(b.drafts()[0].cells[0], text("a"));
    b.redo();
    assert!(!b.revert(&[], &[(n, 0)]), "a default is nothing to revert");
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

#[test]
fn every_change_moves_the_revision_and_reads_and_no_ops_do_not() {
    let mut b = EditBatch::new();
    let mut last = b.revision();
    let mut moved = |b: &EditBatch, what: &str| {
        assert!(b.revision() > last, "{what} moves the revision");
        last = b.revision();
    };
    b.set(&[(1, 1)], &text("x"), orig);
    moved(&b, "set");
    b.delete(&[2], orig);
    moved(&b, "delete");
    let n = b.add(1, None, vec![Value::Default; 3]);
    moved(&b, "add");
    b.set_new(&[(n, 0)], &text("a"));
    moved(&b, "set_new");
    b.revert(&[(1, 1)], &[]);
    moved(&b, "revert");
    b.delete_with_new(&[3], &[n], orig);
    moved(&b, "delete_with_new");
    b.undo();
    moved(&b, "undo");
    b.redo();
    moved(&b, "redo");
    b.undo();
    moved(&b, "undo");
    b.remove_new(&[n]);
    moved(&b, "remove_new");
    b.discard();
    moved(&b, "discard");
    b.clear();
    moved(&b, "clear");
    let at = b.revision();
    let _ = (b.is_empty(), b.counts(), b.rows().count(), b.drafts().len(), b.can_undo(), b.can_redo());
    let _ = (b.row(1), b.value(1, 1), b.deleted(2));
    assert!(!b.undo() && !b.redo());
    assert!(!b.set(&[(1, 1)], &text("n1"), orig), "a cell set to its value as read changes nothing");
    assert!(!b.remove_new(&[99]) && !b.discard());
    assert_eq!(b.revision(), at, "reads and actions that change nothing keep it");
}

/// What was sent to be applied is compared by its changes and their history: an undo and a redo
/// since move the revision but leave the batch as it was.
#[test]
fn batches_are_equal_by_their_changes_and_history_not_their_revision() {
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    let sent = b.clone();
    b.undo();
    b.redo();
    assert_ne!(b.revision(), sent.revision());
    assert_eq!(b, sent);
    b.undo();
    assert_ne!(b, sent, "the change undone");
}

fn paste_ops(set: &[((usize, usize), Value)], set_new: &[((u64, usize), Value)], add: &[Vec<Value>]) -> PasteOps {
    PasteOps { width: 3, set: set.to_vec(), set_new: set_new.to_vec(), add: add.to_vec() }
}

#[test]
fn a_paste_is_one_step_that_undo_takes_back_whole_and_redo_brings_back() {
    let mut b = EditBatch::new();
    let n = b.add(usize::MAX, None, vec![Value::Default; 3]);
    b.set(&[(2, 1)], &text("before"), orig);
    let before = b.clone();
    let at = b.revision();
    let ops = paste_ops(
        &[((1, 1), text("a")), ((2, 1), text("b")), ((2, 2), Value::Null)],
        &[((n, 0), text("9"))],
        &[vec![text("7"), text("x"), Value::Default], vec![Value::Null, text("y"), Value::Default]],
    );
    let r = b.paste(ops, orig).unwrap();
    assert_eq!(b.revision(), at + 1, "the revision moves once");
    assert_eq!(r.changed, 3, "(2, 2) was NULL as read: no change");
    assert_eq!(r.added.len(), 2);
    assert_eq!((b.value(1, 1), b.value(2, 1), b.value(2, 2)), (Some(&text("a")), Some(&text("b")), None));
    let drafts: Vec<(u64, Vec<Value>)> = b.drafts().iter().map(|d| (d.id, d.cells.clone())).collect();
    assert_eq!(
        drafts,
        [
            (n, vec![text("9"), Value::Default, Value::Default]),
            (r.added[0], vec![text("7"), text("x"), Value::Default]),
            (r.added[1], vec![Value::Null, text("y"), Value::Default]),
        ]
    );
    let after = b.clone();
    assert!(b.undo());
    assert_eq!((&b.rows, &b.drafts), (&before.rows, &before.drafts), "undo restores exactly");
    assert!(b.undo(), "the edits before the paste are their own steps");
    assert!(b.redo() && b.redo());
    assert_eq!((&b.rows, &b.drafts), (&after.rows, &after.drafts), "redo re-applies the whole paste");
    assert!(!b.redo());
}

#[test]
fn a_paste_of_the_values_already_there_changes_nothing() {
    let mut b = EditBatch::new();
    let ops = paste_ops(&[((1, 0), text("1")), ((1, 1), text("n1")), ((1, 2), Value::Null)], &[], &[]);
    let at = b.revision();
    assert_eq!(b.paste(ops, orig), Ok(PasteResult { changed: 0, added: Vec::new() }));
    assert!(b.is_empty() && !b.can_undo());
    assert_eq!(b.revision(), at);
    // A cell pasted back to its value as read is no longer changed.
    b.set(&[(1, 1)], &text("x"), orig);
    let r = b.paste(paste_ops(&[((1, 1), text("n1")), ((1, 0), text("5"))], &[], &[]), orig).unwrap();
    assert_eq!(r.changed, 2);
    assert_eq!(b.row(1).unwrap().set, BTreeMap::from([(0, text("5"))]));
}

#[test]
fn a_paste_with_anything_it_cannot_do_changes_nothing() {
    let mut base = EditBatch::new();
    let n = base.add(usize::MAX, None, vec![Value::Default; 3]);
    base.delete(&[4], orig);
    base.set(&[(1, 1)], &text("x"), orig);
    let good = vec![((1, 1), text("y")), ((2, 0), text("7"))];
    let cases: Vec<(PasteOps, PasteRefused)> = vec![
        (paste_ops(&[good.clone(), vec![((4, 1), text("z"))]].concat(), &[], &[]), PasteRefused::Deleted { row: 4 }),
        (paste_ops(&good, &[((99, 0), text("z"))], &[]), PasteRefused::NoDraft { id: 99 }),
        (paste_ops(&good, &[((n, 3), text("z"))], &[]), PasteRefused::NoColumn { column: 3, width: 3 }),
        (paste_ops(&[((2, 5), text("z"))], &[], &[]), PasteRefused::NoColumn { column: 5, width: 3 }),
        (paste_ops(&good, &[], &[vec![text("z"); 3], vec![text("z"); 2]]), PasteRefused::Width { values: 2, width: 3 }),
        (paste_ops(&good, &[], &[vec![text("z"); 4]]), PasteRefused::Width { values: 4, width: 3 }),
        // The result's width, with nothing else to tell it.
        (paste_ops(&[], &[], &[vec![text("z"); 2]]), PasteRefused::Width { values: 2, width: 3 }),
        (paste_ops(&good, &[((n, 0), Value::Default)], &[]), PasteRefused::Default),
        (paste_ops(&[good.clone(), vec![((2, 1), Value::Default)]].concat(), &[], &[]), PasteRefused::Default),
    ];
    for (ops, why) in cases {
        let mut b = base.clone();
        let at = b.revision();
        assert_eq!(b.paste(ops, orig), Err(why.clone()));
        assert_eq!(b, base, "{why:?}: nothing changed");
        assert_eq!(b.revision(), at, "{why:?}");
        assert_eq!(b.next_draft, base.next_draft, "{why:?}");
    }
}

#[test]
fn a_cell_pasted_twice_takes_the_later_value_and_counts_once() {
    let mut b = EditBatch::new();
    let n = b.add(usize::MAX, None, vec![Value::Default; 3]);
    let ops = paste_ops(
        &[((1, 1), text("a")), ((1, 1), text("b")), ((2, 0), text("x")), ((2, 0), text("2"))],
        &[((n, 0), text("c")), ((n, 0), text("d"))],
        &[],
    );
    let r = b.paste(ops, orig).unwrap();
    assert_eq!(r.changed, 2, "(1, 1) once, (n, 0) once; (2, 0) ends as read");
    assert_eq!((b.value(1, 1), b.value(2, 0)), (Some(&text("b")), None));
    assert_eq!(b.drafts()[0].cells[0], text("d"));
}

#[test]
fn a_new_row_pasted_has_the_results_width_with_nothing_else_set() {
    let mut b = EditBatch::new();
    let ops = paste_ops(&[], &[], &[vec![text("z"); 2], vec![text("z"); 2]]);
    assert_eq!(b.paste(ops, orig), Err(PasteRefused::Width { values: 2, width: 3 }));
    assert_eq!(b, EditBatch::new());
    assert_eq!(b.paste(paste_ops(&[], &[], &[vec![text("z"); 3]]), orig).map(|r| r.added.len()), Ok(1));
}
