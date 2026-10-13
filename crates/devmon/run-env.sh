#!/usr/bin/env bash
# Bring up a live Nostr environment and the devmon dashboard, then hold it open.
#
# The relay is leased from defe (the repo's test-resource harness), not hand-rolled: the script
# re-execs itself under `defe exec defe-cli --request-relay`, which starts a defe server, leases a
# Nostr relay, and exports its URL as DEV_DEFE_NOSTR_RELAY_URL for the rest of the run. It then
# starts and onboards N fleet-manager daemons advertising on that relay (kind 37701), then starts
# devmon watching it.
#
# Needs no bitcoind: a fleet-manager advertises without it, and only starts its
# bundled fedimintd once a seat forms.
#
# Development tool. Run from inside `nix develop`.
set -euo pipefail

MG_COUNT="${MG_COUNT:-3}"
DASH_PORT="${DASH_PORT:-7777}"
# Dev setup-payment publisher placeholder (public test secret 3), matching the
# development profile default and distinct from the dev issuer placeholder key.
SETUP_PAYMENT_PUBLISHER="f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9"

REPO_ROOT="$(git rev-parse --show-toplevel)"
BIN="$REPO_ROOT/target/debug"

for bin in defe defe-cli fleet-manager fman-cli devmon manifold-test-issuer; do
  [[ -x "$BIN/$bin" ]] || {
    echo "missing $BIN/$bin, run: cargo build -p defe -p defe-client -p mg -p mg-cli -p devmon --bins"
    exit 1
  }
done

# First pass: no relay yet. Re-exec under a defe-leased relay. defe finds nostr-rs-relay on PATH
# (present in `nix develop`); defe-cli holds the lease until this script exits.
if [[ -z "${DEV_DEFE_NOSTR_RELAY_URL:-}" ]]; then
  exec "$BIN/defe" --binary-path "$BIN" exec \
    "$BIN/defe-cli" --request-relay=shared -- "$0" "$@"
fi

RELAY_URL="$DEV_DEFE_NOSTR_RELAY_URL"
RUN_DIR="$(mktemp -d /tmp/devmon-env.XXXXXX)"

pids=()
cleanup() {
  echo
  echo "shutting down…"
  for pid in "${pids[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
  rm -rf "$RUN_DIR"
}
trap cleanup EXIT INT TERM

echo "run dir:   $RUN_DIR"
echo "relay:     $RELAY_URL (defe lease)"

for i in $(seq 1 "$MG_COUNT"); do
  data_dir="$RUN_DIR/mg$i"
  mkdir -p "$data_dir"
  # Seat port grids must be disjoint per host: the daemon allocates blocks of 4k from the base.
  MANIFOLD_DEV_NOSTR_RELAYS="$RELAY_URL" \
  MANIFOLD_DEV_SETUP_PAYMENT_PUBLISHER="$SETUP_PAYMENT_PUBLISHER" \
  "$BIN/fleet-manager" serve \
    --data-dir "$data_dir" \
    --manifold-environment development \
    --bitcoind-url "http://127.0.0.1:18443" \
    --bitcoind-username devmon \
    --bitcoind-password devmon \
    --first-port-base "$((30000 + (i - 1) * 4000))" \
    > "$data_dir/mg.log" 2>&1 &
  mg_pid=$!
  pids+=("$mg_pid")

  # This disposable demo completes the same mandatory stages as the UI.
  onboarded=false
  for _ in {1..200}; do
    if ! kill -0 "$mg_pid" 2>/dev/null; then
      echo "mg$i exited before onboarding; see $data_dir/mg.log" >&2
      exit 1
    fi
    if "$BIN/fman-cli" --data-dir "$data_dir" onboard new --if-needed \
      >/dev/null 2>&1; then
      onboarded=true
      break
    fi
    sleep 0.05
  done
  if [[ "$onboarded" != true ]]; then
    echo "mg$i did not accept onboarding; see $data_dir/mg.log" >&2
    exit 1
  fi
  onboarding="$($BIN/fman-cli --data-dir "$data_dir" onboarding)"
  authorization_request="$(python3 -c 'import json,sys; print(json.dumps({"subject_pubkey": json.load(sys.stdin)["service_nostr_pubkey"]}))' <<<"$onboarding")"
  "$BIN/manifold-test-issuer" --environment development --relay "$RELAY_URL" \
    --authorization-request "$authorization_request" --publish-mg-authorization >/dev/null
  "$BIN/fman-cli" --data-dir "$data_dir" refresh-holder-authorizations >/dev/null
  "$BIN/fman-cli" --data-dir "$data_dir" onboard offer --max-seats "$((i + 1))" >/dev/null
  echo "mg$i      capacity=$((i + 1))  log: $data_dir/mg.log"
done

"$BIN/devmon" --relay "$RELAY_URL" --port "$DASH_PORT" &
pids+=($!)

echo
echo "dashboard  http://127.0.0.1:${DASH_PORT}"
echo "MGs publish on startup and republish hourly. Ctrl-C to stop."
wait
