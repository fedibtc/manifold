#!/usr/bin/env bash
# Run the full test suite -- including the FMAN_E2E / FLIP_E2E tiers --
# impurely against the working tree (issue #106).
#
# This mirrors what the `.#ci.<system>.tests` derivation does, with two
# deliberate differences:
#   * workspace binaries (defe, push-gateway, fleet-manager, fman-cli, fi-cli) come
#     from the incremental cargo target dir instead of Nix store paths, so
#     an edit->test cycle reuses prior compilation;
#   * the tree is used as-is (untracked files included) -- no git-filtered
#     source snapshot, no commit required.
#
# Service binaries (fedimintd, fedimint-cli, bitcoin-cli, gatewayd,
# gateway-cli, esplora, bitcoind, nostr-rs-relay) come from the dev shell;
# run this under `nix develop`. The `.#ci.<system>.tests` derivation remains
# the pure merge-authority in CI.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! command -v cargo-nextest >/dev/null && ! cargo nextest --version >/dev/null 2>&1; then
  echo "error: cargo-nextest not found; run inside 'nix develop'" >&2
  exit 1
fi

# The git-dependency link farm the workspace build expects (a no-op when the
# dev shellHook already ran it).
if command -v link-external-deps >/dev/null; then
  link-external-deps "$PWD"
fi

profile="${CARGO_PROFILE:-debug}"
build_args=()
case "$profile" in
  debug | dev) profile_dir=debug ;;
  *)
    build_args+=(--profile "$profile")
    profile_dir="$profile"
    ;;
esac
# Absolute: nextest runs each test with the package dir as CWD, so a
# relative target path would dangle from crates/*/.
target_dir="${CARGO_TARGET_DIR:-target}"
case "$target_dir" in
  /*) ;;
  *) target_dir="$PWD/$target_dir" ;;
esac
bin_dir="$target_dir/$profile_dir"

# The binaries defe spawns as resources plus the ones the e2e tests spawn
# directly. nextest builds the test binaries themselves.
cargo build --locked "${build_args[@]}" \
  -p defe \
  -p fedi-decentralized-push-gateway \
  -p fman \
  -p fman-cli \
  -p fi-cli \
  -p devmon

# Resolve a service binary from the dev shell PATH, failing with a teaching
# error instead of letting a test die on a missing binary mid-run.
require_on_path() {
  command -v "$1" || {
    echo "error: '$1' not on PATH; run inside 'nix develop' (dev shell provides it)" >&2
    exit 1
  }
}

FEDIMINTD_BIN=$(require_on_path fedimintd)
FEDIMINT_CLI_BIN=$(require_on_path fedimint-cli)
BITCOIN_CLI_BIN=$(require_on_path bitcoin-cli)
GATEWAYD_BIN=$(require_on_path gatewayd)
GATEWAY_CLI_BIN=$(require_on_path gateway-cli)
ESPLORA_BIN=$(require_on_path esplora)

# Same env contract as the `.#ci.<system>.tests` check phase. Explicit paths
# (rather than the tests' PATH fallback) so a missing binary fails loudly
# with the variable name in the message.
export DEV_DEFE_PORTALLOC_DATA_DIR="${DEV_DEFE_PORTALLOC_DATA_DIR:-${TMPDIR:-/tmp}/defe-portalloc}"
mkdir -p "$DEV_DEFE_PORTALLOC_DATA_DIR"
export FMAN_E2E=1
export FMAN_E2E_FLEET_MANAGER_BIN="$bin_dir/fleet-manager"
export FMAN_E2E_FMAN_CLI_BIN="$bin_dir/fman-cli"
export FMAN_E2E_FI_CLI_BIN="$bin_dir/fi-cli"
export FMAN_E2E_FEDIMINT_CLI_BIN="$FEDIMINT_CLI_BIN"
export FMAN_E2E_BITCOIN_CLI_BIN="$BITCOIN_CLI_BIN"
export FMAN_E2E_ESPLORA_BIN="$ESPLORA_BIN"
export FLIP_E2E_GATEWAYD_BIN="$GATEWAYD_BIN"
export FLIP_E2E_GATEWAY_CLI_BIN="$GATEWAY_CLI_BIN"
export FLIP_E2E_FEDIMINTD_BIN="$FEDIMINTD_BIN"
export FLIP_E2E_FEDIMINT_CLI_BIN="$FEDIMINT_CLI_BIN"
export FLIP_E2E_BITCOIN_CLI_BIN="$BITCOIN_CLI_BIN"

nextest_args=(
  nextest run
  --features fedi-decentralized-cloud-fman-telemetry/defe-test-support
)
if [ -n "${CARGO_PROFILE:-}" ]; then
  nextest_args+=(--cargo-profile "$CARGO_PROFILE" --profile "$CARGO_PROFILE")
fi
nextest_args+=(--workspace "$@")

# One-shot defe server around the run, exactly like CI -- independent of any
# persistent `just defe-serve` instance. Extra arguments are forwarded to
# nextest (e.g. `-E 'binary(=integration_daemon_smoke)'` for a subset).
exec "$bin_dir/defe" \
  --binary-path "$bin_dir" \
  --gatewayd-bin "$GATEWAYD_BIN" \
  --gateway-cli-bin "$GATEWAY_CLI_BIN" \
  exec cargo "${nextest_args[@]}"
