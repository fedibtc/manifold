#!/usr/bin/env bash
set -euo pipefail

: "${FLEET_MANAGER_DATA_DIR:=/data}"

: "${FLEET_MANAGER_MANIFOLD_ENVIRONMENT:?set FLEET_MANAGER_MANIFOLD_ENVIRONMENT to the deployment trust environment}"
: "${FLEET_MANAGER_PUSH_GATEWAY_ORIGIN:?set FLEET_MANAGER_PUSH_GATEWAY_ORIGIN to the deployed HTTPS push-gateway origin}"

# Platform packages must provide these from their Bitcoin Core dependency.
: "${FLEET_MANAGER_BITCOIND_URL:?set FLEET_MANAGER_BITCOIND_URL from platform bitcoind RPC}"
: "${FLEET_MANAGER_BITCOIND_USERNAME:?set FLEET_MANAGER_BITCOIND_USERNAME from platform bitcoind RPC}"
: "${FLEET_MANAGER_BITCOIND_PASSWORD:?set FLEET_MANAGER_BITCOIND_PASSWORD from platform bitcoind RPC}"

# Operator dashboard and HTTP admin API. A release binary embeds the dashboard
# and serves it from this listener, so this is the only way to reach the
# dashboard in a shipped image.
#
# Off unless the deployment asks for it, and no default bind address: the two
# authentication modes have different safe deployments (`trusted-proxy` is sound
# only when an authenticating platform proxy is the listener's sole peer), so
# the package must not pick one. See crates/fman/specs/SPEC-operator-http.md.
operator_http=()
if [ -n "${FLEET_MANAGER_ADMIN_HTTP_BIND:-}" ]; then
  : "${FLEET_MANAGER_ADMIN_HTTP_AUTH:?set FLEET_MANAGER_ADMIN_HTTP_AUTH to trusted-proxy or password alongside FLEET_MANAGER_ADMIN_HTTP_BIND}"
  operator_http+=(
    --admin-http-bind "${FLEET_MANAGER_ADMIN_HTTP_BIND}"
    --admin-http-auth "${FLEET_MANAGER_ADMIN_HTTP_AUTH}"
  )
  # The daemon rejects a password file in trusted-proxy mode, so pass it only
  # where it belongs rather than forwarding whatever is set.
  if [ "${FLEET_MANAGER_ADMIN_HTTP_AUTH}" = "password" ]; then
    : "${FLEET_MANAGER_ADMIN_HTTP_PASSWORD_FILE:?set FLEET_MANAGER_ADMIN_HTTP_PASSWORD_FILE with password auth}"
    operator_http+=(--admin-http-password-file "${FLEET_MANAGER_ADMIN_HTTP_PASSWORD_FILE}")
  fi
elif [ -n "${FLEET_MANAGER_ADMIN_HTTP_AUTH:-}" ] || [ -n "${FLEET_MANAGER_ADMIN_HTTP_PASSWORD_FILE:-}" ]; then
  echo "FLEET_MANAGER_ADMIN_HTTP_BIND is required to enable the operator HTTP listener" >&2
  exit 1
fi

exec fleet-manager serve \
  --data-dir "${FLEET_MANAGER_DATA_DIR}" \
  --manifold-environment "${FLEET_MANAGER_MANIFOLD_ENVIRONMENT}" \
  --push-gateway-origin "${FLEET_MANAGER_PUSH_GATEWAY_ORIGIN}" \
  --bitcoind-url "${FLEET_MANAGER_BITCOIND_URL}" \
  --bitcoind-username "${FLEET_MANAGER_BITCOIND_USERNAME}" \
  --bitcoind-password="${FLEET_MANAGER_BITCOIND_PASSWORD}" \
  ${operator_http[@]+"${operator_http[@]}"}
