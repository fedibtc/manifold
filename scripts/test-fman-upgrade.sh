#!/usr/bin/env bash
# Release qualification, intentionally separate from routine SelfCI.
set -euo pipefail

if [ "$#" -ne 2 ] || [[ ! "$1" =~ ^[0-9a-f]{40}$ ]] || [[ ! "$2" =~ ^[0-9a-f]{40}$ ]]; then
  echo "Usage: nix develop --command $0 <old-image-commit> <new-image-commit>" >&2
  exit 2
fi
if [ "$1" = "$2" ]; then
  echo "Old and new image commits must differ" >&2
  exit 2
fi

root=$(git rev-parse --show-toplevel)
cd "$root"
artifacts=$(mktemp -d "${TMPDIR:-/tmp}/fman-upgrade-bins.XXXXXX")
container=
cleanup() {
  if [ -n "$container" ]; then docker rm -v "$container" >/dev/null; fi
  rm -rf "$artifacts"
}
trap cleanup EXIT
for release in "$1" "$2"; do
  image="ghcr.io/fedibtc/manifold-fman:$release"
  docker pull "$image"
  docker image inspect "$image" --format '{{json .RepoDigests}} {{json .Config.Labels}}'
  container=$(docker create "$image")
  docker cp -L "$container:/bin/fleet-manager" "$artifacts/$release"
  docker rm -v "$container" >/dev/null
  container=
  # The Nix dev shell supplies runtime libraries. Fail before creating test
  # state if this host cannot execute the actual image binary.
  "$artifacts/$release" --help >/dev/null
done

cargo build --locked -p defe -p fi-cli -p fman-cli
cargo build --locked -p devmon --bin manifold-test-issuer
bin_dir="${CARGO_TARGET_DIR:-target}/debug"
bin_dir=$(realpath "$bin_dir")
ln -s "$bin_dir/manifold-test-issuer" "$artifacts/manifold-test-issuer"
export FMAN_E2E=1
export FMAN_UPGRADE_FROM_BIN="$artifacts/$1"
export FMAN_E2E_FLEET_MANAGER_BIN="$artifacts/$2"
export FMAN_E2E_FI_CLI_BIN="$bin_dir/fi-cli"
export FMAN_E2E_FMAN_CLI_BIN="$bin_dir/fman-cli"
export FMAN_E2E_FEDIMINT_CLI_BIN
FMAN_E2E_FEDIMINT_CLI_BIN=$(command -v fedimint-cli)
export FMAN_E2E_BITCOIN_CLI_BIN
FMAN_E2E_BITCOIN_CLI_BIN=$(command -v bitcoin-cli)
export FLIP_E2E_GATEWAY_CLI_BIN
FLIP_E2E_GATEWAY_CLI_BIN=$(command -v gateway-cli)

"$bin_dir/defe" --binary-path "$bin_dir" \
  --gatewayd-bin "$(command -v gatewayd)" \
  --gateway-cli-bin "$FLIP_E2E_GATEWAY_CLI_BIN" \
  exec cargo test --locked -p tests-e2e --test fleet_manager_0_1_formation \
  fman_upgrades_existing_guardians_wallet_and_pending_payout_under_defe \
  -- --ignored --exact --nocapture
