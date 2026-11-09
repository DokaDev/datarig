//! Statement execution and driver events: running, paging, cancelling and turning
//! [`DbEvent`](datarig_core::driver::DbEvent)s into results, tree and completion state.

mod errors;
mod events;
mod meta;
mod paging;
mod queue;
mod run;
mod tab_event;

pub(super) use errors::network_reason;
