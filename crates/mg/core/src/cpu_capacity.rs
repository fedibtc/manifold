//! How much CPU this host has, as a benchmark score
//! (`crates/mg/specs/REQ-seat-capacity-default.md`).
//!
//! The MG measures its CPU capacity once, at daemon start before seats start,
//! with a fixed CPU-only benchmark shaped like a seat's consensus work. The
//! capacity is the benchmark's iterations per second summed over one worker
//! thread per CPU the process may use; one iteration is one second of a
//! 10-guardian seat's consensus. A single worker's score, measured next, tells
//! per-core speed apart from core count. The measurement is stored with the
//! [`BENCHMARK_VERSION`], CPU count and CPU model it was taken at and taken
//! again only when a later start sees a different version, count or model,
//! for example after a benchmark change, a move to new hardware or a changed
//! CPU quota. A measurement that fails leaves the MG without a CPU seat limit
//! rather than stopping it.
//!
//! The benchmark does no IO: the readiness pressure check already gates the
//! disk ([`crate::host_pressure`]), and a startup benchmark must not open
//! endpoints or write files.

use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroUsize};
use std::sync::Barrier;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use fedimint_core::bitcoin::hashes::{Hash as _, sha256};
use fedimint_core::encoding::Encodable as _;
use fedimint_core::secp256k1::{Keypair, Message, SECP256K1, SecretKey, XOnlyPublicKey, schnorr};

use crate::db::Db;

#[cfg(test)]
#[path = "../tests/cpu_capacity.rs"]
mod tests;

/// Set in local E2E to run the benchmark at start, which local E2E otherwise
/// skips: it starts many MGs on one host.
pub const MG_E2E_CPU_BENCHMARK_ENV: &str = "MG_E2E_CPU_BENCHMARK";

/// The version of this benchmark. An iteration, and so the setup-payment
/// publication's `seat_cpu_iterations_per_sec`, means the work of this
/// version. Raise it with any change that moves the score, such as to the
/// constants below, the work in [`Worker`], its crypto dependencies or the
/// compiler settings; every MG then measures again at its next start. The new
/// version needs its own per-seat budget, so it also needs a new policy field
/// (`seat_cpu_iterations_per_sec_v2`) that only MGs of that version read.
pub const BENCHMARK_VERSION: u32 = 1;

/// The first run, discarded, brings clocks and caches to a steady state.
const WARM_UP: Duration = Duration::from_secs(1);
/// Each measured run of every worker; the score is the median of [`RUNS`].
const RUN: Duration = Duration::from_secs(3);
const RUNS: usize = 3;
/// The one run of a single worker, after the runs of every worker.
const SINGLE_CORE_RUN: Duration = Duration::from_secs(3);

/// Consensus rounds in one iteration: one second of a seat's consensus at the
/// production round delay of 50 ms.
const ROUNDS_PER_ITERATION: usize = 20;
/// The other guardians of a 10-guardian federation.
const PEERS: usize = 9;
/// The payload of one consensus unit.
const UNIT_BYTES: usize = 384;
/// Peer unit signatures verified per round, calibrated so the share of CPU in
/// secp256k1 matches an idle seat's profile.
const VERIFICATIONS_PER_ROUND: usize = 1;
/// One ecash transaction every this many iterations.
const TRANSACTION_EVERY_ITERATIONS: u64 = 100;
/// Notes in one transaction: a five-note mint-v2 reissue.
const NOTES_PER_TRANSACTION: usize = 5;

/// One CPU measurement of this host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuCapacity {
    /// The [`BENCHMARK_VERSION`] that measured it.
    pub benchmark_version: u32,
    /// The CPUs the MG may use when it measured, and so the benchmark's
    /// workers, from [`std::thread::available_parallelism`], which follows
    /// the CPU affinity mask and the cgroup CPU quota.
    pub cpu_count: NonZeroU32,
    /// The CPU model when it measured ([`cpu_model`]).
    pub cpu_model: String,
    /// Benchmark iterations per second, summed over all workers: the
    /// capacity the CPU seat limit divides.
    pub iterations_per_sec: u32,
    /// Benchmark iterations per second of one worker alone.
    pub single_core_iterations_per_sec: u32,
}

