#!/usr/bin/env python3
"""Sample the MG under test of the seat-cost rig. See README.md.

Run inside the measured network namespace (it scrapes fedimintd metrics on
loopback and reads that namespace's /proc/net/dev):

    nsenter --user --net=<run-dir>/mut-netns -- python3 sample.py <run-dir> <out.jsonl>

Every INTERVAL seconds it appends one JSON line with cumulative counters:
per-process CPU ticks, memory, and storage I/O for every process in the
measured cgroup; the cgroup's cpu.stat, PSI files, and memory; the veth
counters; and per-CPU /proc/stat lines. Every METRICS_EVERY samples it also
scrapes each seat's fedimintd metrics endpoint. Rates come from deltas
between lines. The process and cgroup sources and their parsing follow
crates/mg/bin/src/resource_report.rs.
"""

import json
import os
import re
import sys
import time
import urllib.request

INTERVAL = float(os.environ.get("SEAT_SIM_SAMPLE_INTERVAL", "5"))
METRICS_EVERY = int(os.environ.get("SEAT_SIM_METRICS_EVERY", "2"))
SEAT_BLOCK = 4
METRIC_PREFIXES = (
    "fm_peer_transport_bytes_total",
    "fm_peer_connect_total",
    "fm_peer_disconnect_total",
    "fm_peer_messages_total",
    "fm_peer_transport_lost_packets_total",
    "fm_consensus_session_count",
    "fm_consensus_sessions_completed_total",
    "fm_consensus_ordering_latency_seconds_sum",
    "fm_consensus_ordering_latency_seconds_count",
    "fm_consensus_items_processed_total",
    "fm_consensus_tx_processed_inputs_count",
    "fm_iroh_api_connections_active",
    "fm_iroh_api_request_duration_seconds_count",
    "fm_iroh_api_request_duration_seconds_sum",
    "fm_jsonrpc_api_request_duration_seconds_count",
)
CLK_TCK = os.sysconf("SC_CLK_TCK")


def read(path):
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return None


def field(text, name, sep):
    for line in text.splitlines():
        if line.startswith(name + sep):
            return line[len(name) + 1 :].strip()
    return None


def kib(value):
    return int(value.split()[0]) * 1024


def process(pid):
    stat = read(f"/proc/{pid}/stat")
    status = read(f"/proc/{pid}/status")
    if stat is None or status is None:
        return None
    after = stat.rsplit(")", 1)[1].split()
    io = read(f"/proc/{pid}/io") or ""
    rollup = read(f"/proc/{pid}/smaps_rollup") or ""
    cmdline = (read(f"/proc/{pid}/cmdline") or "").split("\0")
    seat = None
    for index, arg in enumerate(cmdline):
        if arg == "--data-dir" and index + 1 < len(cmdline):
            match = re.search(r"/seats/(\d+)/data$", cmdline[index + 1])
            if match:
                seat = int(match.group(1))
    sample = {
        "pid": pid,
        "comm": stat.split("(", 1)[1].rsplit(")", 1)[0],
        "seat": seat,
        "cpu_us": (int(after[11]) + int(after[12])) * 1_000_000 // CLK_TCK,
        "rss": kib(field(status, "VmRSS", ":")),
        "anon": kib(field(status, "RssAnon", ":")),
        "threads": int(field(status, "Threads", ":")),
    }
    pss = field(rollup, "Pss", ":")
    if pss:
        sample["pss"] = kib(pss)
    for name in ("read_bytes", "write_bytes", "cancelled_write_bytes", "syscw", "wchar"):
        value = field(io, name, ":")
        if value is not None:
            sample[name] = int(value)
    return sample


def pressure(text):
    if text is None:
        return None
    out = {}
    for line in text.splitlines():
        kind, *pairs = line.split()
        for pair in pairs:
            if pair.startswith("total="):
                out[kind] = int(pair[6:])
    return out


def keyed(text, sep=" "):
    out = {}
    for line in (text or "").splitlines():
        parts = line.split(sep, 1)
        if len(parts) == 2:
            try:
                out[parts[0]] = int(parts[1])
            except ValueError:
                pass
    return out


def net_dev():
    out = {}
    for line in (read("/proc/net/dev") or "").splitlines()[2:]:
        name, data = line.split(":", 1)
        values = [int(v) for v in data.split()]
        out[name.strip()] = {
            "rx_bytes": values[0],
            "rx_packets": values[1],
            "tx_bytes": values[8],
            "tx_packets": values[9],
        }
    return out


def cpu_lines(cpus):
    out = {}
    for line in (read("/proc/stat") or "").splitlines():
        name, *values = line.split()
        if name.startswith("cpu") and name[3:].isdigit() and int(name[3:]) in cpus:
            # user nice system idle iowait irq softirq steal
            out[name] = [int(v) for v in values[:8]]
    return out


def scrape(port):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/metrics", timeout=3) as response:
            text = response.read().decode()
    except OSError as error:
        return {"error": str(error)}
    out = {}
    for line in text.splitlines():
        if line.startswith(METRIC_PREFIXES):
            key, _, value = line.rpartition(" ")
            try:
                out[key] = float(value)
            except ValueError:
                pass
    return out


def main():
    run_dir, out_path = sys.argv[1], sys.argv[2]
    cgroup = read(os.path.join(run_dir, "cgroup")).strip()
    mut = os.path.join(cgroup, "mut")
    count = 0
    with open(out_path, "a") as out:
        while True:
            started = time.time()
            pids = [int(p) for p in (read(os.path.join(mut, "cgroup.procs")) or "").split()]
            processes = [p for p in map(process, pids) if p]
            # The CPU set can change during a run (taskset on every process).
            allowed = None
            cpus = set()
            if pids:
                allowed = read(f"/proc/{pids[0]}/status")
                allowed = field(allowed, "Cpus_allowed_list", ":") if allowed else None
                if allowed:
                    for part in allowed.split(","):
                        lo, _, hi = part.partition("-")
                        cpus.update(range(int(lo), int(hi or lo) + 1))
            sample = {
                "ts": started,
                "processes": processes,
                "cgroup": {
                    "cpu_stat": keyed(read(os.path.join(mut, "cpu.stat"))),
                    "cpu_pressure": pressure(read(os.path.join(mut, "cpu.pressure"))),
                    "io_pressure": pressure(read(os.path.join(mut, "io.pressure"))),
                    "memory_pressure": pressure(read(os.path.join(mut, "memory.pressure"))),
                    "memory_current": int(read(os.path.join(mut, "memory.current")) or 0),
                    "memory_stat": {
                        k: v
                        for k, v in keyed(read(os.path.join(mut, "memory.stat"))).items()
                        if k in ("anon", "file", "file_dirty", "file_writeback", "kernel", "sock", "shmem")
                    },
                },
                "net": net_dev(),
                "cpus_allowed": allowed,
                "cpu_lines": cpu_lines(cpus),
            }
            if count % METRICS_EVERY == 0:
                state = read(os.path.join(run_dir, "state.json"))
                if state:
                    port_base = json.loads(state)["port_base"]
                    seats = sorted(p["seat"] for p in processes if p["seat"] is not None)
                    sample["metrics"] = {
                        str(seat): scrape(port_base + seat * SEAT_BLOCK + 3) for seat in seats
                    }
            out.write(json.dumps(sample) + "\n")
            out.flush()
            count += 1
            time.sleep(max(0.0, INTERVAL - (time.time() - started)))


if __name__ == "__main__":
    main()
