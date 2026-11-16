//! Fixed CPU benchmark shaped like a guardian seat's work, for normalizing the
//! seat cost model across hosts. Not wired into the MG; see
//! `scripts/seat-cost-sim/README.md`.
//!
//! One **iteration** is one second of a 10-guardian seat's consensus at the
//! production round delay (50 ms, so 20 rounds) plus a tenth of a
//! transaction:
//!
//! - per **round**: consensus-encode and hash one 300-byte unit, sign it with
//!   secp256k1 Schnorr, verify 9 peer unit signatures, exchange 9 × 300-byte
//!   messages each way over a loopback iroh QUIC connection (the seat's p2p
//!   transport: noq, AEAD, UDP syscalls, the tokio runtime), and commit one
//!   RocksDB transaction through `fedimint-rocksdb` that writes the unit and
//!   reads one key;
//! - every 10th iteration, one **transaction**: a five-note mint-v2 reissue
//!   (5 `tbs::sign_message`, 5 `tbs::verify`, 5 Schnorr verifications, one
//!   RocksDB transaction of 10 entries).
//!
//! Each worker thread runs its own single-worker tokio runtime, endpoints,
//! and database. With `--threads 0` (the default) it starts one worker per
//! available CPU, which is the host-capacity measurement; `--threads 1`
//! gives the per-core reference. The score is iterations per second summed
//! over workers, after a warm-up, over a fixed run length; the run repeats
//! and reports every repeat and the median. Component micro-benchmarks
//! follow with `--components`.
//!
//! Run: `cargo run --release -p tests-e2e --example seat_crypto_bench -- [--threads N] [--secs S] [--repeats R] [--components]`

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bls12_381::{G2Projective, Scalar};
use fedi_iroh_rpc::iroh::Endpoint;
use fedi_iroh_rpc::iroh::endpoint::presets;
use fedimint_core::bitcoin::hashes::{Hash as _, sha256};
use fedimint_core::db::{Database, IDatabaseTransactionOpsCore as _};
use fedimint_core::encoding::Encodable as _;
use fedimint_core::secp256k1::{Keypair, Message as SecpMessage, SECP256K1, SecretKey, schnorr};
use group::Curve as _;
use group::ff::Field as _;
use rand::rngs::OsRng;

const ALPN: &[u8] = b"seat-bench/0";
const PEERS: usize = 9;
const ROUNDS_PER_ITERATION: usize = 20;
const TX_EVERY_ITERATIONS: u64 = 100;
const NOTES_PER_TX: usize = 5;
const UNIT_BYTES: usize = 400;
/// Schnorr verifications per round, calibrated so the share of CPU in
/// secp256k1 matches an idle seat's profile.
const VERIFICATIONS_PER_ROUND: usize = 1;

struct Crypto {
    keypair: Keypair,
    peer_sigs: Vec<(
        schnorr::Signature,
        SecpMessage,
        fedimint_core::secp256k1::XOnlyPublicKey,
    )>,
    tbs_sk: tbs::SecretKeyShare,
    tbs_pk: tbs::AggregatePublicKey,
    blinded: tbs::BlindedMessage,
    note_message: tbs::Message,
    note_signature: tbs::Signature,
}

impl Crypto {
    fn new() -> Self {
        let keypair =
            Keypair::from_secret_key(SECP256K1, &SecretKey::from_slice(&[7; 32]).unwrap());
        let peer_sigs = (0..PEERS as u8)
            .map(|peer| {
                let key = Keypair::from_secret_key(
                    SECP256K1,
                    &SecretKey::from_slice(&[peer + 11; 32]).unwrap(),
                );
                let message = SecpMessage::from_digest(sha256::Hash::hash(&[peer]).to_byte_array());
                (
                    SECP256K1.sign_schnorr(&message, &key),
                    message,
                    key.x_only_public_key().0,
                )
            })
            .collect();
        let sk = Scalar::random(&mut OsRng);
        let tbs_sk = tbs::SecretKeyShare(sk);
        let tbs_pk = tbs::AggregatePublicKey((G2Projective::generator() * sk).to_affine());
        let note_message = tbs::Message::from_bytes_sha256(b"note nonce");
        let blinding = tbs::BlindingKey::random();
        let blinded = tbs::blind_message(note_message, blinding);
        let note_signature = tbs::unblind_signature(
            blinding,
            tbs::aggregate_signature_shares(&[(0, tbs::sign_message(blinded, tbs_sk))].into()),
        );
        assert!(tbs::verify(note_message, note_signature, tbs_pk));
        Self {
            keypair,
            peer_sigs,
            tbs_sk,
            tbs_pk,
            blinded,
            note_message,
            note_signature,
        }
    }

