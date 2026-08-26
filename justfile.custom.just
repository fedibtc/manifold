# Local project-specific just recipes.

# The nix image is tagged with the workspace version, so derive it here rather
# than hardcoding a tag that silently goes stale on the next version bump.
flip_version := `sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1`
flip_image := "flip-liquidity-manager:" + flip_version
flip_image_result := "result-flip-liquidity-manager-image"
flip_data_dir := "/tmp/flip-liquidity-manager-docker-data"
flip_admin_token := "flip-local-admin-token"

# Build the FLIP Docker image tarball with Nix.
flip-docker-build:
  nix build path:.#liquidityManagerDockerImage -o "{{flip_image_result}}" --no-write-lock-file

# Load the FLIP Docker image into the local Docker image store.
flip-docker-load: flip-docker-build
  docker load --input "{{flip_image_result}}"

# Build, load, and run the FLIP Docker image locally.
flip-docker-run: flip-docker-load
  mkdir -p "{{flip_data_dir}}"
  docker run --rm \
    -e FLIP_BOOTSTRAP_ADMIN_TOKEN="${FLIP_BOOTSTRAP_ADMIN_TOKEN:-{{flip_admin_token}}}" \
    -e FLIP_MANIFOLD_ENVIRONMENT="${FLIP_MANIFOLD_ENVIRONMENT:-development}" \
    -p 127.0.0.1:8173:8173 \
    -p 127.0.0.1:8174:8174/udp \
    -v "{{flip_data_dir}}:/var/lib/flip" \
    "{{flip_image}}"

# Run the defe-backed FLIP liquidity-manager E2E suite.
flip-test-integration:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  export RUST_BACKTRACE=1
  export RUST_LOG=info
  cargo run -p defe -- exec cargo nextest run \
    -p fedi-decentralized-liquidity-manager-daemon \
    --test-threads=1 \
    -E 'binary(=integration_daemon_smoke) | binary(=integration_nostr_relay) | binary(=integration_live_liquidity)'

# Run the operator-UI Playwright suite against a real, defe-provisioned FLIP
# daemon. Extra args are forwarded to `playwright test`, e.g.
#   just test-e2e-ui-flip -- -g "degraded" --headed
test-e2e-ui-flip *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  export RUST_BACKTRACE=1
  export RUST_LOG=info
  # The bare FlipDriver needs only the daemon binary; build it and the launcher,
  # then run the launcher under a one-shot defe that resolves the daemon here.
  cargo build -p fedi-decentralized-liquidity-manager-daemon -p flip-ui-e2e-runner
  bin_dir="${CARGO_TARGET_DIR:-target}/debug"
  cargo run -p defe -- --binary-path "$bin_dir" exec "$bin_dir/flip-ui-e2e-runner" {{ARGS}}

# Run the operator-UI Playwright suite against a real, defe-provisioned Fleet
# Manager. Extra args are forwarded to `playwright test`, e.g.
#   just test-e2e-ui-fman -- -g "empty fleet" --headed
test-e2e-ui-fman *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  export RUST_BACKTRACE=1
  export RUST_LOG=info
  # bitcoind sizes its file-descriptor pool from the inherited soft limit and
  # refuses to start ("-1 available, 160 required") when that limit is enormous,
  # as it is by default on macOS. Cap it at a sane value for the children.
  ulimit -n 4096 || true
  # The FMan needs a bitcoind and a Nostr relay to start; defe resolves those
  # two from the dev shell's PATH and the manager and launcher from the build
  # directory.
  # `fman` is the package; `fleet-manager` is only its [[bin]] name, so `-p
  # fleet-manager` matches no package and this recipe cannot build.
  cargo build -p fman -p fman-ui-e2e-runner
  bin_dir="${CARGO_TARGET_DIR:-target}/debug"
  cargo run -p defe -- --binary-path "$bin_dir" exec "$bin_dir/fman-ui-e2e-runner" {{ARGS}}

# Build and run the foreground disposable Defe environment.
defe-env *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  ulimit -n 4096 || true
  cargo build \
    -p defe \
    -p defe-env \
    -p devmon \
    -p fman \
    -p fi-cli \
    -p fman-cli \
    -p fedi-decentralized-liquidity-manager-daemon
  bin_dir="${CARGO_TARGET_DIR:-target}/debug"
  "$bin_dir/defe" --binary-path "$bin_dir" env {{ARGS}}

