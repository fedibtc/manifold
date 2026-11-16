//! Run the MG's CPU benchmark (`mg_core::cpu_capacity`) on this host, as the
//! daemon does at its first start, and print both scores.
//!
//! Run: `cargo run --release -p mg-core --example cpu_benchmark -- [WORKERS]`
//! (default: one worker per available CPU).

use std::time::Instant;

fn main() {
    let workers = match std::env::args().nth(1) {
        Some(workers) => workers.parse().expect("WORKERS is a positive integer"),
        None => std::thread::available_parallelism().expect("count the available CPUs"),
    };
    let started = Instant::now();
    let scores = mg_core::cpu_capacity::measure(workers);
    println!(
        "workers {workers}: {} iterations/s, single worker {} iterations/s, {:.1} s",
        scores.iterations_per_sec,
        scores.single_core_iterations_per_sec,
        started.elapsed().as_secs_f64(),
    );
}
