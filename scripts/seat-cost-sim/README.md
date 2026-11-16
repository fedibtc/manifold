# Seat-cost simulation

Measures what one active guardian seat costs the Manifold Guardian (MG) that
hosts it: CPU, memory, disk writes, pressure, and network bytes, as the
number of seats N and the per-federation transaction rate vary.

## Rig

- **MG under test (MUT):** one release `fleet-manager` with N seats. Each
  seat is one guardian of its own federation. It runs in its own cgroup
  (`<scope>/mut`), on a fixed CPU set (`taskset`), and alone in its own
  network namespace behind a veth pair (`vmut` 10.77.0.2 ⇄ `vrig` 10.77.0.1).
  Every byte it exchanges with peers, clients, the gateway, bitcoind, and the
  relay crosses that veth; its own loopback traffic (MG ⇄ its fedimintd
  children) does not.
- **Helpers:** the other guardians of every federation run on helper MGs
  outside the measured cgroup and namespace, on a separate CPU set.
  Federations have `SEAT_SIM_GUARDIANS` guardians (default 10; fa-cli
  requires 7–20), so there are `SEAT_SIM_GUARDIANS - 1` helper MGs, each with
  N seats.
- **Chain and services:** a defe regtest bitcoind and Nostr relay, a
  `gatewayd` (LDK, regtest) without `FM_IN_DEVIMINT`, and a block every
  `SEAT_SIM_BLOCK_SECS` (600) seconds.
- **Load:** per federation two in-process fedimint clients, funded by a
  walletv2 peg-in, run user operations at the requested rate with
  exponential inter-arrival times, alternating a mint-v2 ecash transfer and
  a Lightning v2 payment through the gateway (a direct swap: the gateway
  pays and receives in the same federation), and alternating direction.
- **Production cadence:** the MUT's fedimintd children run without
  `FM_IN_DEVIMINT`, so the federations use production consensus parameters
  (3600 rounds of at least 50 ms per session) and production walletv2
  polling. The MUT logs at the production default (`info`); its stderr goes
  through a pipe to a logger outside its cgroup, as journald would.
- The whole rig lives in an unprivileged user and network namespace with
  no route out, so it cannot reach any shared or production service.

The rig is the opt-in test `seat_cost_rig::seat_cost_rig` in
`crates/tests-e2e/tests/manifold_guardian_0_1_formation/seat_cost_rig.rs`,
built on the formation harness of that file. The only product change is the
test-only `MG_E2E_IROH_BIND_IP` override in `mg-core`'s seat spawner: with
`MG_E2E_LOCAL_IROH` set, seats bind their iroh sockets to that address
instead of loopback, so peers in another namespace can reach them.

## Running

```bash
cargo build --release -p mg --bin fleet-manager -p mg-cli --bin fman-cli \
  -p fa-cli --bin fa-cli -p defe --bin defe -p devmon --bin manifold-test-issuer
cargo test --release -p tests-e2e --test manifold_guardian_0_1_formation --no-run
# note the printed test executable path

export SEAT_SIM_TEST_BIN=<test executable> SEAT_SIM_RELEASE_DIR=$PWD/target/release
export SEAT_SIM_GUARDIANS=10 SEAT_SIM_MAX_SEATS=16
scripts/seat-cost-sim/launch.sh <run-dir> 8-11 16-47 > <run-dir>/rig.log 2>&1 &

holder=$(cat <run-dir>/mut-netns-holder.pid)
nsenter -t "$holder" -U -n --preserve-credentials -- \
  python3 scripts/seat-cost-sim/sample.py <run-dir> <run-dir>/samples.jsonl &

scripts/seat-cost-sim/drive.sh <run-dir> "1:0 1:0.01 1:0.1 1:1 4:0 ..." 180 600
scripts/seat-cost-sim/analyze.py <run-dir>

# stop: the rig shuts the MGs down cleanly, then the scope ends
echo '{"federations": 0, "ops_per_sec": 0, "stop": true}' > <run-dir>/control.json
```

The rig's control file is `<run-dir>/control.json`:
`{"federations": N, "ops_per_sec": r, "stop": false}`. Federations are only
added. The MUT's CPU set can be changed while it runs with
`taskset -a -c -p <cpus> <pid>` on every pid in `<scope>/mut/cgroup.procs`.

## What is measured

`sample.py` reads, every 5 s, the same sources as
`crates/mg/bin/src/resource_report.rs`: per-process `/proc/<pid>/stat` CPU
ticks, `status` RSS, `io` storage bytes (plus `smaps_rollup` PSS), and the
cgroup's `cpu.stat`, `cpu.pressure`, `io.pressure`, `memory.current`, and
`memory.stat`; the veth's `/proc/net/dev` counters; `/proc/stat` lines of the
MUT's CPUs, to show other users' load on them; and every 10 s each seat's
fedimintd metrics (`fm_peer_transport_bytes_total`, peer message counters,
`fm_consensus_session_count`, ordering latency, processed transactions, API
connections). `drive.sh` records each point's window and nft byte counters
in the MUT namespace split by transport (UDP is iroh: p2p and client API;
TCP is bitcoind RPC and the relay).