/// The two scores of one measurement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scores {
    pub iterations_per_sec: u32,
    pub single_core_iterations_per_sec: u32,
}

/// Return the stored measurement, or measure now when there is none or it
/// was taken by another benchmark version or at a different CPU count or
/// model. Call it once at daemon start, before seats start: the measurement
/// occupies every CPU for about 13 seconds.
///
/// Returns `None`, which is no CPU seat limit, when the CPU count is unknown,
/// the benchmark fails or the store fails: the CPU limit only narrows
/// admission, and an MG that cannot apply it still serves its seats.
pub async fn measure_at_start(db: &Db) -> Option<CpuCapacity> {
    let cpu_model = cpu_model(&std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default());
    let capacity = match std::thread::available_parallelism() {
        Ok(cpu_count) => measured(db, cpu_count, cpu_model, measure).await,
        Err(error) => Err(anyhow::Error::new(error).context("count the CPUs this MG may use")),
    };
    capacity
        .inspect_err(|error| {
            tracing::warn!(
                error = format_args!("{error:#}"),
                "cannot measure CPU capacity; seats get no CPU limit until a later start measures"
            );
        })
        .ok()
}

/// The first `model name` in `/proc/cpuinfo`, or an empty string where there
/// is none, as on most ARM hosts: then only the CPU count tells hosts apart.
pub(crate) fn cpu_model(cpuinfo: &str) -> String {
    cpuinfo
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == "model name").then(|| value.trim().to_owned())
        })
        .unwrap_or_default()
}

/// [`measure_at_start`] with the CPU count, model and measurement supplied.
pub(crate) async fn measured(
    db: &Db,
    cpu_count: NonZeroUsize,
    cpu_model: String,
    measure: impl FnOnce(NonZeroUsize) -> Scores + Send + 'static,
) -> anyhow::Result<CpuCapacity> {
    let count = NonZeroU32::try_from(cpu_count).unwrap_or(NonZeroU32::MAX);
    if let Some(stored) = db.cpu_capacity().await?
        && stored.benchmark_version == BENCHMARK_VERSION
        && stored.cpu_count == count
        && stored.cpu_model == cpu_model
    {
        return Ok(stored);
    }
    let started = Instant::now();
    let scores = tokio::task::spawn_blocking(move || measure(cpu_count))
        .await
        .context("run the CPU benchmark")?;
    let capacity = CpuCapacity {
        benchmark_version: BENCHMARK_VERSION,
        cpu_count: count,
        cpu_model,
        iterations_per_sec: scores.iterations_per_sec,
        single_core_iterations_per_sec: scores.single_core_iterations_per_sec,
    };
    db.record_cpu_capacity(&capacity).await?;
    tracing::info!(
        safe_to_share = true,
        benchmark_version = capacity.benchmark_version,
        cpu_count = capacity.cpu_count.get(),
        iterations_per_sec = capacity.iterations_per_sec,
        single_core_iterations_per_sec = capacity.single_core_iterations_per_sec,
        duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "measured CPU capacity"
    );
    Ok(capacity)
}

/// Measure this host: after a warm-up, the median of [`RUNS`] runs of one
/// worker per CPU, then one run of a single worker.
#[must_use]
pub fn measure(cpu_count: NonZeroUsize) -> Scores {
    run(cpu_count, WARM_UP);
    let iterations_per_sec = median(|| run(cpu_count, RUN));
    let single_core_iterations_per_sec = run(NonZeroUsize::MIN, SINGLE_CORE_RUN);
    Scores {
        // `as` saturates, which is the intent for an absurd score.
        iterations_per_sec: iterations_per_sec.round() as u32,
        single_core_iterations_per_sec: single_core_iterations_per_sec.round() as u32,
    }
}

fn median(mut run: impl FnMut() -> f64) -> f64 {
    let mut scores: Vec<f64> = (0..RUNS).map(|_| run()).collect();
    scores.sort_by(f64::total_cmp);
    scores[RUNS / 2]
}

