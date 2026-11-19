#!/usr/bin/env bash
# Set or clear a WAN-like one-way delay on the rig's guardian traffic.
# See README.md.
#
# Usage: netem.sh <run-dir> <delay-ms|off>
#
# Delays every UDP (iroh QUIC) packet on the rig's loopback, which the
# helper MGs and load clients share, and on both ends of the MUT's veth, so
# every guardian-to-guardian and client-to-guardian path gets the same
# one-way delay. TCP (bitcoind RPC, the relay, the gateway API) is not
# delayed. drive.sh clears it while federations form, because fa-cli's
# selection and formation timeouts assume a LAN.
set -euo pipefail
run_dir=$(realpath "$1")
delay=$2
rig=$(cat "$run_dir/rig-netns.pid")
mut=$(cat "$run_dir/mut-netns-holder.pid")

apply() {
  local pid=$1 dev=$2
  local enter=(nsenter -t "$pid" -U -n --preserve-credentials --)
  if [ "$delay" = off ]; then
    "${enter[@]}" tc qdisc del dev "$dev" root 2>/dev/null || true
    return
  fi
  "${enter[@]}" tc qdisc replace dev "$dev" root handle 1: prio bands 3 priomap 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1
  "${enter[@]}" tc qdisc replace dev "$dev" parent 1:3 handle 30: netem delay "${delay}ms" limit 1000000
  "${enter[@]}" tc filter replace dev "$dev" parent 1: protocol ip prio 1 u32 match ip protocol 17 0xff flowid 1:3
}
apply "$rig" lo
apply "$rig" vrig
apply "$mut" vmut
