use super::*;

fn test_lock() -> DatabaseWriteLock {
    DatabaseWriteLock::new(NonZeroUsize::MIN)
}

#[tokio::test]
async fn request_admission_is_bounded_and_cancellation_releases_the_permit() {
    let lock = test_lock();
    let holder = lock.acquire_worker().await;
    let acquisition = lock.observe_next_acquisition().await;
    let waiting_lock = lock.clone();
    let waiting_request = tokio::spawn(async move { waiting_lock.acquire_request().await });
    acquisition
        .await
        .expect("request reached the request queue boundary");

    assert!(matches!(
        lock.acquire_request().await,
        Err(WriteAdmissionError::Saturated)
    ));

    waiting_request.abort();
    assert!(
        waiting_request
            .await
            .is_err_and(|error| error.is_cancelled())
    );

    let acquisition = lock.observe_next_acquisition().await;
    let replacement_lock = lock.clone();
    let replacement_request = tokio::spawn(async move { replacement_lock.acquire_request().await });
    acquisition
        .await
        .expect("replacement reached the request queue boundary");
    drop(holder);
    assert!(replacement_request.await.expect("replacement task").is_ok());
}

#[tokio::test]
async fn worker_waits_for_the_request_that_entered_before_it() {
    let lock = test_lock();
    let holder = lock.acquire_worker().await;

    let acquisition = lock.observe_next_acquisition().await;
    let request_lock = lock.clone();
    let request = tokio::spawn(async move { request_lock.acquire_request().await });
    acquisition
        .await
        .expect("request reached the request queue boundary");

    let acquisition = lock.observe_next_acquisition().await;
    let worker_lock = lock.clone();
    let worker = tokio::spawn(async move { worker_lock.acquire_worker().await });
    acquisition
        .await
        .expect("worker reached the mutation-lock boundary");
    assert!(matches!(
        lock.acquire_request().await,
        Err(WriteAdmissionError::Saturated)
    ));

    drop(holder);
    let request_guard = request.await.expect("request task").expect("request guard");
    drop(request_guard);
    drop(worker.await.expect("worker task"));
}

#[tokio::test]
async fn queued_worker_preempts_requests_at_the_default_admission_capacity() {
    let lock = DatabaseWriteLock::default();
    let holder = lock.acquire_worker().await;
    let acquisition = lock.observe_next_acquisition().await;
    let first_request_lock = lock.clone();
    let first_request = tokio::spawn(async move { first_request_lock.acquire_request().await });
    acquisition
        .await
        .expect("first request reached the request queue boundary");

    let mut later_requests = Vec::with_capacity(
        DEFAULT_DATABASE_WRITE_REQUEST_ADMISSION
            .get()
            .saturating_sub(1),
    );
    for _ in 1..DEFAULT_DATABASE_WRITE_REQUEST_ADMISSION.get() {
        let acquisition = lock.observe_next_acquisition().await;
        let request_lock = lock.clone();
        later_requests.push(tokio::spawn(
            async move { request_lock.acquire_request().await },
        ));
        acquisition
            .await
            .expect("request reached the request queue boundary");
    }

    let acquisition = lock.observe_next_acquisition().await;
    let worker_lock = lock.clone();
    let worker = tokio::spawn(async move { worker_lock.acquire_worker().await });
    acquisition
        .await
        .expect("worker reached the mutation-lock boundary");

    drop(holder);
    let first_request_guard = first_request
        .await
        .expect("first request task")
        .expect("first request admission");
    drop(first_request_guard);

    // The writer proceeds before every request still queued at request serialization.
    drop(worker.await.expect("worker task"));
    for request in later_requests {
        drop(
            request
                .await
                .expect("later request task")
                .expect("later request admission"),
        );
    }
}
