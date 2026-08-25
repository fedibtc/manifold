# Decentralized Federations

Shared monorepo for the Federation Decentralization Scaling project. Contains component design docs, the cross-component protocol, and reference Rust types.

## Repository structure

| Path | Description |
| --- | --- |
| `crates/fi-client/` | Consumer-neutral, stateful Federation Initiator library - [start here](./crates/fi-client/specs/ARCH-fi-client.md) |
| `crates/fi-cli/` | Development/test-only terminal and E2E consumer of `fi-client` |
| `crates/fman/` | Fleet Manager (FMan): `core` (the daemon and the capability traits it defines), `fedimint` and `nostr` (implementations of them), `bin` (the composition root) |
| `crates/setup-payment-publisher/` | Production custodian tool for signing and publishing the common setup-payment federation policy |
| `crates/` | Remaining Rust workspace crates: shared domain/service types, daemons, and test infrastructure |
| `docs/fedi-app/` | Fedi app design (the primary `fi-client` consumer) |
| `crates/liquidity-manager-daemon/` | FLIP (Federation Liquidity Provisioner) daemon - [start here](./crates/liquidity-manager-daemon/specs/ARCH-liquidity-manager.md) |
| `docs/liquidity-manager/` | FLIP open-items tracker, trust-validation implementation guide, and Docker packaging notes |
| `crates/fman/specs/` | Fleet Manager (FMan) design - [start here](./crates/fman/specs/ARCH-fleet-manager.md) |
| `packages/fleet-manager/` | Fleet Manager 0.1 Umbrel/StartOS package skeletons |

## Development environment

Nix is required for builds and development. Enter `nix develop` (or allow
direnv to load the flake) before running Cargo commands; Cargo outside the Nix
environment is unsupported because Nix provides source path dependencies
under `.nix-deps/`.

All source inputs used by the development shell are public. The `github:`
fetcher uses the GitHub API, not `ssh-agent`, and needs no repository access
token for these inputs.

To update the credential SDK intentionally, update its Nix input first, then
refresh the Rust dependency graph in a new dev shell:

```bash
nix flake update credential-sdk-src
nix develop --command cargo update -p fedi-credential-sdk-protocol
nix develop --command cargo metadata --locked
```

Review both lockfiles: `flake.lock` owns the SDK source revision and content
hash, while `Cargo.lock` owns the resolved Rust package graph.

### Running the Linux CI locally (Docker)

CI runs on Linux. To build the `.#ci.*` checks from a macOS workstation — which
has no native Linux builder and, unless configured as a Nix trusted-user, cannot
use the fedimint binary cache — run them inside a `nixos/nix` Docker container:

```bash
just ci-docker clippy    # lint/compile the whole workspace
just ci-docker tests     # full test suite; needs Docker Desktop memory >= ~24 GiB
```

Inside the container we are a trusted Nix user, so the fedimint closure is fetched
prebuilt instead of compiled from source. See
[docs/local-linux-ci.md](./docs/local-linux-ci.md) for targets, the `tests`
memory requirement, and container setup.

## `defe` development test runner

This workspace includes `defe`, a local client/server helper for integration tests that need temporary resources such as a Nostr relay.

Smoke-test the server/client loop from the repository root with:

```bash
cargo build -p defe -p defe-client
target/debug/defe exec target/debug/defe-cli ping
# or run the server through Cargo after building the client:
cargo run -p defe -- exec target/debug/defe-cli ping
```

Wrap a command that needs a relay or push-gateway lease with:

```bash
target/debug/defe exec target/debug/defe-cli --request-relay -- sh -c 'echo "$DEV_DEFE_NOSTR_RELAY_URL"'
target/debug/defe exec target/debug/defe-cli --request-relay=exclusive -- <cmd> [args...]
target/debug/defe exec target/debug/defe-cli --request-push-gateway -- sh -c 'echo "$DEV_DEFE_PUSH_GATEWAY_URL"'
target/debug/defe exec target/debug/defe-cli --request-bitcoind -- sh -c 'echo "$DEV_DEFE_BITCOIND_URL"'
```

`defe-cli --request-relay` exports `DEV_DEFE_NOSTR_RELAY_URL`, `DEV_DEFE_NOSTR_RELAY_PORT`, and `DEV_DEFE_NOSTR_RELAY_DATA_DIR` for the child command. `defe-cli --request-push-gateway` exports `DEV_DEFE_PUSH_GATEWAY_URL`, `DEV_DEFE_PUSH_GATEWAY_PORT`, `DEV_DEFE_PUSH_GATEWAY_APP_ID`, and `DEV_DEFE_PUSH_GATEWAY_DATABASE_PATH`. `defe-cli --request-bitcoind` starts Bitcoin Core in regtest mode and exports `DEV_DEFE_BITCOIND_URL`, `DEV_DEFE_BITCOIND_RPC_HOST`, `DEV_DEFE_BITCOIND_RPC_PORT`, `DEV_DEFE_BITCOIND_P2P_PORT`, `DEV_DEFE_BITCOIND_RPC_USERNAME`, `DEV_DEFE_BITCOIND_RPC_PASSWORD`, and `DEV_DEFE_BITCOIND_DATA_DIR`; Fleet Manager E2E tests pass these through to `--bitcoind-*` daemon args. In all cases the wrapper keeps the lease alive until the child exits. Regular workspace tests should use fake or lightweight resources and do not require `nostr-rs-relay` or `bitcoind`; the low-level `defe`/`defe-client` real relay probes are opt-in ignored tests gated by `DEV_DEFE_RUN_REAL_NOSTR_RELAY_TESTS=1`.

