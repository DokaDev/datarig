//! The filter and sort dialog of a table tab: the conditions, order and cap of the table's rows,
//! built as chips and keys and applied as one query.
//!
//! [`draft`] holds what the dialog edits, as data: no drawing, no keys, no app state.

// The dialog that uses the draft is not wired in yet.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod draft;
