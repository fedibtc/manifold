#!/usr/bin/env python3
"""Summarize a seat-cost rig run. See README.md.

Usage: analyze.py <run-dir> [--json]

Reads samples.jsonl (sample.py) and phases.jsonl (drive.sh) and prints one
row per measured point: totals for the MG under test and per-seat values.
"""

import json
import statistics
import sys

CLK_TCK = 100


def load(path):
    """Concatenated JSON values; drive.sh may embed pretty-printed ones."""
    with open(path) as f:
        text = f.read()
    decoder = json.JSONDecoder()
    values, index = [], 0
    while True:
        while index < len(text) and text[index].isspace():
            index += 1
        if index >= len(text):
            return values
        value, index = decoder.raw_decode(text, index)
        values.append(value)


def window(samples, start, end):
    inside = [s for s in samples if start <= s["ts"] <= end]
    return inside


def with_metrics(samples):
    return [s for s in samples if s.get("metrics")]


def metric_sum(metrics, prefix, **labels):
    total = 0.0
    for key, value in metrics.items():
        if not key.startswith(prefix):
            continue
        if all(f'{k}="{v}"' in key for k, v in labels.items()):
            total += value
    return total


def nft(counters):
    return {
        c["counter"]["name"]: c["counter"]["bytes"]
        for c in counters["nftables"]
        if "counter" in c
    }


def summarize(samples, phase):
    start, end = phase["start"], phase["end"]
    inside = window(samples, start, end)
    s0, s1 = inside[0], inside[-1]
    dt = s1["ts"] - s0["ts"]
    n = phase["federations"]
    row = {"N": n, "ops_per_sec_per_fed": phase["ops_per_sec"], "seconds": round(dt)}
    row["cpus"] = s1.get("cpus_allowed")

    # CPU: the cgroup total, split into the daemon and its seats by process.
    cg = lambda s, k: s["cgroup"]["cpu_stat"][k]
    row["cpu_cores_total"] = (cg(s1, "usage_usec") - cg(s0, "usage_usec")) / 1e6 / dt
    procs0 = {p["pid"]: p for p in s0["processes"]}
    seat_cpu = daemon_cpu = 0.0
    writes = 0
    for p in s1["processes"]:
        before = procs0.get(p["pid"])
        if before is None:
            continue
        cpu = (p["cpu_us"] - before["cpu_us"]) / 1e6 / dt
        written = (p["write_bytes"] - p.get("cancelled_write_bytes", 0)) - (
            before["write_bytes"] - before.get("cancelled_write_bytes", 0)
        )
        writes += written
        if p["seat"] is None:
            daemon_cpu += cpu
        else:
            seat_cpu += cpu
    seats = [p for p in s1["processes"] if p["seat"] is not None]
    row["seats_running"] = len(seats)
    row["cpu_cores_daemon"] = daemon_cpu
    row["cpu_cores_per_seat"] = seat_cpu / max(1, len(seats))
    row["disk_write_kib_s_total"] = writes / dt / 1024
    row["disk_write_kib_s_per_seat"] = row["disk_write_kib_s_total"] / max(1, n)

    # Memory: means over the window.
    def mean(values):
        values = list(values)
        return sum(values) / len(values) if values else 0.0

    row["rss_mib_total"] = mean(sum(p["rss"] for p in s["processes"]) for s in inside) / 2**20
    row["pss_mib_total"] = mean(sum(p.get("pss", 0) for p in s["processes"]) for s in inside) / 2**20
    row["rss_mib_daemon"] = mean(
        sum(p["rss"] for p in s["processes"] if p["seat"] is None) for s in inside
    ) / 2**20
    row["rss_mib_per_seat"] = mean(
        mean(p["rss"] for p in s["processes"] if p["seat"] is not None) for s in inside
    ) / 2**20
    row["cgroup_memory_mib"] = mean(s["cgroup"]["memory_current"] for s in inside) / 2**20
    row["cgroup_anon_mib"] = mean(s["cgroup"]["memory_stat"].get("anon", 0) for s in inside) / 2**20

    # Pressure, as a percentage of wall time.
    for resource in ("cpu", "io"):
        for kind in ("some", "full"):
            p0 = s0["cgroup"][f"{resource}_pressure"].get(kind, 0)
            p1 = s1["cgroup"][f"{resource}_pressure"].get(kind, 0)
            row[f"{resource}_psi_{kind}_pct"] = (p1 - p0) / 1e6 / dt * 100

    # Other work on the measured CPUs: busy CPU time minus our cgroup's.
    busy = 0
    for cpu, values in s1["cpu_lines"].items():
        before = s0["cpu_lines"].get(cpu)
        if before:
            delta = [a - b for a, b in zip(values, before)]
            busy += sum(delta) - delta[3] - delta[4]
    row["cpu_cores_foreign_on_cpuset"] = busy / CLK_TCK / dt - row["cpu_cores_total"]

    # Network: veth totals and nft transport split.
    v0, v1 = s0["net"]["vmut"], s1["net"]["vmut"]
    row["net_rx_kib_s_total"] = (v1["rx_bytes"] - v0["rx_bytes"]) / dt / 1024
    row["net_tx_kib_s_total"] = (v1["tx_bytes"] - v0["tx_bytes"]) / dt / 1024
    row["net_kib_s_per_seat"] = (row["net_rx_kib_s_total"] + row["net_tx_kib_s_total"]) / max(1, n)
    ndt = phase["end"] - phase["start"]
    c0, c1 = nft(phase["nft_before"]), nft(phase["nft_after"])
    for name in sorted(c1):
        row[f"nft_{name}_kib_s"] = (c1[name] - c0.get(name, 0)) / ndt / 1024

    # Seat metrics from fedimintd: p2p bytes, peers, sessions, transactions.
    m = with_metrics(inside)
    m0, m1 = m[0], m[-1]
    mdt = m1["ts"] - m0["ts"]
    p2p_in = p2p_out = 0.0
    peers = []
    sessions = []
    ordering = []
    txs = 0.0
    api_conns = []
    for seat, after in m1["metrics"].items():
        before = m0["metrics"].get(seat)
        if not before or "error" in after or "error" in before:
            continue
        p2p_in += metric_sum(after, "fm_peer_transport_bytes_total", direction="incoming") - metric_sum(
            before, "fm_peer_transport_bytes_total", direction="incoming"
        )
        p2p_out += metric_sum(after, "fm_peer_transport_bytes_total", direction="outgoing") - metric_sum(
            before, "fm_peer_transport_bytes_total", direction="outgoing"
        )
        active = set()
        for key, value in after.items():
            if key.startswith("fm_peer_messages_total") and 'direction="incoming"' in key:
                if value > before.get(key, 0):
                    active.add(key.split('peer_id="')[1].split('"')[0])
        peers.append(len(active))
        completed = after.get("fm_consensus_session_count", 0) - before.get("fm_consensus_session_count", 0)
        if completed > 0:
            sessions.append(mdt / completed)
        count = after.get("fm_consensus_ordering_latency_seconds_count", 0) - before.get(
            "fm_consensus_ordering_latency_seconds_count", 0
        )
        if count > 0:
            ordering.append(
                (after["fm_consensus_ordering_latency_seconds_sum"] - before["fm_consensus_ordering_latency_seconds_sum"])
                / count
            )
        txs += after.get("fm_consensus_tx_processed_inputs_count", 0) - before.get(
            "fm_consensus_tx_processed_inputs_count", 0
        )
        api_conns.append(after.get("fm_iroh_api_connections_active", 0))
    row["p2p_kib_s_per_seat"] = (p2p_in + p2p_out) / mdt / 1024 / max(1, n)
    row["p2p_peers_per_seat"] = statistics.median(peers) if peers else None
    row["p2p_connections_total"] = sum(peers)
    row["api_connections_per_seat"] = statistics.mean(api_conns) if api_conns else None
    row["session_secs_median"] = statistics.median(sessions) if sessions else None
    row["ordering_latency_ms_mean"] = statistics.mean(ordering) * 1000 if ordering else None
    row["fed_tx_per_sec_per_fed"] = txs / mdt / max(1, n)

    # Achieved user operations.
    lb = phase["load_before"]["federations"]
    la = phase["load_after"]["federations"]
    ok = err = skipped = 0
    for before, after in zip(lb, la):
        ok += (after["ecash_ok"] + after["ln_ok"]) - (before["ecash_ok"] + before["ln_ok"])
        err += (after["ecash_err"] + after["ln_err"]) - (before["ecash_err"] + before["ln_err"])
        skipped += after["skipped_busy"] - before["skipped_busy"]
    row["user_ops_per_sec_per_fed"] = ok / ndt / max(1, n)
    row["user_op_errors"] = err
    row["user_ops_skipped_busy"] = skipped
    return row