# Start a persistent `defe` development server and restart it when built binaries change.
defe-serve:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  if [ -z "${DEFE_SOCKET:-}" ]; then
    export DEFE_SOCKET="$PWD/.defe.sock"
  fi
  export DEV_DEFE_SOCKET_PATH="$DEFE_SOCKET"
  profile="${CARGO_PROFILE:-debug}"
  build_args=()
  case "$profile" in
    debug|dev)
      profile_dir=debug
      ;;
    *)
      build_args+=(--profile "$profile")
      profile_dir="$profile"
      ;;
  esac
  cargo build "${build_args[@]}" -p defe -p fedi-decentralized-push-gateway
  bin_dir="${CARGO_TARGET_DIR:-target}/$profile_dir"
  export DEFE_RESOURCE_BIN_DIR="$bin_dir"
  export DEFE_SERVER_BIN="$bin_dir/defe"
  export DEFE_PUSH_GATEWAY_BIN="$bin_dir/fedi-decentralized-push-gateway"
  systemfd --no-pid -s "unix::${DEFE_SOCKET}" -- /bin/sh -c \
    "cargo watch --ignore-nothing \
      -w \"\$DEFE_SERVER_BIN\" \
      -w \"\$DEFE_PUSH_GATEWAY_BIN\" \
      -s 'exec \"\$DEFE_SERVER_BIN\" --binary-path \"\$DEFE_RESOURCE_BIN_DIR\" serve --listenfd --log-requests'"

# Run the FULL test suite (unit + FMAN_E2E + FLIP_E2E) impurely against the
# working tree: incremental target dir, no git snapshot, no commit required.
test-e2e-local *ARGS:
  ./scripts/test-e2e-local.sh {{ARGS}}

# Check the proof models under `lean/` that back the `claims/` records.
lean-check:
  cd lean && lake build

# Regenerate everything Rust produces for the TypeScript side: the contract
# fixtures under operator-ui/packages/types/fixtures/, and the FMan
# `AdminRequest` union at operator-ui/packages/types/src/generated/. Both are
# consumed by TS tests and MSW mocks so they cannot drift from the real
# admin-API serde shapes. Review the diff and commit it. Three paired tests fail
# CI if the committed files go stale:
# `cargo test --package fedi-decentralized-service-liquidity-manager --test
# contract_fixtures` (FLIP), `cargo test --package fman-core --test
# contract_fixtures` and `--test admin_request_ts` (FMan).
gen-contract-fixtures:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo run --package fedi-decentralized-service-liquidity-manager --bin gen_contract_fixtures
  cargo run --package fman-core --bin gen_fman_contract_fixtures
  cargo run --package fman-core --bin gen_fman_admin_request_ts

# Measure Rust line coverage for the dashboard release surface (FLIP daemon,
# FMan core, and the shared operator-ui-auth adapter). Deferred: a
# fully-instrumented cargo-llvm-cov build is long-running; not run as part of
# task A5's measured-baseline scope. Requires `cargo llvm-cov` (cargo-llvm-cov).
rust-coverage:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo llvm-cov \
    --package fedi-decentralized-service-liquidity-manager \
    --package operator-ui-auth \
    --package fman-core \
    --lcov --output-path target/llvm-cov/lcov.info

# Run the Linux Nix CI checks locally in a `nixos/nix` Docker container, so a
# macOS host (not a Nix trusted-user) can build them with the fedimint binary
# cache and no native Linux builder. See docs/local-linux-ci.md.
# Examples:
#   just ci-docker clippy
#   just ci-docker cargoFmt clippy leanProofs
#   just ci-docker tests        # needs Docker Desktop memory >= ~24 GiB
ci-docker *TARGETS:
  ./scripts/ci-in-docker.sh {{TARGETS}}
