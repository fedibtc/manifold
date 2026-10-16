use std::sync::Arc;

use super::*;

#[tokio::test(start_paused = true)]
async fn startup_guard_samples_time_after_two_seconds() -> anyhow::Result<()> {
    let start = Instant::now();
    let mut writer = AdvertisementWriter::new(Timestamp(100));
    let issued = writer
        .next_timestamp(start + WAIT_TIMEOUT, || {
            assert!(start.elapsed() >= Duration::from_secs(2));
            Timestamp(100 + start.elapsed().as_secs())
        })
        .await?;
    assert_eq!(issued, Timestamp(102));
    assert_eq!(start.elapsed(), Duration::from_secs(2));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn ambiguous_attempt_and_canceled_io_keep_their_in_memory_order() -> anyhow::Result<()> {
    let start = Instant::now();
    let writer = Arc::new(tokio::sync::Mutex::new(AdvertisementWriter::new(
        Timestamp(100),
    )));
    let (sent, observed) = tokio::sync::oneshot::channel();
    let canceled = {
        let writer = writer.clone();
        tokio::spawn(async move {
            let mut writer = writer.lock().await;
            let timestamp = writer
                .next_timestamp(Instant::now() + WAIT_TIMEOUT, || {
                    Timestamp(100 + start.elapsed().as_secs())
                })
                .await
                .unwrap();
            sent.send(timestamp).unwrap();
            // Stand in for a relay send whose outcome is lost to cancellation.
            std::future::pending::<()>().await;
        })
    };
    assert_eq!(observed.await?, Timestamp(102));
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());

    let mut writer = writer.lock().await;
    let next = writer
        .next_timestamp(Instant::now() + WAIT_TIMEOUT, || {
            Timestamp(100 + start.elapsed().as_secs())
        })
        .await?;
    assert_eq!(next, Timestamp(103));
    // A transport error after that reservation must not put its slot back.
    let retry = writer
        .next_timestamp(Instant::now() + WAIT_TIMEOUT, || {
            Timestamp(100 + start.elapsed().as_secs())
        })
        .await?;
    assert_eq!(retry, Timestamp(104));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stalled_or_backward_clocks_fail_within_the_original_budget() {
    for wall in [100, 99] {
        let mut writer = AdvertisementWriter::new(Timestamp(100));
        let deadline = Instant::now() + WAIT_TIMEOUT;
        assert!(
            writer
                .next_timestamp(deadline, || Timestamp(wall))
                .await
                .is_err()
        );
        assert_eq!(Instant::now(), deadline);
        assert_eq!(writer.last_attempt, Timestamp(101));
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_startup_does_not_bypass_the_remaining_gate() -> anyhow::Result<()> {
    let start = Instant::now();
    let mut writer = AdvertisementWriter::new(Timestamp(100));
    {
        let waiting = writer.next_timestamp(start + WAIT_TIMEOUT, || Timestamp(102));
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        tokio::time::advance(Duration::from_secs(1)).await;
    }
    let issued = writer
        .next_timestamp(start + WAIT_TIMEOUT, || Timestamp(102))
        .await?;
    assert_eq!(issued, Timestamp(102));
    assert_eq!(start.elapsed(), Duration::from_secs(2));
    Ok(())
}
