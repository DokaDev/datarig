use super::*;
use crate::app::filter_dialog::tests::{cursors, dialects, target, texts};

/// The completion of a condition and of an order, at every place of the cursor in any text, as
/// the filter line and the dialog's SQL fields ask for it, never panics.
#[test]
fn the_filter_completions_never_panic() {
    for text in texts() {
        for d in dialects() {
            let t = target(d);
            for cursor in cursors(&text) {
                for force in [false, true] {
                    let asked = Completion { items: Vec::new(), replace_start: cursor, trail: 0 };
                    let _ = condition_keywords(None, &text, cursor, d, force);
                    let _ = condition_keywords(Some(asked), &text, cursor, d, force);
                    let _ = order_completion(&text, cursor, &t, force);
                }
            }
        }
    }
}