    fn verify_peers(&self, count: usize) {
        for (signature, message, key) in self.peer_sigs.iter().cycle().take(count) {
            assert!(SECP256K1.verify_schnorr(signature, message, key).is_ok());
        }
    }
}

struct Worker {
    crypto: Crypto,
    db: Database,
    /// One connection to each peer, as a seat has.
    peers: Vec<fedi_iroh_rpc::iroh::endpoint::Connection>,
    local: Endpoint,
    _peer_endpoints: Vec<Endpoint>,
    key: u64,
}

impl Worker {
    async fn new(dir: &std::path::Path) -> anyhow::Result<Self> {
        let local = Endpoint::builder(presets::N0DisableRelay)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await?;
        let mut peer_endpoints = Vec::new();
        let mut peers = Vec::new();
        for _ in 0..PEERS {
            let peer = Endpoint::builder(presets::N0DisableRelay)
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await?;
            // The peer: answer every unit with one of its own on a new
            // stream, as fedimint's p2p sends each message on its own
            // unidirectional stream.
            let accepting = peer.clone();
            tokio::spawn(async move {
                let Some(incoming) = accepting.accept().await else {
                    return;
                };
                let Ok(connection) = incoming.await else {
                    return;
                };
                while let Ok(mut recv) = connection.accept_uni().await {
                    let Ok(unit) = recv.read_to_end(UNIT_BYTES * 2).await else {
                        return;
                    };
                    let Ok(mut send) = connection.open_uni().await else {
                        return;
                    };
                    if send.write_all(&unit).await.is_err() || send.finish().is_err() {
                        return;
                    }
                }
            });
            peers.push(local.connect(peer.addr(), ALPN).await?);
            peer_endpoints.push(peer);
        }
        let db: Database = fedimint_rocksdb::RocksDb::build(dir.join("db"))
            .open()
            .await?
            .into();
        Ok(Self {
            crypto: Crypto::new(),
            db,
            peers,
            local,
            _peer_endpoints: peer_endpoints,
            key: 0,
        })
    }

    async fn round(&mut self) -> anyhow::Result<()> {
        let mut unit = vec![0_u8; UNIT_BYTES - 16];
        unit[..8].copy_from_slice(&self.key.to_le_bytes());
        let encoded = unit.consensus_encode_to_vec();
        let digest = sha256::Hash::hash(&encoded);
        let signature = SECP256K1.sign_schnorr(
            &SecpMessage::from_digest(digest.to_byte_array()),
            &self.crypto.keypair,
        );
        let mut message = encoded.clone();
        message[..64].copy_from_slice(signature.as_ref());
        for peer in &self.peers {
            let mut send = peer.open_uni().await?;
            send.write_all(&message).await?;
            send.finish()?;
        }
        let mut received = Vec::with_capacity(PEERS);
        for peer in &self.peers {
            let mut recv = peer.accept_uni().await?;
            received.push(recv.read_to_end(UNIT_BYTES * 2).await?);
        }
        self.crypto.verify_peers(VERIFICATIONS_PER_ROUND);
        let mut dbtx = self.db.begin_transaction().await;
        for unit in std::iter::once(&encoded).chain(&received) {
            self.key += 1;
            dbtx.raw_insert_bytes(&self.key.to_be_bytes(), unit).await?;
        }
        let _ = dbtx.raw_get_bytes(&(self.key / 2).to_be_bytes()).await?;
        dbtx.commit_tx_result().await?;
        Ok(())
    }

