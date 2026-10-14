//! A panic inside a future that is run under [`datarig_core::panics::caught`] is known to be
//! caught while the panic hook runs (the binary's hook then leaves the terminal as it is and
//! prints nothing); outside it, it is not.

use datarig_core::panics::{caught, caught_here};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};

#[test]
fn a_panic_the_runner_catches_is_known_as_caught_to_the_hook() {
    let seen: Arc<Mutex<Vec<bool>>> = Arc::default();
    let hook = std::panic::take_hook();
    let record = seen.clone();
    std::panic::set_hook(Box::new(move |_| record.lock().unwrap().push(caught_here())));
    let inside = futures::executor::block_on(AssertUnwindSafe(caught(async { panic!("inside") })).catch_unwind());
    let outside = futures::executor::block_on(AssertUnwindSafe(async { panic!("outside") }).catch_unwind());
    std::panic::set_hook(hook);
    assert!(inside.is_err() && outside.is_err());
    assert_eq!(*seen.lock().unwrap(), [true, false]);
    // The scope ends with the unwinding.
    assert!(!caught_here());
}
