# Fleet Manager 0.1 packaging

This directory contains the smallest useful packaging artifact for the
Fleet Manager daemon described by
[`ARCH-fleet-manager`](../../crates/fman/specs/ARCH-fleet-manager.md):

- a Nix-built OCI image (`nix build .#fleet-manager-oci-image`) carrying the
  single `fleet-manager` binary, which is also the `fedimintd` its seats run;
- an entrypoint that wires Bitcoin mainnet and Bitcoin Core RPC settings into
  the daemon's 0.1 CLI args.

Real, shipping packages live outside this repo and pin the published GHCR
images: [manifold-umbrel-store](https://github.com/fedibtc/manifold-umbrel-store)
(Umbrel, staging) and
[manifold-fman-startos](https://github.com/fedibtc/manifold-fman-startos)
(StartOS 0.4, staging). This directory keeps only what the image itself
carries — the entrypoint and its contract below — plus the operator
deployment checklist.

The [secure-deployment checklist](./secure-deployment.md) is the authoritative
operator contract for FMan's external production envelope. Nothing here
certifies an Umbrel, StartOS, VPS, or any other live deployment.

Required SelfCI packages the real operator UI with a CI-profile Fleet Manager
daemon and checks the OCI runtime contract. Publishing uses the same image
constructor with the release-profile daemon through `release-container-images`;
the optimized image is first built in the trusted publish workflow.

The bundled `fedimintd` is compiled into the daemon binary from the `fedimint`
flake input pinned in
[`flake.nix`](../../flake.nix) — currently the immutable Fedi release
[`v0.11.1-fedi16`](https://github.com/fedibtc/fedimint/releases/tag/v0.11.1-fedi16)
(commit `881b0c2eda6b4b97785fce977a9c7ea65942a0ee`), whose release identity is
carried as
`0.11.1-fedi16` (the `FEDIMINTD_VERSION_0_1` constant and the image's
`org.fedi.fedimintd.release` label). The pinned release is bumped by updating
the `fedimint` flake input (and `flake.lock`), not this package. The
`fleet-manager-cli-contract` / OCI-image checks fail if the tag, the release
constant, this document, and the image label drift apart.

## Runtime contract

The image runs:

```text
fleet-manager serve \
  --data-dir $FLEET_MANAGER_DATA_DIR \
  --manifold-environment $FLEET_MANAGER_MANIFOLD_ENVIRONMENT \
  --push-gateway-origin $FLEET_MANAGER_PUSH_GATEWAY_ORIGIN \
  --bitcoind-url $FLEET_MANAGER_BITCOIND_URL \
  --bitcoind-username $FLEET_MANAGER_BITCOIND_USERNAME \
  --bitcoind-password=$FLEET_MANAGER_BITCOIND_PASSWORD \
  [--admin-http-bind $FLEET_MANAGER_ADMIN_HTTP_BIND \
   --admin-http-auth $FLEET_MANAGER_ADMIN_HTTP_AUTH \
   --admin-http-password-file $FLEET_MANAGER_ADMIN_HTTP_PASSWORD_FILE]
```

Seat capacity and price are configured durably during browser or admin-socket onboarding. The daemon presents the available-RAM recommendation from [REQ-seat-capacity-default](../../crates/fman/specs/REQ-seat-capacity-default.md); it is not a process-start argument.

`FLEET_MANAGER_MANIFOLD_ENVIRONMENT` is required and must match the FI and
FLIP deployment (`development`, `staging`, or `production`).
The profile supplies the Bitcoin network; this production package supplies an
operator-owned Bitcoin Core backend instead of any profile Esplora default.
`FLEET_MANAGER_PUSH_GATEWAY_ORIGIN` is also required by every production
package path and must be the real public HTTPS origin of the deployed gateway.
The package intentionally has no fake/localhost default. The daemon validates
every callback bearer against this origin before any network request. Package
startup fails when the variable is absent. A directly invoked daemon may omit
the option to keep callback-free service available; it rejects new callbacks,
and callback work restored under a changed origin becomes `operator_blocked`
until the matching origin is restored.

The last three arguments are the operator dashboard and HTTP admin API
([`SPEC-operator-http`](../../crates/fman/specs/SPEC-operator-http.md)). The Nix
release binary embeds the dashboard, so this listener is the only way to reach
it in the image; there is no separate dashboard container. The entrypoint adds
the arguments only when `FLEET_MANAGER_ADMIN_HTTP_BIND` is set, and there is no
default bind address on purpose: `trusted-proxy` is sound only when an
authenticating platform proxy is the listener's sole peer, and `password`
additionally requires `FLEET_MANAGER_ADMIN_HTTP_PASSWORD_FILE`. Setting the auth
mode or the password file without a bind address fails startup rather than
starting a daemon with no dashboard.

No package performs a runtime `fedimintd --version` probe or downloads a binary:
a seat's `fedimintd` is this very binary, spawned under a `fedimintd` argv[0].

## Seat iroh reachability

Each seat's `fedimintd` places its iroh UDP sockets at the seat's p2p and api
ports (the daemon binds those two on all interfaces; ui and metrics stay
loopback-only). Containerized packages should publish the seat port grid as
**UDP** so iroh can hole-punch direct peer paths instead of falling back to
public relays. Only UDP is published: in iroh mode fedimintd binds no TCP
listener at the p2p port, and the api port's plaintext WebSocket client API
(the same public API already served over iroh; admin verbs gated by the
seat's api auth) is deliberately left unpublished. The grid is 4 ports per seat
from `--first-port-base` (default 30000), and seat ordinals are
lifetime-monotonic — a decommissioned seat's ordinal is never reused — so a
fixed mapping such as 30000-30031 covers the first 8 seats a host ever
creates, not 8 concurrent seats. A seat allocated beyond the published range
still works but falls back to relays; extend the mapping (a package update)
to restore direct paths for later ordinals.

## Focused validation

From the repository root:

```sh
packages/fleet-manager/validate.sh
```

To build and load the image locally:

```sh
nix build .#fleet-manager-oci-image && docker load -i result
# or, equivalently:
nix run .#fleet-manager-container-load
```

Package manifests are validated in their own repos
(manifold-umbrel-store, manifold-fman-startos), not here.