    async fn transaction(&mut self) -> anyhow::Result<()> {
        for _ in 0..NOTES_PER_TX {
            std::hint::black_box(tbs::sign_message(self.crypto.blinded, self.crypto.tbs_sk));
            assert!(tbs::verify(
                self.crypto.note_message,
                self.crypto.note_signature,
                self.crypto.tbs_pk
            ));
        }
        self.crypto.verify_peers(NOTES_PER_TX);
        let mut dbtx = self.db.begin_transaction().await;
        for entry in 0..2 * NOTES_PER_TX as u64 {
            self.key += 1;
            dbtx.raw_insert_bytes(
                &[
                    b"tx".as_slice(),
                    &self.key.to_be_bytes(),
                    &entry.to_be_bytes(),
                ]
                .concat(),
                &[1; 100],
            )
            .await?;
        }
        dbtx.commit_tx_result().await?;
        Ok(())
    }

    async fn iteration(&mut self, index: u64) -> anyhow::Result<()> {
        for _ in 0..ROUNDS_PER_ITERATION {
            self.round().await?;
        }
        if index % TX_EVERY_ITERATIONS == 0 {
            self.transaction().await?;
        }
        Ok(())
    }
}

fn run(threads: usize, warmup: Duration, measure: Duration) -> f64 {
    let base = std::env::temp_dir().join(format!("seat-bench-{}", std::process::id()));
    let measuring = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let counted = Arc::new(AtomicU64::new(0));
    let ready = Arc::new(std::sync::Barrier::new(threads + 1));
    let handles: Vec<_> = (0..threads)
        .map(|index| {
            let (dir, measuring, stop, counted, ready) = (
                base.join(index.to_string()),
                measuring.clone(),
                stop.clone(),
                counted.clone(),
                ready.clone(),
            );
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async move {
                    let mut worker = Worker::new(&dir).await.expect("start worker");
                    ready.wait();
                    let mut index = 0;
                    while !stop.load(Ordering::Relaxed) {
                        worker.iteration(index).await.expect("iteration");
                        index += 1;
                        if measuring.load(Ordering::Relaxed) {
                            counted.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            })
        })
        .collect();
    ready.wait();
    std::thread::sleep(warmup);
    measuring.store(true, Ordering::Relaxed);
    let started = Instant::now();
    std::thread::sleep(measure);
    let done = counted.load(Ordering::Relaxed);
    let elapsed = started.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    for handle in handles {
        handle.join().unwrap();
    }
    let _ = std::fs::remove_dir_all(&base);
    done as f64 / elapsed
}

fn components() {
    let crypto = Crypto::new();
    let time = |iterations: u32, body: &mut dyn FnMut()| {
        let started = Instant::now();
        for _ in 0..iterations {
            body();
        }
        started.elapsed().as_secs_f64() / f64::from(iterations) * 1e6
    };
    let message = SecpMessage::from_digest([1; 32]);
    println!(
        "component us/op: schnorr_verify {:.1}, schnorr_sign {:.1}, tbs_sign {:.1}, tbs_verify {:.1}",
        time(20_000, &mut || crypto.verify_peers(1)),
        time(20_000, &mut || {
            std::hint::black_box(SECP256K1.sign_schnorr(&message, &crypto.keypair));
        }),
        time(1_000, &mut || {
            std::hint::black_box(tbs::sign_message(crypto.blinded, crypto.tbs_sk));
        }),
        time(200, &mut || assert!(tbs::verify(
            crypto.note_message,
            crypto.note_signature,
            crypto.tbs_pk
        ))),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str, default: u64| {
        args.iter()
            .position(|a| a == name)
            .map_or(default, |i| args[i + 1].parse().expect("numeric argument"))
    };
    let threads = match arg("--threads", 0) {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n as usize,
    };
    let secs = arg("--secs", 10);
    let repeats = arg("--repeats", 3);
    let warmup = Duration::from_secs(arg("--warmup-secs", 2));
    let mut scores: Vec<f64> = (0..repeats)
        .map(|_| run(threads, warmup, Duration::from_secs(secs)))
        .collect();
    let all = scores.clone();
    scores.sort_by(f64::total_cmp);
    let median = scores[scores.len() / 2];
    println!(
        "{}",
        serde_json::json!({
            "threads": threads,
            "secs": secs,
            "iterations_per_sec": all,
            "median_iterations_per_sec": median,
            "per_thread": median / threads as f64,
        })
    );
    if args.iter().any(|a| a == "--components") {
        components();
    }
}