# Run an indexed persistent staging FMan on Mutinynet with its loopback operator UI.
staging-fman *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  set -- {{ARGS}}

  instance=0
  while (($#)); do
    case "$1" in
      --instance)
        if (($# < 2)); then
          echo "--instance requires a non-negative integer" >&2
          exit 2
        fi
        instance="$2"
        shift 2
        ;;
      --instance=*)
        instance="${1#*=}"
        shift
        ;;
      *)
        echo "usage: just staging-fman [--instance N]" >&2
        exit 2
        ;;
    esac
  done
  if [[ ! "$instance" =~ ^[0-9]+$ ]]; then
    echo "--instance requires a non-negative integer" >&2
    exit 2
  fi
  instance=$((10#$instance))

  data_dir_base="${FMAN_STAGING_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/fman-staging}"
  if ((instance == 0)); then
    data_dir="$data_dir_base"
  else
    data_dir="$data_dir_base-$instance"
  fi
  max_seats="${FMAN_STAGING_MAX_SEATS:-3}"
  if [[ ! "$max_seats" =~ ^[0-9]+$ ]] || ((max_seats > 25)); then
    echo "FMAN_STAGING_MAX_SEATS must be between 0 and 25" >&2
    exit 2
  fi
  first_port_base=$((11060 + instance * 100))
  admin_port=$((20061 + instance))
  ui_port=$((25231 + instance))
  if ((first_port_base + max_seats * 4 > 20000 || admin_port >= 21000 || ui_port >= 26000)); then
    echo "--instance is too large for the non-ephemeral staging port ranges" >&2
    exit 2
  fi
  admin_addr="127.0.0.1:$admin_port"
  password_file="$data_dir/operator-password"
  bin="$PWD/target/debug/fleet-manager"

  cargo build -p fman
  (cd operator-ui && pnpm install --frozen-lockfile)
  mkdir -p "$data_dir"
  password="$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')"
  (umask 077 && printf '%s\n' "$password" >"$password_file")
  chmod 600 "$password_file"

  "$bin" serve \
    --data-dir "$data_dir" \
    --manifold-environment staging \
    --first-port-base "$first_port_base" \
    --admin-http-bind "$admin_addr" \
    --admin-http-auth password \
    --admin-http-password-file "$password_file" &
  fman_pid=$!
  ui_pid=""
  cleanup() {
    if [[ -n "$ui_pid" ]]; then
      kill "$ui_pid" 2>/dev/null || true
      wait "$ui_pid" 2>/dev/null || true
    fi
    kill "$fman_pid" 2>/dev/null || true
    wait "$fman_pid" 2>/dev/null || true
  }
  trap cleanup EXIT INT TERM

  onboarded=false
  for _ in {1..200}; do
    if ! kill -0 "$fman_pid" 2>/dev/null; then
      wait "$fman_pid"
    fi
    if "$bin" admin --data-dir "$data_dir" onboard new --if-needed >/dev/null 2>&1; then
      onboarded=true
      break
    fi
    sleep 0.05
  done
  if [[ "$onboarded" != true ]]; then
    echo "fleet-manager did not accept onboarding" >&2
    exit 1
  fi

  api_ready=false
  for _ in {1..200}; do
    if curl --fail --silent \
      --output /dev/null \
      --request POST \
      --header 'content-type: application/json' \
      --data "{\"password\":\"$password\"}" \
      "http://$admin_addr/api/auth"; then
      api_ready=true
      break
    fi
    sleep 0.05
  done
  if [[ "$api_ready" != true ]]; then
    echo "fleet-manager operator API did not become ready" >&2
    exit 1
  fi

  (
    cd operator-ui
    exec env FMAN_ADMIN_PROXY_TARGET="http://$admin_addr" \
      pnpm --filter fman exec vite --host 127.0.0.1 --port "$ui_port"
  ) &
  ui_pid=$!

  ui_ready=false
  for _ in {1..200}; do
    if ! kill -0 "$ui_pid" 2>/dev/null; then
      wait "$ui_pid"
    fi
    if curl --fail --silent --output /dev/null "http://127.0.0.1:$ui_port"; then
      ui_ready=true
      break
    fi
    sleep 0.05
  done
  if [[ "$ui_ready" != true ]]; then
    echo "fleet-manager operator UI did not become ready" >&2
    exit 1
  fi

  echo
  echo "FMan staging instance:    $instance"
  echo "Operator UI:              http://127.0.0.1:$ui_port"
  echo "Operator password:        $password"
  echo "Data directory:           $data_dir"
  echo "First seat port base:     $first_port_base"
  echo "Ctrl-C to stop."
  while kill -0 "$fman_pid" 2>/dev/null && kill -0 "$ui_pid" 2>/dev/null; do
    sleep 1
  done
  if ! kill -0 "$fman_pid" 2>/dev/null; then
    wait "$fman_pid"
  else
    wait "$ui_pid"
  fi

# Run an indexed persistent LDK gatewayd on Mutinynet.
staging-gatewayd *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  set -- {{ARGS}}

  instance=0
  while (($#)); do
    case "$1" in
      --instance)
        if (($# < 2)); then
          echo "--instance requires a non-negative integer" >&2
          exit 2
        fi
        instance="$2"
        shift 2
        ;;
      --instance=*)
        instance="${1#*=}"
        shift
        ;;
      *)
        echo "usage: just staging-gatewayd [--instance N]" >&2
        exit 2
        ;;
    esac
  done
  if [[ ! "$instance" =~ ^[0-9]+$ ]]; then
    echo "--instance requires a non-negative integer" >&2
    exit 2
  fi
  instance=$((10#$instance))

  data_dir_base="${GATEWAYD_STAGING_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/gatewayd-staging}"
  if ((instance == 0)); then
    data_dir="$data_dir_base"
  else
    data_dir="$data_dir_base-$instance"
  fi
  gateway_port=$((27261 + instance))
  lightning_port=$((28261 + instance))
  metrics_port=$((29261 + instance))
  if ((gateway_port >= 32768 || lightning_port >= 32768 || metrics_port >= 32768)); then
    echo "--instance is too large for the non-ephemeral staging port ranges" >&2
    exit 2
  fi
  gateway_url="http://127.0.0.1:$gateway_port"
  password_file="$data_dir/password"
  password_hash_file="$data_dir/password-hash"

  mkdir -p "$data_dir"
  if [[ ! -s "$password_file" ]]; then
    (umask 077 && od -An -N24 -tx1 /dev/urandom | tr -d ' \n' >"$password_file")
  fi
  if [[ ! -s "$password_hash_file" ]]; then
    gateway-cli create-password-hash "$(cat "$password_file")" \
      | tr -d '\"\n' >"$password_hash_file"
  fi
  chmod 600 "$password_file" "$password_hash_file"
  password="$(cat "$password_file")"

  FM_GATEWAY_BCRYPT_PASSWORD_HASH="$(cat "$password_hash_file")" \
  FM_GATEWAY_SKIP_SETUP=true \
  FM_GATEWAY_LIGHTNING_MODULE_MODE=LNv1 \
  FM_GATEWAY_METRICS_LISTEN_ADDR="127.0.0.1:$metrics_port" \
  FM_DEFAULT_ROUTING_FEES='2000,5000' \
  FM_DEFAULT_TRANSACTION_FEES='2000,5000' \
  gatewayd \
    --data-dir "$data_dir/gateway" \
    --listen "127.0.0.1:$gateway_port" \
    --api-addr "$gateway_url" \
    --network signet \
    --esplora-url https://mutinynet.com/api/ \
    --num-route-hints 0 \
    ldk \
    --ldk-lightning-port "$lightning_port" \
    --ldk-alias "staging-gatewayd-$instance" &
  gateway_pid=$!
  cleanup() {
    kill "$gateway_pid" 2>/dev/null || true
    wait "$gateway_pid" 2>/dev/null || true
  }
  trap cleanup EXIT INT TERM

  ready=false
  address_output=""
  for _ in {1..120}; do
    if ! kill -0 "$gateway_pid" 2>/dev/null; then
      wait "$gateway_pid"
    fi
    if address_output="$(gateway-cli --address "$gateway_url" --rpcpassword "$password" onchain address 2>/dev/null)"; then
      ready=true
      break
    fi
    sleep 1
  done
  if [[ "$ready" != true ]]; then
    echo "gatewayd staging instance did not become ready" >&2
    exit 1
  fi

  echo
  echo "gatewayd staging instance: $instance"
  echo "Gateway URL:              $gateway_url"
  echo "Gateway password:         $password"
  echo "Data directory:           $data_dir"
  echo "On-chain address:"
  printf '%s\n' "$address_output"
  echo "Wallet:                   just staging-gatewayd-wallet --instance $instance <command...>"
  echo "Open Mutinynet channel:   just staging-gatewayd-open-channel <SATS> --instance $instance"
  echo "Ctrl-C to stop; wallet state survives restarts."
  wait "$gateway_pid"

# Operate a persistent staging gatewayd wallet.
# Examples:
#   just staging-gatewayd-wallet --instance 0 onchain address
#   just staging-gatewayd-wallet --instance 0 get-balances
#   just staging-gatewayd-wallet --instance 1 lightning list-channels
staging-gatewayd-wallet *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  set -- {{ARGS}}

  instance=0
  if (($# >= 2)) && [[ "$1" == "--instance" ]]; then
    instance="$2"
    shift 2
  elif (($# >= 1)) && [[ "$1" == --instance=* ]]; then
    instance="${1#*=}"
    shift
  fi
  if [[ ! "$instance" =~ ^[0-9]+$ ]]; then
    echo "--instance requires a non-negative integer" >&2
    exit 2
  fi
  instance=$((10#$instance))
  if (($# == 0)); then
    echo "usage: just staging-gatewayd-wallet [--instance N] <gateway-cli command...>" >&2
    exit 2
  fi

  data_dir_base="${GATEWAYD_STAGING_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/gatewayd-staging}"
  if ((instance == 0)); then
    data_dir="$data_dir_base"
  else
    data_dir="$data_dir_base-$instance"
  fi
  gateway_port=$((27261 + instance))
  password_file="$data_dir/password"
  if ((gateway_port >= 32768)); then
    echo "--instance is too large for the non-ephemeral staging port range" >&2
    exit 2
  fi
  if [[ ! -s "$password_file" ]]; then
    echo "gatewayd staging instance $instance is not initialized; start it first" >&2
    exit 1
  fi

  exec gateway-cli \
    --address "http://127.0.0.1:$gateway_port" \
    --rpcpassword "$(cat "$password_file")" \
    "$@"

# Ask FLIP for a gateway deposit address and fund it from the shared treasury.
staging-flip-top-up *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  set -- {{ARGS}}
  sats=""
  instance=0
  while (($#)); do
    case "$1" in
      --instance)
        if (($# < 2)); then
          echo "--instance requires a non-negative integer" >&2
          exit 2
        fi
        instance="$2"
        shift 2
        ;;
      --instance=*)
        instance="${1#*=}"
        shift
        ;;
      -*|?*)
        if [[ -z "$sats" && "$1" != -* ]]; then
          sats="$1"
          shift
        else
          echo "usage: just staging-flip-top-up <SATS> [--instance N]" >&2
          exit 2
        fi
        ;;
    esac
  done
  if [[ ! "$sats" =~ ^[1-9][0-9]*$ ]]; then
    echo "SATS must be a positive integer" >&2
    exit 2
  fi
  if [[ ! "$instance" =~ ^[0-9]+$ ]]; then
    echo "--instance requires a non-negative integer" >&2
    exit 2
  fi
  instance=$((10#$instance))

  data_dir_base="${FLIP_STAGING_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/flip-staging}"
  if ((instance == 0)); then
    data_dir="$data_dir_base"
  else
    data_dir="$data_dir_base-$instance"
  fi
  admin_port=$((21061 + instance))
  token_file="$data_dir/operator-token"
  if ((admin_port >= 22000)); then
    echo "INSTANCE is too large for the non-ephemeral staging port range" >&2
    exit 2
  fi
  if [[ ! -s "$token_file" ]]; then
    echo "FLIP staging instance $instance is not initialized" >&2
    exit 1
  fi

  response="$(curl --fail-with-body --silent --show-error \
    --request POST \
    --header "authorization: Bearer $(cat "$token_file")" \
    --header 'content-type: application/json' \
    --data '{"label":"local Mutinynet treasury"}' \
    "http://127.0.0.1:$admin_port/admin/v1/create_deposit_address")"
  address="$(jq --exit-status --raw-output '.address' <<<"$response")"
  just --justfile "{{justfile()}}" mutinynet-fund "$address" "$sats"

# Open a funded staging gatewayd channel to Mutinynet's public node.
staging-gatewayd-open-channel *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  set -- {{ARGS}}
  sats=""
  instance=0
  while (($#)); do
    case "$1" in
      --instance)
        if (($# < 2)); then
          echo "--instance requires a non-negative integer" >&2
          exit 2
        fi
        instance="$2"
        shift 2
        ;;
      --instance=*)
        instance="${1#*=}"
        shift
        ;;
      -*|?*)
        if [[ -z "$sats" && "$1" != -* ]]; then
          sats="$1"
          shift
        else
          echo "usage: just staging-gatewayd-open-channel <SATS> [--instance N]" >&2
          exit 2
        fi
        ;;
    esac
  done
  if [[ ! "$sats" =~ ^[1-9][0-9]*$ ]]; then
    echo "SATS must be a positive integer" >&2
    exit 2
  fi
  if [[ ! "$instance" =~ ^[0-9]+$ ]]; then
    echo "--instance requires a non-negative integer" >&2
    exit 2
  fi
  just --justfile "{{justfile()}}" staging-gatewayd-wallet --instance "$instance" \
    lightning open-channel \
    --pubkey 02465ed5be53d04fde66c9418ff14a5f2267723810176c9212b722e542dc1afb1b \
    --host 45.79.52.207:9735 \
    --channel-size-sats "$sats"

# Run a persistent Mutinynet treasury for local agents and development tools.
# Fund the printed address once at https://faucet.mutinynet.com/.
mutinynet-faucet:
  #!/usr/bin/env bash
  set -euo pipefail
  data_dir="${MUTINYNET_FAUCET_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/mutinynet-faucet}"
  gateway_url=http://127.0.0.1:30261
  password_file="$data_dir/password"
  password_hash_file="$data_dir/password-hash"

  mkdir -p "$data_dir"
  if [[ ! -s "$password_file" ]]; then
    (umask 077 && od -An -N24 -tx1 /dev/urandom | tr -d ' \n' >"$password_file")
  fi
  if [[ ! -s "$password_hash_file" ]]; then
    gateway-cli create-password-hash "$(cat "$password_file")" \
      | tr -d '\"\n' >"$password_hash_file"
  fi
  chmod 600 "$password_file" "$password_hash_file"
  password="$(cat "$password_file")"

  FM_GATEWAY_BCRYPT_PASSWORD_HASH="$(cat "$password_hash_file")" \
  FM_GATEWAY_SKIP_SETUP=true \
  FM_GATEWAY_LIGHTNING_MODULE_MODE=LNv1 \
  FM_GATEWAY_METRICS_LISTEN_ADDR=127.0.0.1:32261 \
  FM_DEFAULT_ROUTING_FEES='2000,5000' \
  FM_DEFAULT_TRANSACTION_FEES='2000,5000' \
  gatewayd \
    --data-dir "$data_dir/gateway" \
    --listen 127.0.0.1:30261 \
    --api-addr "$gateway_url" \
    --network signet \
    --esplora-url https://mutinynet.com/api/ \
    --num-route-hints 0 \
    ldk \
    --ldk-lightning-port 31261 \
    --ldk-alias manifold-local-faucet &
  gateway_pid=$!
  cleanup() {
    kill "$gateway_pid" 2>/dev/null || true
    wait "$gateway_pid" 2>/dev/null || true
  }
  trap cleanup EXIT INT TERM

  ready=false
  address_output=""
  for _ in {1..120}; do
    if ! kill -0 "$gateway_pid" 2>/dev/null; then
      wait "$gateway_pid"
    fi
    if address_output="$(gateway-cli --address "$gateway_url" --rpcpassword "$password" onchain address 2>/dev/null)"; then
      ready=true
      break
    fi
    sleep 1
  done
  if [[ "$ready" != true ]]; then
    echo "Mutinynet faucet wallet did not become ready" >&2
    exit 1
  fi

  echo
  echo "Mutinynet local faucet is ready."
  echo "Fund this persistent wallet once:"
  printf '%s\n' "$address_output"
  echo
  echo "Balance:                  just mutinynet-wallet get-balances"
  echo "Fund an address:          just mutinynet-fund <ADDRESS> <SATS>"
  echo "Pay a Lightning invoice: just mutinynet-pay <INVOICE>"
  echo "Open faucet-node channel: just mutinynet-open-channel <SATS>"
  echo "Raw wallet access:        just mutinynet-wallet <gateway-cli command...>"
  echo "Ctrl-C to stop; wallet state survives restarts."
  wait "$gateway_pid"

# Run a gateway-cli command against the persistent Mutinynet treasury.
mutinynet-wallet *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  set -- {{ARGS}}
  if (($# == 0)); then
    echo "usage: just mutinynet-wallet <gateway-cli command...>" >&2
    exit 2
  fi
  data_dir="${MUTINYNET_FAUCET_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/mutinynet-faucet}"
  password_file="$data_dir/password"
  if [[ ! -s "$password_file" ]]; then
    echo "Mutinynet treasury is not initialized; start just mutinynet-faucet" >&2
    exit 1
  fi
  exec gateway-cli \
    --address http://127.0.0.1:30261 \
    --rpcpassword "$(cat "$password_file")" \
    "$@"

# Send test sats from the persistent Mutinynet treasury.
mutinynet-fund ADDRESS SATS:
  #!/usr/bin/env bash
  set -euo pipefail
  sats='{{SATS}}'
  if [[ ! "$sats" =~ ^[1-9][0-9]*$ ]]; then
    echo "SATS must be a positive integer" >&2
    exit 2
  fi
  just --justfile "{{justfile()}}" mutinynet-wallet onchain send \
    --address '{{ADDRESS}}' \
    --amount "$((sats * 1000))" \
    --fee-rate-sats-per-vbyte 1

# Pay a test Lightning invoice from the persistent Mutinynet treasury.
mutinynet-pay INVOICE:
  just mutinynet-wallet lightning pay-invoice '{{INVOICE}}'

# Open a funded channel from the treasury to Mutinynet's public faucet node.
mutinynet-open-channel SATS:
  #!/usr/bin/env bash
  set -euo pipefail
  sats='{{SATS}}'
  if [[ ! "$sats" =~ ^[1-9][0-9]*$ ]]; then
    echo "SATS must be a positive integer" >&2
    exit 2
  fi
  just --justfile "{{justfile()}}" mutinynet-wallet lightning open-channel \
    --pubkey 02465ed5be53d04fde66c9418ff14a5f2267723810176c9212b722e542dc1afb1b \
    --host 45.79.52.207:9735 \
    --channel-size-sats "$sats"

# Run an indexed persistent staging FLIP on Mutinynet with the operator UI.
staging-flip *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  set -- {{ARGS}}

  instance=0
  while (($#)); do
    case "$1" in
      --instance)
        if (($# < 2)); then
          echo "--instance requires a non-negative integer" >&2
          exit 2
        fi
        instance="$2"
        shift 2
        ;;
      --instance=*)
        instance="${1#*=}"
        shift
        ;;
      *)
        echo "usage: just staging-flip [--instance N]" >&2
        exit 2
        ;;
    esac
  done
  if [[ ! "$instance" =~ ^[0-9]+$ ]]; then
    echo "--instance requires a non-negative integer" >&2
    exit 2
  fi

  instance=$((10#$instance))

  data_dir_base="${FLIP_STAGING_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/flip-staging}"
  if ((instance == 0)); then
    data_dir="$data_dir_base"
  else
    data_dir="$data_dir_base-$instance"
  fi
  admin_port=$((21061 + instance))
  public_port=$((22061 + instance))
  gateway_port=$((27261 + instance))
  ui_port=$((26231 + instance))
  if ((admin_port >= 22000 || public_port >= 23000 || gateway_port >= 32768 || ui_port >= 27000)); then
    echo "--instance is too large for the non-ephemeral staging port ranges" >&2
    exit 2
  fi
  admin_addr="127.0.0.1:$admin_port"
  public_addr="127.0.0.1:$public_port"
  gateway_url="http://127.0.0.1:$gateway_port"
  token_file="$data_dir/operator-token"
  provider_key_file="$data_dir/provider-nostr-secret-key"
  bin="$PWD/target/debug/liquidity-manager-daemon"

  cargo build -p fedi-decentralized-liquidity-manager-daemon --bin liquidity-manager-daemon
  (cd operator-ui && pnpm install --frozen-lockfile)
  mkdir -p "$data_dir"
  if [[ ! -s "$token_file" ]]; then
    (umask 077 && od -An -N24 -tx1 /dev/urandom | tr -d ' \n' >"$token_file")
  fi
  if [[ ! -s "$provider_key_file" ]]; then
    (umask 077 && od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$provider_key_file")
  fi
  chmod 600 "$token_file" "$provider_key_file"
  token="$(cat "$token_file")"
  provider_key="$(cat "$provider_key_file")"

  env -u DEV_DEFE_SOCKET_PATH FLIP_BOOTSTRAP_ADMIN_TOKEN="$token" "$bin" run daemon \
    --manifold-environment staging \
    --data-dir "$data_dir" \
    --admin-bind-address "$admin_addr" \
    --public-bind-address "$public_addr" &
  flip_pid=$!
  ui_pid=""
  cleanup() {
    if [[ -n "$ui_pid" ]]; then
      kill "$ui_pid" 2>/dev/null || true
      wait "$ui_pid" 2>/dev/null || true
    fi
    kill "$flip_pid" 2>/dev/null || true
    wait "$flip_pid" 2>/dev/null || true
  }
  trap cleanup EXIT INT TERM

  api_ready=false
  for _ in {1..200}; do
    if ! kill -0 "$flip_pid" 2>/dev/null; then
      wait "$flip_pid"
    fi
    if curl --fail --silent --output /dev/null "http://$admin_addr/health"; then
      api_ready=true
      break
    fi
    sleep 0.05
  done
  if [[ "$api_ready" != true ]]; then
    echo "FLIP operator API did not become ready" >&2
    exit 1
  fi

  if ! curl --fail --silent --output /dev/null \
    --request POST \
    --header "authorization: Bearer $token" \
    --header 'content-type: application/json' \
    --data "{\"nostr_secret_key\":\"$provider_key\"}" \
    "http://$admin_addr/admin/v1/install_provider_identity"; then
    echo "FLIP identity install failed; the stored admin token may have been rotated" >&2
    exit 1
  fi

  (
    cd operator-ui
    exec env FLIP_ADMIN_PROXY_TARGET="http://$admin_addr" \
      pnpm --filter flip exec vite --host 127.0.0.1 --port "$ui_port"
  ) &
  ui_pid=$!

  ui_ready=false
  for _ in {1..200}; do
    if ! kill -0 "$ui_pid" 2>/dev/null; then
      wait "$ui_pid"
    fi
    if curl --fail --silent --output /dev/null "http://127.0.0.1:$ui_port"; then
      ui_ready=true
      break
    fi
    sleep 0.05
  done
  if [[ "$ui_ready" != true ]]; then
    echo "FLIP operator UI did not become ready" >&2
    exit 1
  fi

  echo
  echo "FLIP staging instance:    $instance"
  echo "Operator UI:              http://127.0.0.1:$ui_port"
  echo "Operator token:           $token"
  echo "Data directory:           $data_dir"
  echo "Public API bind:          $public_addr"
  echo "Expected gateway URL:     $gateway_url"
  echo "Gateway:                  run just staging-gatewayd --instance $instance separately"
  echo "Top up from treasury:     just staging-flip-top-up <SATS> --instance $instance"
  echo "Ctrl-C to stop."
  while kill -0 "$flip_pid" 2>/dev/null && kill -0 "$ui_pid" 2>/dev/null; do
    sleep 1
  done
  if ! kill -0 "$flip_pid" 2>/dev/null; then
    wait "$flip_pid"
  else
    wait "$ui_pid"
  fi

# Issue staging trust material, install the issuer authority, and enroll the
# published holder authorization for a running FLIP instance.
staging-flip-trust *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  set -- {{ARGS}}

  instance=0
  while (($#)); do
    case "$1" in
      --instance)
        if (($# < 2)); then
          echo "--instance requires a non-negative integer" >&2
          exit 2
        fi
        instance="$2"
        shift 2
        ;;
      --instance=*)
        instance="${1#*=}"
        shift
        ;;
      *)
        echo "usage: just staging-flip-trust [--instance N]" >&2
        exit 2
        ;;
    esac
  done
  if [[ ! "$instance" =~ ^[0-9]+$ ]]; then
    echo "--instance requires a non-negative integer" >&2
    exit 2
  fi
  instance=$((10#$instance))

  data_dir_base="${FLIP_STAGING_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/flip-staging}"
  if ((instance == 0)); then
    data_dir="$data_dir_base"
  else
    data_dir="$data_dir_base-$instance"
  fi
  admin_port=$((21061 + instance))
  token_file="$data_dir/operator-token"
  if ((admin_port >= 22000)); then
    echo "--instance is too large for the non-ephemeral staging port range" >&2
    exit 2
  fi
  if [[ ! -s "$token_file" || ! -s "$data_dir/flip.sqlite" ]]; then
    echo "FLIP staging instance $instance is not initialized" >&2
    exit 1
  fi

  subject="$(python3 - "$data_dir/flip.sqlite" <<'PY'
  import sqlite3
  import sys

  row = sqlite3.connect(sys.argv[1]).execute(
      "SELECT provider_pubkey FROM provider_identity WHERE id = 1"
  ).fetchone()
  if row is None:
      raise SystemExit("FLIP provider identity is not installed")
  print(row[0])
  PY
  )"
  authorization_request="$(jq --compact-output --null-input \
    --arg subject_pubkey "$subject" '{subject_pubkey:$subject_pubkey}')"
  echo "Issuing staging trust material for FLIP $instance..." >&2
  # `--publish-flip-authorization` publishes the kind-37705 holder
  # authorization to the environment-pinned relays; FLIP enrolls it from
  # there (SPEC-flip-holder-authorization), so only the issuer authority is
  # installed directly — `attestation_install` refuses credentials and
  # holder authorizations.
  bundle="$(cargo run --quiet -p devmon --bin manifold-test-issuer -- \
    --environment staging \
    --authorization-request "$authorization_request" \
    --publish-flip-authorization)"
  jq '{subject_pubkey,issuer_pubkey,holder_pubkey,authority_event_ids,authorization_event_ids}' <<<"$bundle"

  echo "Installing the issuer authority..." >&2
  token="$(cat "$token_file")"
  request="$(jq --compact-output \
    '{payload:(.issuer_authority | tojson | explode)}' <<<"$bundle")"
  curl --fail-with-body --silent --show-error \
    --request POST \
    --header "authorization: Bearer $token" \
    --header 'content-type: application/json' \
    --data "$request" \
    "http://127.0.0.1:$admin_port/admin/v1/attestation_install" \
    | jq --compact-output '{id,kind}'

  echo "Enrolling the published holder authorization from the relays..." >&2
  # A refresh deliberately succeeds on partial or empty relay answers, and
  # reruns of this recipe accumulate holders, so require THIS run's holder to
  # be enrolled — anything weaker can exit 0 on stale trust material.
  # Publication and the refresh race relay propagation, so retry like
  # staging-fman-trust does instead of failing confusingly at the republish
  # step when the first read comes back empty.
  holder="$(jq --exit-status --raw-output '.holder_pubkey' <<<"$bundle")"
  enrolled=""
  for _ in {1..20}; do
    refresh="$(curl --fail-with-body --silent --show-error \
      --request POST \
      --header "authorization: Bearer $token" \
      "http://127.0.0.1:$admin_port/admin/v1/refresh_holder_authorizations")"
    if jq --exit-status --arg holder "$holder" \
      '.status.state == "authorization_observed"
        and (.status.holders | index($holder) != null)' <<<"$refresh" >/dev/null; then
      enrolled=1
      jq --compact-output '{relays_answered,candidates_seen,candidates_verified,status}' <<<"$refresh"
      break
    fi
    sleep 1
  done
  if [[ -z "$enrolled" ]]; then
    echo "holder $holder was not enrolled from the relays" >&2
    jq --compact-output . <<<"$refresh" >&2
    exit 1
  fi

  echo "Republishing the FLIP advertisement..." >&2
  curl --fail-with-body --silent --show-error \
    --request POST \
    --header "authorization: Bearer $token" \
    --header 'content-type: application/json' \
    --data '{"force":true}' \
    "http://127.0.0.1:$admin_port/admin/v1/republish_advertisement" \
    | jq '{publication_status,relay_states}'

# Issue staging trust material and publish the holder authorization an FMan discovers.
staging-fman-trust *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  set -- {{ARGS}}

  instance=0
  while (($#)); do
    case "$1" in
      --instance)
        if (($# < 2)); then
          echo "--instance requires a non-negative integer" >&2
          exit 2
        fi
        instance="$2"
        shift 2
        ;;
      --instance=*)
        instance="${1#*=}"
        shift
        ;;
      *)
        echo "usage: just staging-fman-trust [--instance N]" >&2
        exit 2
        ;;
    esac
  done
  if [[ ! "$instance" =~ ^[0-9]+$ ]]; then
    echo "--instance requires a non-negative integer" >&2
    exit 2
  fi
  instance=$((10#$instance))

  data_dir_base="${FMAN_STAGING_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/manifold/fman-staging}"
  if ((instance == 0)); then
    data_dir="$data_dir_base"
  else
    data_dir="$data_dir_base-$instance"
  fi
  bin="$PWD/target/debug/fleet-manager"
  if [[ ! -x "$bin" ]]; then
    cargo build -p fman
  fi
  onboarding="$($bin admin --data-dir "$data_dir" onboarding)"
  subject="$(jq --exit-status --raw-output '.service_nostr_pubkey' <<<"$onboarding")"
  authorization_request="$(jq --compact-output --null-input \
    --arg subject_pubkey "$subject" '{subject_pubkey:$subject_pubkey}')"
  echo "FMan holder-authorization request:" >&2
  jq . <<<"$authorization_request" >&2
  issued="$(cargo run --quiet -p devmon --bin manifold-test-issuer -- \
    --environment staging \
    --authorization-request "$authorization_request" \
    --publish-fman-authorization)"
  jq '{subject_pubkey,issuer_pubkey,holder_pubkey,authority_event_ids,authorization_event_ids}' \
    <<<"$issued"

  echo "Waiting for FMan to observe the published authorization..." >&2
  holder="$(jq --exit-status --raw-output '.holder_pubkey' <<<"$issued")"
  for _ in {1..20}; do
    onboarding="$($bin admin --data-dir "$data_dir" onboarding)"
    if jq --exit-status --arg holder "$holder" \
      '.nostr.holders | index($holder) != null' <<<"$onboarding" >/dev/null; then
      jq '.nostr' <<<"$onboarding"
      exit 0
    fi
    sleep 1
  done
  echo "FMan did not observe its holder authorization within 20 seconds" >&2
  exit 1