Build managed resource binaries before using a persistent server, or add the Cargo profile output directory to the managed-binary search path:

```bash
cargo build -p defe -p defe-client -p fedi-decentralized-push-gateway
cargo run -p defe -- --binary-path target/debug exec target/debug/defe-cli --request-push-gateway -- <cmd>
```

For iterative local development, start a persistent dev server from the Nix dev shell:

```bash
nix develop
just defe-serve
```

In another `nix develop` terminal, run tests normally:

```bash
cargo test
# or
cargo nextest run
```

The dev shell exports `DEFE_SOCKET="$PWD/.defe.sock"` and sets `DEV_DEFE_SOCKET_PATH` to the same workspace-local socket so tests using `AsyncDefeClient::connect_from_env().await` can connect to the persistent server. `just defe-serve` uses the same default when `DEFE_SOCKET` is unset, then does one initial build and starts `systemfd` socket activation plus `cargo watch` to restart the built `defe` binary with request logging enabled on stderr. It watches `${CARGO_TARGET_DIR:-target}/<profile-dir>` and defaults to `debug`; `CARGO_PROFILE=debug` and `CARGO_PROFILE=dev` both use `target/debug`, while `CARGO_PROFILE=release` or a custom profile uses `target/<profile>`.

Outside this persistent dev-server workflow, crates that use `AsyncDefeClient::connect_from_env().await` need either a running server with `DEV_DEFE_SOCKET_PATH` set, or the command must be wrapped by `defe exec`. The unpublished `tests-e2e` crate contains ordinary, non-ignored cross-project tests using this path. Unlike the opt-in ignored probes above, it is expected to request real relay and push-gateway resources through a running `defe` server. Run it under a one-shot server with:

```bash
target/debug/defe --binary-path target/debug exec cargo test -p tests-e2e
```

Plain workspace test runs outside `defe exec` or `just defe-serve` should exclude this crate, because `cargo test -p tests-e2e` is expected to fail without `DEV_DEFE_SOCKET_PATH`.

## CI packaging

Default `selfci check` includes Rust Clippy, the full workspace and
external-service nextest suite, formatting, lockfile freshness, dependency
source hygiene, release metadata, and Lean proofs.

On Linux, SelfCI builds CI-profile OCI runtime-contract checks for Push Gateway,
Fleet Manager, and FLIP, including the Fleet Manager entrypoint CLI contract.
The FMan and FLIP checks use their `embedded-operator-ui` feature, so Nix must
fetch and build the real operator UI and package it with each daemon. All Rust
compilation remains on the CI profile.

## Image publishing

`.github/workflows/publish.yml` pushes the Fleet Manager, FLIP
liquidity-manager, cloud FMan telemetry and push gateway images to GHCR on
every push to `master`, and on manual `workflow_dispatch`. Publishing remains
separate from SelfCI so a fork pull request can never obtain registry
credentials or write to the registry. Its build phase uses one
system-qualified `release-container-images` Nix target containing all four
release-profile images. SelfCI uses the same image constructors and embedded
UI with CI-profile daemons; publishing is where the optimized Rust graph is
first built, loaded, tagged, and pushed.

Each architecture pushes `<git-sha>-<arch>`; a follow-up job assembles those
into manifest lists published as `<git-sha>` (immutable, what deployments pin
and roll back to) and a moving tag named after the ref (`master` in the usual
case). That job is the sole writer of the unsuffixed tags and requires every
architecture to have succeeded, so a canonical tag is never single-platform, and
it re-reads the registry afterwards to assert the published list serves exactly
the expected platforms. The registry credential is the workflow's own
`GITHUB_TOKEN`; no long-lived keys are stored.

Both `amd64` and `arm64` are built. amd64 runs on the usual `[self-hosted,
linux]` runner; arm64 runs on `linux-arm64-8core`, the same aarch64 runner
`fedibtc/fedi` uses. That runner has no Nix preinstalled (the workflow installs
it). Each publish pushes the built image closures to the public `fedibtc`
Cachix cache, so once those accumulate an unchanged build substitutes instead
of compiling; a cold arm64 leg still compiles the native tree from source and
is budgeted hours rather than minutes.

A manual `workflow_dispatch` run takes two inputs: `architectures` (`both`,
`amd64`, or `arm64` — useful for a quick publish that skips the long arm64
leg) and an optional `extra_tag` published alongside `<git-sha>`.

The images are published to the `ghcr.io/fedibtc/manifold-fman`,
`manifold-flip`, `manifold-cloud-fman-telemetry` and `manifold-push-gateway`
GHCR packages.

The push gateway image carries no dashboard and no embedded UI, so it is the
only one whose contents do not depend on `operator-ui/`. It is the
same `push-gateway-oci-image` the OCI runtime-contract check already inspects,
so publishing adds a destination rather than a new artifact shape.

Each Nix-built daemon binary embeds its own operator dashboard, built from
`operator-ui/`, and serves it from the listener that already carries its
operator API: `/api` for Fleet Manager, `/admin` for FLIP. Ordinary Cargo builds
omit those assets. There is no separate dashboard image and no reverse-proxy
sidecar, because the daemon and its dashboard are one origin by construction —
which is what FMan's same-origin session cookie and FLIP's bearer token need.
The dashboard shell and its hashed assets load before authentication; the API
namespaces keep their existing authentication.

Fleet Manager serves the dashboard only when the deployment enables the operator
HTTP listener; see [`packages/fleet-manager`](./packages/fleet-manager/README.md).
