#!/usr/bin/env bash
#
# Run the repository's Linux Nix CI checks locally, inside a `nixos/nix` Docker
# container. This exists so a macOS (or otherwise non-trusted) host can build the
# `x86_64-linux` / `aarch64-linux` CI derivations without a native Linux builder.
#
# Why Docker: on a host where you are not a Nix `trusted-user`, the
# `fedimint.cachix.org` binary cache is ignored, so the fedimint closure compiles
# from source (very slow) — and `selfci` / some `iroh` deps do not build on
# darwin at all. Inside the container we run as root = a trusted user, so the
# cache is honored and the closure is fetched prebuilt.
#
# Usage:
#   scripts/ci-in-docker.sh [TARGET ...]
#
# Examples:
#   scripts/ci-in-docker.sh clippy
#   scripts/ci-in-docker.sh tests
#   scripts/ci-in-docker.sh cargoFmt clippy leanProofs
#
# Targets are attribute names under `.#ci.<system>` (see `nix eval .#ci --apply
# builtins.attrNames`): cargoFmt, clippy, leanProofs, tests, ...
#
# Environment overrides:
#   MAXJOBS   nix --max-jobs      (default 2)
#   CORES     nix --cores         (default 6)
#   IMAGE     container image      (default nixos/nix:latest)
#   CI_SYSTEM force the Nix system (default: autodetected from `uname -m`)
#
# Memory: `tests` builds the `fedimintd` release binary with `lto=fat`, a single
# rustc process that needs well over 8 GiB. Give Docker Desktop >= ~24 GiB
# (Settings -> Resources -> Memory) or `tests` will be OOM-killed (exit 137).
# The lighter targets (clippy, leanProofs, cargoFmt) fit in the ~8 GiB default.
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
  targets=(clippy)
fi

# Match the container's native Linux arch so we don't pay Rosetta emulation.
case "$(uname -m)" in
  arm64 | aarch64) sys="${CI_SYSTEM:-aarch64-linux}" ;;
  x86_64 | amd64)  sys="${CI_SYSTEM:-x86_64-linux}" ;;
  *) echo "error: unsupported arch $(uname -m); set CI_SYSTEM=" >&2; exit 1 ;;
esac

if ! command -v docker >/dev/null 2>&1; then
  echo "error: docker not found on PATH" >&2; exit 1
fi
if ! docker info >/dev/null 2>&1; then
  echo "error: Docker daemon not reachable (is Docker Desktop running?)" >&2; exit 1
fi

# Named volumes persist the Nix store (prebuilt closure) and the flake/cargo
# caches across runs, so only the first invocation pays the download cost.
docker volume create defe-nix  >/dev/null
docker volume create defe-home >/dev/null

attrs=""
for t in "${targets[@]}"; do
  attrs+=" .#ci.${sys}.${t}"
done

echo ">>> building${attrs}  (image=${IMAGE:-nixos/nix:latest}, max-jobs=${MAXJOBS:-2}, cores=${CORES:-6})"

exec docker run --rm \
  -v defe-nix:/nix \
  -v defe-home:/root \
  -v "$PWD":/work \
  -e MAXJOBS="${MAXJOBS:-2}" \
  -e CORES="${CORES:-6}" \
  -e ATTRS="$attrs" \
  "${IMAGE:-nixos/nix:latest}" \
  bash -c '
set -euo pipefail
export NIX_CONFIG="experimental-features = nix-command flakes
substituters = https://cache.nixos.org https://fedimint.cachix.org
trusted-public-keys = cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY= fedimint.cachix.org-1:FpJJjy1iPVlvyv4OMiN5y9+/arFLPcnZhZVVCHCDYTs="
cd /work
# shellcheck disable=SC2086  # ATTRS is intentionally word-split into multiple attrs
nix build $ATTRS -L --no-link --print-out-paths --max-jobs "$MAXJOBS" --cores "$CORES"
'
