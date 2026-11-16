#!/usr/bin/env bash
# Inside the rig's user and network namespace: give the MG under test its own
# network namespace behind a veth pair, then run the rig under a one-shot
# defe server. Started by launch.sh.
set -euo pipefail
run_dir=$SEAT_SIM_RUN_DIR

ip link set lo up
# Holder process for the measured namespace; link its namespace so the MG
# wrapper and the sampler can enter it by path.
unshare -n sleep infinity &
holder=$!
sleep 0.2
echo "$holder" > "$run_dir/mut-netns-holder.pid"
ln -sfn "/proc/$holder/ns/net" "$run_dir/mut-netns"

ip link add vrig type veth peer name vmut
ip link set vmut netns "$holder"
ip addr add 10.77.0.1/24 dev vrig
ip link set vrig up
nsenter --net="/proc/$holder/ns/net" sh -euc '
  ip link set lo up
  ip addr add 10.77.0.2/24 dev vmut
  ip link set vmut up
  ip route add default via 10.77.0.1
  # Byte accounting by transport. Rules without verdicts only count.
  nft -f - <<EOF
table inet acct {
  counter udp_in {}
  counter udp_out {}
  counter tcp_in {}
  counter tcp_out {}
  counter other_in {}
  counter other_out {}
  chain input {
    type filter hook input priority 0; policy accept;
    iifname "lo" accept
    meta l4proto udp counter name udp_in accept
    meta l4proto tcp counter name tcp_in accept
    counter name other_in
  }
  chain output {
    type filter hook output priority 0; policy accept;
    oifname "lo" accept
    meta l4proto udp counter name udp_out accept
    meta l4proto tcp counter name tcp_out accept
    counter name other_out
  }
}
EOF
'

# bitcoind and the relay bind loopback and admit only loopback peers; the MG
# under test reaches them at the rig-side veth address, translated to
# loopback on both ends.
sysctl -qw net.ipv4.conf.vrig.route_localnet=1
nft -f - <<EOF
table ip seatsim {
  chain pre {
    type nat hook prerouting priority dstnat;
    iifname "vrig" ip daddr 10.77.0.1 meta l4proto tcp dnat to 127.0.0.1
  }
  chain in {
    type nat hook input priority 100;
    ct status dnat ip daddr 127.0.0.1 snat to 127.0.0.1
  }
}
EOF

export DEV_DEFE_PORTALLOC_DATA_DIR="$run_dir/portalloc"
export SEAT_SIM_DIR="$run_dir"
export SEAT_SIM_MUT_MG_BIN="$run_dir/bin/seat-sim-mut-mg"
export SEAT_SIM_HELPER_MG_BIN="$run_dir/bin/seat-sim-helper-mg"
export MG_E2E_MG_CLI_BIN="$SEAT_SIM_RELEASE_DIR/fman-cli"
export MG_E2E_FA_CLI_BIN="$SEAT_SIM_RELEASE_DIR/fa-cli"
export MG_E2E_BITCOIN_CLI_BIN="$(command -v bitcoin-cli)"
export FLIP_E2E_GATEWAY_CLI_BIN="$(command -v gateway-cli)"
export SEAT_SIM_GATEWAYD_BIN="$(command -v gatewayd)"
cd "$run_dir"
trap 'kill "$holder" 2>/dev/null || true' EXIT
"$SEAT_SIM_RELEASE_DIR/defe" --binary-path "$SEAT_SIM_RELEASE_DIR" --tmp-dir "$run_dir/defe" \
  exec "$SEAT_SIM_TEST_BIN" --exact seat_cost_rig::seat_cost_rig --nocapture
