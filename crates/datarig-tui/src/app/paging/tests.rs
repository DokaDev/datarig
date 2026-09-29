use super::{Paging, fmt_left};
use std::time::{Duration, Instant};

const T: Option<Duration> = Some(Duration::from_secs(30));

#[test]
fn an_open_portal_counts_down_from_its_last_page() {
    let t0 = Instant::now();
    let p = Paging::Open { since: t0, in_block: false };
    assert_eq!(p.left(t0, T), Some(Duration::from_secs(30)));
    assert_eq!(p.left(t0 + Duration::from_millis(6_500), T), Some(Duration::from_millis(23_500)));
    assert!(!p.due(t0 + Duration::from_millis(29_999), T));
    assert!(p.due(t0 + Duration::from_secs(30), T));
    assert!(p.due(t0 + Duration::from_secs(90), T), "late ticks still close it");
    assert_eq!(p.left(t0 + Duration::from_secs(90), T), Some(Duration::ZERO));
    // A new page restarts the count.
    let p = Paging::Open { since: t0 + Duration::from_secs(20), in_block: false };
    assert!(!p.due(t0 + Duration::from_secs(30), T));
}

#[test]
fn nothing_is_due_without_an_open_portal_or_a_timeout() {
    let t0 = Instant::now();
    let later = t0 + Duration::from_secs(3600);
    for p in [Paging::None, Paging::ClosedIdle, Paging::Replaced, Paging::Interrupted, Paging::Stopped] {
        assert_eq!(p.left(later, T), None);
        assert!(!p.due(later, T));
    }
    let open = Paging::Open { since: t0, in_block: false };
    assert_eq!(open.left(later, None), None, "policy: off");
    assert!(!open.due(later, None));
    // A portal inside the user's transaction is never closed for being idle.
    let in_tx = Paging::Open { since: t0, in_block: true };
    assert_eq!(in_tx.left(later, T), None);
    assert!(!in_tx.due(later, T));
}

#[test]
fn pages_count_whole_pages_from_the_first_row() {
    assert_eq!(super::page_of(0, 500), 0);
    assert_eq!(super::page_of(499, 500), 0);
    assert_eq!(super::page_of(500, 500), 1);
    assert_eq!(super::pages(0, 500), 1);
    assert_eq!(super::pages(500, 500), 1);
    assert_eq!(super::pages(501, 500), 2);
    assert_eq!(super::pages(19_873, 500), 40);
    assert!(Paging::ClosedIdle.closed() && Paging::Replaced.closed() && Paging::Interrupted.closed());
    assert!(!Paging::Stopped.closed() && !Paging::None.closed());
}

#[test]
fn remaining_time_rounds_up_to_whole_seconds() {
    assert_eq!(fmt_left(Duration::from_secs(30)), "30s");
    assert_eq!(fmt_left(Duration::from_millis(23_400)), "24s");
    assert_eq!(fmt_left(Duration::from_millis(1)), "1s");
    assert_eq!(fmt_left(Duration::ZERO), "0s");
}
