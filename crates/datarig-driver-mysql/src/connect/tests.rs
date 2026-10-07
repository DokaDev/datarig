use super::*;

/// A panic in a session's task (a bug, here or in a crate under the driver) still ends the
/// session with one reported failure: `ConnectFailed` before it connected, `Lost` after. The
/// panic's message is not in it.
#[tokio::test]
async fn a_panic_in_the_session_ends_it_with_a_reported_failure() {
    let (events, mut rx) = unbounded_channel();
    let connected = AtomicBool::new(false);
    guarded(&events, &connected, async { panic!("s3cret-Pass-zz") }).await;
    match rx.try_recv() {
        Ok(DbEvent::ConnectFailed { error: DbError::Connection(f), auth: false }) => {
            assert!(!f.detail.contains("s3cret"), "{f:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(rx.try_recv().is_err(), "one event");
    connected.store(true, Ordering::SeqCst);
    guarded(&events, &connected, async { panic!("s3cret-Pass-zz") }).await;
    assert!(matches!(rx.try_recv(), Ok(DbEvent::Lost { error: DbError::Connection(_) })));
    // A task that ends as it should reports nothing more.
    guarded(&events, &connected, async {}).await;
    assert!(rx.try_recv().is_err());
}
