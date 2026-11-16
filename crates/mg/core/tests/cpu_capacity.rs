use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

fn count(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).unwrap()
}

const EPYC: &str = "AMD EPYC 9454P 48-Core Processor";

/// A measurement that records how often it ran and scores `total` plus the
/// worker count, so a test can tell which run produced a stored value.
fn counting(
    total: u32,
    runs: &Arc<AtomicUsize>,
) -> impl FnOnce(NonZeroUsize) -> Scores + Send + 'static {
    let runs = runs.clone();
    move |workers| {
        runs.fetch_add(1, Ordering::SeqCst);
        Scores {
            iterations_per_sec: total + u32::try_from(workers.get()).unwrap(),
            single_core_iterations_per_sec: 800,
        }
    }
}

#[tokio::test]
async fn measures_once_and_again_only_when_the_cpu_count_or_model_changes() {
    let dir = tempfile::tempdir().unwrap();
    let base = crate::facts::PortBase::new(30_000).unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    {
        let db = Db::open(dir.path()).await.unwrap();
        assert_eq!(db.cpu_capacity().await.unwrap(), None);
        assert_eq!(db.offer_snapshot(base).await.unwrap().cpu_capacity, None);
        let first = measured(&db, count(8), EPYC.to_owned(), counting(4_000, &runs))
            .await
            .unwrap();
        assert_eq!(
            first,
            CpuCapacity {
                benchmark_version: BENCHMARK_VERSION,
                cpu_count: NonZeroU32::new(8).unwrap(),
                cpu_model: EPYC.to_owned(),
                iterations_per_sec: 4_008,
                single_core_iterations_per_sec: 800,
            }
        );
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // The snapshot reads the measurement beside the limits.
        assert_eq!(
            db.offer_snapshot(base).await.unwrap().cpu_capacity,
            Some(first)
        );
    }

    // A later start on the same CPUs reuses the stored value, even though a
    // new measurement would differ.
    let db = Db::open(dir.path()).await.unwrap();
    let reused = measured(&db, count(8), EPYC.to_owned(), counting(9_000, &runs))
        .await
        .unwrap();
    assert_eq!(reused.iterations_per_sec, 4_008);
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    // A different CPU count, such as a changed quota, measures again and
    // replaces the stored value.
    let moved = measured(&db, count(4), EPYC.to_owned(), counting(2_000, &runs))
        .await
        .unwrap();
    assert_eq!(
        (moved.cpu_count.get(), moved.iterations_per_sec),
        (4, 2_004)
    );
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(db.cpu_capacity().await.unwrap(), Some(moved));
    // Back on the 8 CPUs it measures again: only the latest is kept.
    measured(&db, count(8), EPYC.to_owned(), counting(4_000, &runs))
        .await
        .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 3);

    // The same CPU count on another CPU model, such as a move to other
    // hardware with as many vCPUs, measures again too.
    let other = measured(
        &db,
        count(8),
        "Intel Xeon".to_owned(),
        counting(1_000, &runs),
    )
    .await
    .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 4);
    assert_eq!(other.iterations_per_sec, 1_008);
    // A host without a model name keys on the count alone, and that is a
    // change from a named model as well.
    measured(&db, count(8), String::new(), counting(1_000, &runs))
        .await
        .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 5);
    let again = measured(&db, count(8), String::new(), counting(7_000, &runs))
        .await
        .unwrap();
    assert_eq!(
        (again.cpu_model.as_str(), again.iterations_per_sec),
        ("", 1_008)
    );
    assert_eq!(runs.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn a_stored_value_from_another_benchmark_version_measures_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).await.unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    // The same CPUs, measured by another version of the benchmark, whose
    // iterations mean other work.
    db.record_cpu_capacity(&CpuCapacity {
        benchmark_version: BENCHMARK_VERSION + 1,
        cpu_count: NonZeroU32::new(8).unwrap(),
        cpu_model: EPYC.to_owned(),
        iterations_per_sec: 9_999,
        single_core_iterations_per_sec: 999,
    })
    .await
    .unwrap();
    let measured = measured(&db, count(8), EPYC.to_owned(), counting(4_000, &runs))
        .await
        .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        (measured.benchmark_version, measured.iterations_per_sec),
        (BENCHMARK_VERSION, 4_008)
    );
    assert_eq!(db.cpu_capacity().await.unwrap(), Some(measured));
}

#[tokio::test]
async fn a_failed_measurement_stores_nothing_and_a_later_start_measures() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).await.unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    // A benchmark that panics, as when a worker thread cannot be spawned.
    let failed = measured(&db, count(8), EPYC.to_owned(), |_| -> Scores {
        panic!("cannot spawn a benchmark worker")
    })
    .await;
    assert!(failed.is_err());
    assert_eq!(db.cpu_capacity().await.unwrap(), None);
    let later = measured(&db, count(8), EPYC.to_owned(), counting(4_000, &runs))
        .await
        .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(db.cpu_capacity().await.unwrap(), Some(later));
}

#[tokio::test]
async fn measure_at_start_gives_no_capacity_rather_than_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).await.unwrap();
    // A store that fails before the benchmark runs.
    db.pool().close().await;
    assert_eq!(measure_at_start(&db).await, None);
}

#[test]
fn cpu_model_is_the_first_model_name_or_empty() {
    let x86 = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel\t\t: 17\n\
               model name\t: AMD EPYC 9454P 48-Core Processor\n\n\
               processor\t: 1\nmodel name\t: Other\n";
    // `model` alone is not `model name`, and the first CPU's name counts.
    assert_eq!(cpu_model(x86), EPYC);
    let arm = "processor\t: 0\nBogoMIPS\t: 50.00\nCPU implementer\t: 0x41\nCPU part\t: 0xd0c\n";
    assert_eq!(cpu_model(arm), "");
    assert_eq!(cpu_model(""), "");
}

#[test]
fn a_worker_iterates_and_a_run_sums_its_workers() {
    let worker = Worker::new();
    // The transaction iteration includes the tbs work and still verifies.
    worker.iteration(0);
    worker.iteration(1);
    let one = run(count(1), Duration::from_millis(200));
    let two = run(count(2), Duration::from_millis(200));
    // Comparing the two runs would flake on a loaded runner.
    assert!(one > 0.0, "{one}");
    assert!(two > 0.0, "{two}");
}

#[test]
fn median_takes_the_middle_run() {
    let mut runs = [9.0, 1.0, 5.0].into_iter();
    assert_eq!(median(|| runs.next().unwrap()), 5.0);
}
