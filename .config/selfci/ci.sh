#!/usr/bin/env bash
set -eou pipefail

# Before adding routine work here, read
# specs/GATE-selfci-local-development-cost.md.
function job_lint() {
  selfci step start "cargo fmt"
  if ! nix build --accept-flake-config -L .#ci.cargoFmt ; then
    selfci step fail
  fi

  selfci step start "treefmt"
  if ! nix build --accept-flake-config -L .#treefmt ; then
    selfci step fail
  fi

  selfci step start "publish image loader"
  if ! .github/scripts/test-load-single-docker-image ; then
    selfci step fail
  fi

}

function job_cargo() {
  selfci step start "Cargo.lock up-to-date"
  link-external-deps "$PWD"
  if ! cargo update --workspace --locked -q; then
    selfci step fail
  fi

  # Submit independent non-test checks together so Nix can share and schedule
  # their common CI-profile build graph.
  local system
  system=$(nix eval --raw --impure --expr builtins.currentSystem)
  local tests_target=".#ci.${system}.tests"
  local targets=(
    ".#ci.${system}.clippy"
    ".#ci.${system}.fleetManagerReleaseSync"
    ".#ci.${system}.cargoDependencyHygiene"
    ".#ci.${system}.leanProofs"
  )
  case "$system" in
    *-linux)
      targets+=(
        ".#ci.${system}.pushGatewayOciImage"
        ".#ci.${system}.fleetManagerCliContract"
        ".#ci.${system}.fleetManagerOciImage"
        ".#ci.${system}.liquidityManagerOciImage"
      )
      ;;
  esac

  selfci step start "nix cargo checks"
  case "$system" in
    *-darwin)
      # Darwin has no Nix network namespaces and service-heavy E2E runners
      # share the host's CPU and loopback. Finish compilation/static checks
      # before starting the already-serialized test graph so protocol deadlines
      # measure the services rather than contention with concurrent rustc jobs.
      if ! nix build --accept-flake-config -L --no-link --max-jobs 16 "${targets[@]}" ; then
        selfci step fail
      fi
      if ! nix build --accept-flake-config -L --no-link --max-jobs 16 "$tests_target" ; then
        selfci step fail
      fi
      ;;
    *)
      targets+=("$tests_target")
      if ! nix build --accept-flake-config -L --no-link --max-jobs 16 "${targets[@]}" ; then
        selfci step fail
      fi
      ;;
  esac

}

case "$SELFCI_JOB_NAME" in
  main)
    selfci job start "lint"
    selfci job start "cargo"
    ;;
  cargo)
    job_cargo
    ;;
  lint)
    job_lint
    ;;
  *)
    echo "Unknown job: $SELFCI_JOB_NAME"
    exit 1
esac
