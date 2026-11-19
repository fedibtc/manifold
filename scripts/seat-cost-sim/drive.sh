#!/usr/bin/env bash
# Drive a running seat-cost rig through a schedule of (federations, load)
# points and record phase boundaries. See README.md.
#
# Usage: drive.sh <run-dir> "<N:ops/s list>" [warmup-secs] [measure-secs]
# Example: drive.sh run "1:0 1:0.01 1:0.1 1:1 4:0 4:1" 180 600
#
# Points run in the given order. Federations are only ever added, so list
# them by nondecreasing N.
#
# Appends one JSON line per measured point to <run-dir>/phases.jsonl with
# its start and end times and the measured namespace's nft byte counters at
# both ends.
set -euo pipefail
run_dir=$(realpath "$1")
points=$2
# Optional one-way delay applied while measuring (netem.sh), cleared while
# federations form.
delay=${SEAT_SIM_NETEM_DELAY_MS:-off}
warmup=${3:-180}
measure=${4:-600}
holder=$(cat "$run_dir/mut-netns-holder.pid")

in_mut_netns() {
  nsenter -t "$holder" -U -n --preserve-credentials -- "$@"
}

nft_counters() {
  in_mut_netns nft -j list counters table inet acct
}

# Split TCP into bitcoind RPC and Nostr relay traffic once their ports exist.
add_port_counters() {
  if in_mut_netns nft list counter inet acct btc_in >/dev/null 2>&1; then return; fi
  local btc relay
  btc=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["bitcoind_rpc_port"])' "$run_dir/state.json")
  relay=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["relay_url"].rsplit(":",1)[1].strip("/"))' "$run_dir/state.json")
  in_mut_netns nft -f - <<EOF
add counter inet acct btc_in
add counter inet acct btc_out
add counter inet acct relay_in
add counter inet acct relay_out
insert rule inet acct input tcp sport $btc counter name btc_in
insert rule inet acct output tcp dport $btc counter name btc_out
insert rule inet acct input tcp sport $relay counter name relay_in
insert rule inet acct output tcp dport $relay counter name relay_out
EOF
}

formed() {
  python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["federations"]))' "$run_dir/state.json"
}

set_control() {
  printf '{"federations": %s, "ops_per_sec": %s}\n' "$1" "$2" > "$run_dir/control.json.tmp"
  mv "$run_dir/control.json.tmp" "$run_dir/control.json"
}

while [ ! -s "$run_dir/state.json" ]; do sleep 5; done
add_port_counters

for point in $points; do
  n=${point%%:*}
  load=${point#*:}
  if [ "$(formed)" -lt "$n" ]; then
    "$(dirname "$0")/netem.sh" "$run_dir" off
    set_control "$n" 0
    while [ "$(formed)" -lt "$n" ]; do sleep 10; done
  fi
  "$(dirname "$0")/netem.sh" "$run_dir" "$delay"
  {
    set_control "$n" "$load"
    echo "$(date -Is) N=$n load=$load warming up ${warmup}s" >&2
    sleep "$warmup"
    start=$(date +%s.%N)
    before=$(nft_counters)
    load_before=$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))))' "$run_dir/load.json")
    sleep "$measure"
    end=$(date +%s.%N)
    after=$(nft_counters)
    printf '{"federations": %s, "ops_per_sec": %s, "netem_delay_ms": "%s", "start": %s, "end": %s, "nft_before": %s, "nft_after": %s, "load_before": %s, "load_after": %s}\n' \
      "$n" "$load" "$delay" "$start" "$end" "$before" "$after" "$load_before" "$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))))' "$run_dir/load.json")" >> "$run_dir/phases.jsonl"
    echo "$(date -Is) N=$n load=$load measured" >&2
  }
done
set_control "$n" 0
