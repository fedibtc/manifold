use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::{ACTIVE_FOR, Coordinator};

fn waiter() -> (oneshot::Sender<()>, oneshot::Receiver<()>) {
    oneshot::channel()
}

/// Makes `seats` active at `at`, leaving no window open.
fn warm_up(coordinator: &mut Coordinator, seats: &[u64], at: Instant) {
    for &seat in seats {
        let (go, _) = waiter();
        coordinator.ready(seat, go, at);
    }
    coordinator.close(coordinator.window);
}

#[test]
fn releases_when_every_active_seat_waits() {
    let mut coordinator = Coordinator::default();
    let start = Instant::now();
    warm_up(&mut coordinator, &[0, 1, 2], start);

    let (go_a, mut a) = waiter();
    let (go_b, mut b) = waiter();
    let (go_c, mut c) = waiter();
    let later = start + Duration::from_millis(50);
    let window = coordinator.ready(0, go_a, later);
    assert!(window.is_some(), "the first seat opens a window");
    assert!(coordinator.ready(1, go_b, later).is_none());
    assert!(
        a.try_recv().is_err() && b.try_recv().is_err(),
        "two of three still wait"
    );
    assert!(coordinator.ready(2, go_c, later).is_none());
    assert!(a.try_recv().is_ok() && b.try_recv().is_ok() && c.try_recv().is_ok());

    // The opener's timer then fires on a window that is already closed and
    // must not release the next window early.
    let (go_d, mut d) = waiter();
    let next = coordinator.ready(0, go_d, later + Duration::from_millis(50));
    coordinator.close(window.expect("checked above"));
    assert!(
        d.try_recv().is_err(),
        "a stale timer released a later window"
    );
    coordinator.close(next.expect("seats 1 and 2 are still active"));
    assert!(d.try_recv().is_ok());
}

#[test]
fn stalled_seat_stops_holding_the_window() {
    let start = Instant::now();

    // Seat 7 said ready recently: seat 1 waits for it.
    let mut coordinator = Coordinator::default();
    warm_up(&mut coordinator, &[7, 1], start);
    let (go, mut released) = waiter();
    let recent = start + ACTIVE_FOR - Duration::from_millis(1);
    assert!(coordinator.ready(1, go, recent).is_some());
    assert!(released.try_recv().is_err());

    // Seat 7 last said ready longer ago: seat 1 goes at once.
    let mut coordinator = Coordinator::default();
    warm_up(&mut coordinator, &[7, 1], start);
    let (go, mut released) = waiter();
    let stale = start + ACTIVE_FOR + Duration::from_millis(1);
    assert!(coordinator.ready(1, go, stale).is_none());
    assert!(released.try_recv().is_ok());
}
