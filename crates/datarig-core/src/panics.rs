//! Panics a future's runner catches itself (a driver's session task, which turns a panic into
//! a reported failure): while such a future is polled, [`caught_here`] says so, so the
//! process-wide panic hook can leave the terminal as it is and print nothing over the screen.

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

thread_local! {
    /// How many [`Caught`] futures this thread is polling now.
    static CAUGHT: Cell<u32> = const { Cell::new(0) };
}

/// Whether a panic on this thread now happens inside a future run under [`caught`].
pub fn caught_here() -> bool {
    CAUGHT.with(|c| c.get() > 0)
}

/// `future`, polled with [`caught_here`] true. The runner catches its panics
/// (`catch_unwind`).
pub fn caught<F: Future>(future: F) -> Caught<F> {
    Caught { inner: Box::pin(future) }
}

pub struct Caught<F> {
    inner: Pin<Box<F>>,
}

impl<F: Future> Future for Caught<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        /// Ends the scope, also when the poll unwinds.
        struct Scope;
        impl Drop for Scope {
            fn drop(&mut self) {
                CAUGHT.with(|c| c.set(c.get().saturating_sub(1)));
            }
        }
        CAUGHT.with(|c| c.set(c.get() + 1));
        let _scope = Scope;
        self.inner.as_mut().poll(cx)
    }
}
