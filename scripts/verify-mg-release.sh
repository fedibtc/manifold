#!/usr/bin/env bash
# Rebuild every binary of an MG release from its source commit and check that
# each one matches the published hash and size. Run this before signing the
# release with `setup-payment-publisher sign-mg-release`.
#
# Usage: scripts/verify-mg-release.sh <mg-release.json>
set -euo pipefail

release=${1:?usage: $0 <mg-release.json>}
jq=(nix run nixpkgs#jq --)
commit=$("${jq[@]}" -r .source_commit "$release")
flake="github:fedibtc/manifold/$commit"

status=0
for arch in $("${jq[@]}" -r '.binaries | keys[]' "$release"); do
  expected_sha=$("${jq[@]}" -r ".binaries.\"$arch\".sha256" "$release")
  expected_size=$("${jq[@]}" -r ".binaries.\"$arch\".size" "$release")
  out=$(nix build --accept-flake-config --no-link --print-out-paths "$flake#fleet-manager-static-$arch")
  sha=$(sha256sum "$out/bin/fleet-manager" | cut -d' ' -f1)
  size=$(stat -c %s "$out/bin/fleet-manager")
  if [ "$sha" = "$expected_sha" ] && [ "$size" = "$expected_size" ]; then
    echo "OK: $arch $sha"
  else
    echo "MISMATCH: $arch built $sha ($size bytes), release says $expected_sha ($expected_size bytes)" >&2
    status=1
  fi
done
exit "$status"
