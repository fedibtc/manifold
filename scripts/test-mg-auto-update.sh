#!/usr/bin/env bash
# Release qualification for automatic MG updates, intentionally separate from
# routine SelfCI. Seven MGs built from this checkout form a federation, then
# install the given newer static release from a local HTTP server.
#
# Build the release from a checkout whose workspace version is higher, e.g.:
#   nix build .#fleet-manager-static -o result-next
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "Usage: nix develop --command $0 <newer-static-fleet-manager> <its-version>" >&2
  exit 2
fi
new_bin=$(realpath "$1")
new_version=$2

root=$(git rev-parse --show-toplevel)
cd "$root"
cargo build --locked -p mg -p defe -p fa-cli -p mg-cli
cargo build --locked -p devmon --bin manifold-test-issuer
bin_dir=$(realpath "${CARGO_TARGET_DIR:-target}/debug")

export MG_E2E=1
export MG_E2E_LOCAL_IROH=1
export MG_E2E_MANIFOLD_GUARDIAN_BIN="$bin_dir/fleet-manager"
export MG_E2E_FA_CLI_BIN="$bin_dir/fa-cli"
export MG_E2E_MG_CLI_BIN="$bin_dir/fman-cli"
export MG_AUTO_UPDATE_TO_BIN="$new_bin"
export MG_AUTO_UPDATE_TO_VERSION="$new_version"
export MG_E2E_FEDIMINT_CLI_BIN
MG_E2E_FEDIMINT_CLI_BIN=$(command -v fedimint-cli)
export MG_E2E_BITCOIN_CLI_BIN
MG_E2E_BITCOIN_CLI_BIN=$(command -v bitcoin-cli)
export FLIP_E2E_GATEWAY_CLI_BIN
FLIP_E2E_GATEWAY_CLI_BIN=$(command -v gateway-cli)

"$bin_dir/defe" --binary-path "$bin_dir" \
  --gatewayd-bin "$(command -v gatewayd)" \
  --gateway-cli-bin "$FLIP_E2E_GATEWAY_CLI_BIN" \
  exec cargo test --locked -p tests-e2e --test manifold_guardian_0_1_formation \
  mg_auto_updates_a_formed_federation_under_defe \
  -- --ignored --exact --nocapture