def main():
    run_dir = sys.argv[1]
    samples = load(f"{run_dir}/samples.jsonl")
    phases = load(f"{run_dir}/phases.jsonl")
    rows = [summarize(samples, phase) for phase in phases]
    if "--json" in sys.argv:
        print(json.dumps(rows, indent=1))
        return
    columns = [
        ("N", "N"), ("ops_per_sec_per_fed", "ops/s/fed"), ("cpus", "cpus"),
        ("cpu_cores_total", "cores"), ("cpu_cores_per_seat", "cores/seat"), ("cpu_cores_daemon", "mg cores"),
        ("rss_mib_total", "RSS MiB"), ("rss_mib_per_seat", "RSS/seat"), ("pss_mib_total", "PSS MiB"),
        ("disk_write_kib_s_total", "wr KiB/s"), ("disk_write_kib_s_per_seat", "wr/seat"),
        ("cpu_psi_some_pct", "cpu some%"), ("cpu_psi_full_pct", "cpu full%"),
        ("io_psi_some_pct", "io some%"), ("io_psi_full_pct", "io full%"),
        ("net_rx_kib_s_total", "rx KiB/s"), ("net_tx_kib_s_total", "tx KiB/s"), ("net_kib_s_per_seat", "net/seat"),
        ("p2p_kib_s_per_seat", "p2p/seat"), ("p2p_connections_total", "p2p conns"),
        ("session_secs_median", "session s"), ("fed_tx_per_sec_per_fed", "fed tx/s"),
        ("user_ops_per_sec_per_fed", "ops ok/s"), ("cpu_cores_foreign_on_cpuset", "foreign"),
    ]
    print("| " + " | ".join(title for _, title in columns) + " |")
    print("|" + "---|" * len(columns))
    for row in rows:
        cells = []
        for key, _ in columns:
            value = row.get(key)
            if isinstance(value, float):
                cells.append(f"{value:.3g}" if abs(value) < 1000 else f"{value:.0f}")
            else:
                cells.append(str(value))
        print("| " + " | ".join(cells) + " |")


if __name__ == "__main__":
    main()