/// One run: every worker iterates for `length` from a common start.
fn run(threads: NonZeroUsize, length: Duration) -> f64 {
    let start = Barrier::new(threads.get());
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.get())
            .map(|_| {
                scope.spawn(|| {
                    let worker = Worker::new();
                    start.wait();
                    let started = Instant::now();
                    let mut iterations = 0_u64;
                    while started.elapsed() < length {
                        worker.iteration(iterations);
                        iterations += 1;
                    }
                    iterations as f64 / started.elapsed().as_secs_f64()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a benchmark worker does not panic"))
            .sum()
    })
}

/// The keys and messages of one worker, made once so that iterations only
/// do the work a seat repeats.
struct Worker {
    keypair: Keypair,
    peer_signatures: Vec<(schnorr::Signature, Message, XOnlyPublicKey)>,
    tbs_secret: tbs::SecretKeyShare,
    tbs_public: tbs::AggregatePublicKey,
    blinded_note: tbs::BlindedMessage,
    note: tbs::Message,
    note_signature: tbs::Signature,
}

impl Worker {
    fn new() -> Self {
        let key = |byte: u8| {
            Keypair::from_secret_key(
                SECP256K1,
                &SecretKey::from_slice(&[byte; 32]).expect("a constant secret key is valid"),
            )
        };
        let peer_signatures = (0..PEERS as u8)
            .map(|peer| {
                let key = key(peer + 11);
                let message = Message::from_digest(sha256::Hash::hash(&[peer]).to_byte_array());
                (
                    SECP256K1.sign_schnorr(&message, &key),
                    message,
                    key.x_only_public_key().0,
                )
            })
            .collect();
        let tbs_secret = tbs::SecretKeyShare(bls12_381::Scalar::from(7_u64));
        let tbs_public = tbs::aggregate_public_key_shares(&BTreeMap::from([(
            0,
            tbs::derive_pk_share(&tbs_secret),
        )]));
        let note = tbs::Message::from_bytes_sha256(b"note nonce");
        let blinding = tbs::BlindingKey(bls12_381::Scalar::from(11_u64));
        let blinded_note = tbs::blind_message(note, blinding);
        let note_signature = tbs::unblind_signature(
            blinding,
            tbs::aggregate_signature_shares(&BTreeMap::from([(
                0,
                tbs::sign_message(blinded_note, tbs_secret),
            )])),
        );
        Self {
            keypair: key(7),
            peer_signatures,
            tbs_secret,
            tbs_public,
            blinded_note,
            note,
            note_signature,
        }
    }

    /// One second of a 10-guardian seat's consensus, plus every
    /// [`TRANSACTION_EVERY_ITERATIONS`]th iteration one transaction.
    fn iteration(&self, index: u64) {
        for round in 0..ROUNDS_PER_ITERATION {
            self.round(index, round);
        }
        if index.is_multiple_of(TRANSACTION_EVERY_ITERATIONS) {
            self.transaction();
        }
    }

    /// Encode, hash and sign one unit, and verify peer unit signatures.
    fn round(&self, index: u64, round: usize) {
        let mut unit = vec![0_u8; UNIT_BYTES];
        unit[..8].copy_from_slice(&index.to_le_bytes());
        unit[8..16].copy_from_slice(&round.to_le_bytes());
        let encoded = unit.consensus_encode_to_vec();
        let digest = sha256::Hash::hash(&encoded);
        std::hint::black_box(
            SECP256K1.sign_schnorr(&Message::from_digest(digest.to_byte_array()), &self.keypair),
        );
        self.verify_peers(VERIFICATIONS_PER_ROUND);
    }

    /// A five-note reissue: sign each blinded note, verify each input note
    /// and its spend signature.
    fn transaction(&self) {
        for _ in 0..NOTES_PER_TRANSACTION {
            std::hint::black_box(tbs::sign_message(self.blinded_note, self.tbs_secret));
            assert!(tbs::verify(self.note, self.note_signature, self.tbs_public));
        }
        self.verify_peers(NOTES_PER_TRANSACTION);
    }

    fn verify_peers(&self, count: usize) {
        for (signature, message, key) in self.peer_signatures.iter().cycle().take(count) {
            assert!(SECP256K1.verify_schnorr(signature, message, key).is_ok());
        }
    }
}
