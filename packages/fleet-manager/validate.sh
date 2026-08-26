#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/../.."

bash -n packages/fleet-manager/entrypoint.sh
bash -n packages/fleet-manager/validate.sh

grep -q -- '--manifold-environment' packages/fleet-manager/entrypoint.sh
grep -q -- '--push-gateway-origin' packages/fleet-manager/entrypoint.sh
grep -q 'FLEET_MANAGER_PUSH_GATEWAY_ORIGIN: ${FLEET_MANAGER_PUSH_GATEWAY_ORIGIN}' packages/fleet-manager/umbrel/docker-compose.yml
! grep -R -E 'FLEET_MANAGER_PUSH_GATEWAY_ORIGIN:.*(localhost|127\.0\.0\.1|\.invalid)' \
  packages/fleet-manager/umbrel

# `--option=value` makes a value beginning with `-` unambiguously an option
# value to clap. The stub receives precisely what the release entrypoint execs.
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
cat >"$tmp_dir/fleet-manager" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" >"$FLEET_MANAGER_CAPTURED_ARGV"
EOF
chmod +x "$tmp_dir/fleet-manager"
capture="$tmp_dir/argv"
PATH="$tmp_dir:$PATH" \
  FLEET_MANAGER_CAPTURED_ARGV="$capture" \
  FLEET_MANAGER_MANIFOLD_ENVIRONMENT=development \
  FLEET_MANAGER_PUSH_GATEWAY_ORIGIN=https://gateway.example \
  FLEET_MANAGER_BITCOIND_URL=http://127.0.0.1:18443 \
  FLEET_MANAGER_BITCOIND_USERNAME=operator \
  FLEET_MANAGER_BITCOIND_PASSWORD=-leading-hyphen-password \
  packages/fleet-manager/entrypoint.sh
grep -Fx -- '--bitcoind-password=-leading-hyphen-password' "$capture"
# The daemon requires the `serve` subcommand; the entrypoint must invoke it.
grep -q 'fleet-manager serve' packages/fleet-manager/entrypoint.sh
grep -q 'FLEET_MANAGER_MANIFOLD_ENVIRONMENT: production' packages/fleet-manager/umbrel/docker-compose.yml
grep -q 'dependencies:' packages/fleet-manager/umbrel/umbrel-app.yml

# The image is built with Nix, not a vendored Dockerfile. Guard against a
# Dockerfile creeping back in and assert
# the flake still exposes the image and its container-load app.
test ! -e packages/fleet-manager/Dockerfile
grep -q 'fleet-manager-oci-image' flake.nix
grep -q 'fleetManagerContainerImage' flake.nix
grep -q 'fleet-manager-container-load' flake.nix

if command -v nix >/dev/null 2>&1; then
  system=$(nix eval --raw --impure --expr builtins.currentSystem)
  # Enforce the CLI contract the image entrypoint depends on (the `serve`
  # subcommand and its flags), the fedimint release-identity synchronization,
  # and the produced OCI image's runtime contract.
  nix build --accept-flake-config -L --no-link ".#ci.${system}.fleetManagerCliContract"
  nix build --accept-flake-config -L --no-link ".#ci.${system}.fleetManagerReleaseSync"
  case "$system" in
    *-linux)
      nix build --accept-flake-config -L --no-link ".#ci.${system}.fleetManagerOciImage"
      ;;
  esac
else
  echo "nix not found; skipped fleet-manager image + CLI-contract checks" >&2
fi
