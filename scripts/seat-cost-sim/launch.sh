#!/usr/bin/env bash
# Start the seat-cost simulation rig. See README.md.
#
# Usage: launch.sh <run-dir> <mut-cpus> <rig-cpus>
#   run-dir   empty directory for control, state, logs, and MG data
#   mut-cpus  CPU list for the MG under test and its fedimintd children
#   rig-cpus  CPU list for everything else: helper MGs, bitcoind, relay,
#             gateway, and the load clients
#
# Run it from the repository's development shell after building the
# release binaries and the test binary (see README.md). It must itself run in
# a delegated systemd scope, which it creates by re-executing itself.
set -euo pipefail

run_dir=$(realpath "$1")
mut_cpus=$2
rig_cpus=$3
script_dir=$(dirname "$(realpath "$0")")

if [ -z "${SEAT_SIM_IN_SCOPE:-}" ]; then
  exec systemd-run --user --scope -p Delegate=yes --unit="seat-sim-$(basename "$run_dir")" \
    --setenv=SEAT_SIM_IN_SCOPE=1 -- env SEAT_SIM_IN_SCOPE=1 "$0" "$@"
fi

: "${SEAT_SIM_TEST_BIN:?path to the manifold_guardian_0_1_formation test binary}"
: "${SEAT_SIM_RELEASE_DIR:?directory with release fleet-manager, fman-cli, fa-cli, defe, manifold-test-issuer}"

cgroup=/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)
mkdir -p "$cgroup/rig" "$cgroup/mut"
echo $$ > "$cgroup/rig/cgroup.procs"
echo "+cpu +memory +pids" > "$cgroup/cgroup.subtree_control"

mkdir -p "$run_dir/bin" "$run_dir/logs"
real_mg="$SEAT_SIM_RELEASE_DIR/fleet-manager"
ln -sf "$SEAT_SIM_RELEASE_DIR/manifold-test-issuer" "$run_dir/bin/manifold-test-issuer"

# The MG under test moves itself into the measured cgroup, pins itself to its
# CPU set, and enters the measured network namespace. Its stderr goes through
# a pipe to a logger that stays in the rig cgroup, as journald would take it,
# so log files do not count as the MG's disk writes. It runs with the
# production log level and without defe's test-cadence switch.
cat > "$run_dir/bin/seat-sim-mut-mg" <<EOF
#!/usr/bin/env bash
exec 2> >(exec cat >> "$run_dir/logs/mut-mg.stderr")
echo \$\$ > "$cgroup/mut/cgroup.procs"
exec env -u DEV_DEFE_SOCKET_PATH -u FM_IN_DEVIMINT -u RUST_LOG -u NEXTEST \\
  MG_E2E_IROH_BIND_IP=0.0.0.0 \\
  taskset -c "$mut_cpus" nsenter --net="$run_dir/mut-netns" -- "$real_mg" "\$@"
EOF
cat > "$run_dir/bin/seat-sim-helper-mg" <<EOF
#!/usr/bin/env bash
exec env -u DEV_DEFE_SOCKET_PATH -u FM_IN_DEVIMINT -u NEXTEST \\
  MG_E2E_IROH_BIND_IP=0.0.0.0 "$real_mg" "\$@"
EOF
chmod +x "$run_dir/bin/seat-sim-mut-mg" "$run_dir/bin/seat-sim-helper-mg"

[ -f "$run_dir/control.json" ] || echo '{"federations": 0, "ops_per_sec": 0}' > "$run_dir/control.json"
echo "$cgroup" > "$run_dir/cgroup"

exec taskset -c "$rig_cpus" unshare -Urn --kill-child \
  env SEAT_SIM_RUN_DIR="$run_dir" bash "$script_dir/netns.sh"
