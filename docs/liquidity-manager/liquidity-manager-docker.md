# FLIP Liquidity Manager Docker Image

This document covers the first Docker packaging path for the FLIP Liquidity
Manager daemon. Umbrel and StartOS packaging are separate later phases.

## Build And Load

Build the image tarball with Nix:

```bash
just flip-docker-build
```

Load it into the local Docker image store:

```bash
just flip-docker-load
```

The image name is `flip-liquidity-manager:<workspace-version>` (`flip-liquidity-manager:0.1.0` today); `just flip-docker-run` derives the tag from `Cargo.toml`.

## Run Locally

The local recipe starts the daemon with a persistent data directory under
`/tmp/flip-liquidity-manager-docker-data`:

```bash
FLIP_BOOTSTRAP_ADMIN_TOKEN=change-me \
  FLIP_MANIFOLD_ENVIRONMENT=development \
  just flip-docker-run
```

The recipe maps both daemon ports to loopback:

- `127.0.0.1:8173` -> private Operator Admin API (TCP/HTTP)
- `127.0.0.1:8174` -> Public Liquidity API (Iroh over QUIC — **UDP**, not HTTP)

For a manual run:

```bash
mkdir -p /tmp/flip-liquidity-manager-docker-data
docker run --rm \
  -e FLIP_BOOTSTRAP_ADMIN_TOKEN=change-me \
  -e FLIP_MANIFOLD_ENVIRONMENT=development \
  -p 127.0.0.1:8173:8173 \
  -p 127.0.0.1:8174:8174/udp \
  -v /tmp/flip-liquidity-manager-docker-data:/var/lib/flip \
  flip-liquidity-manager:0.1.0
```

The image defaults are:

- `FLIP_DATA_DIR=/var/lib/flip`
- `FLIP_ADMIN_BIND_ADDRESS=0.0.0.0:8173`
- `FLIP_PUBLIC_BIND_ADDRESS=0.0.0.0:8174`
- `SSL_CERT_FILE` points at the bundled CA certificate bundle

The image deliberately has no `FLIP_MANIFOLD_ENVIRONMENT` default. Every
normal and restore invocation must select `development`, `staging`, or
`production` explicitly.

`FLIP_BOOTSTRAP_ADMIN_TOKEN` is intentionally not baked into the image. Set it
at runtime for Docker and bare-host deployments. It is a bootstrap credential:
once `POST /admin/v1/rotate_admin_token` has run, the rotated token replaces it
and the boot value stops being accepted, so it does not have to stay in
deployment wiring.

Retain it anyway, and keep treating it as a live secret. "Stops being accepted"
holds for a running daemon that can read its rotated token. It does not hold
against a restart with the break-glass flag below, which accepts the boot value
again — that is the documented way back in when the secret store is unreadable,
and it needs the original token. A rotation retires the bootstrap token from the
request path, not from the deployment. See the credential-rotation boundary in
[`SECURITY.md`](../../SECURITY.md).

There is a single auth/verification flow: without a provider signing key the
daemon boots unconfigured and fails closed, and its public Iroh transport
waits rather than binding. Supply the key either at boot
(`-e FLIP_PROVIDER_NOSTR_SECRET_KEY=<hex>`) or afterwards through
`POST /admin/v1/install_provider_identity` — no restart either way. The public
endpoint identity is derived from that key, so it is the same on every
subsequent start and the advertised address never needs re-applying. Test
deployments may substitute the federation preview and
FMan trust-material verification inputs with local fixture files via
`-e FLIP_TRUST_FIXTURES=<dir>` (`--trust-fixtures`); fixture mode refuses
Bitcoin mainnet configurations.

## Persistent State

Mount `/var/lib/flip` as a persistent volume. It contains SQLite state,
provider identity material, the generated local secret-store key when no
`FLIP_SECRET_KEY` is supplied, target-federation client storage, chain-observer
cursors, relay cursors, and operation history.

Treat this volume as sensitive. Admin API backups are unencrypted
gzip-compressed tar archives of this data directory and may include the
generated `secret-store.key` needed to decrypt local secret records.

## Backup And Restore

Create a backup through the private Admin API:

```bash
curl -sf \
  -H "Authorization: Bearer $FLIP_BOOTSTRAP_ADMIN_TOKEN" \
  -X POST \
  http://127.0.0.1:8173/admin/v1/create_backup
```

The response includes a local `archive` path under `/var/lib/flip/backups/` and
a `manifest`. The `backups/` directory is excluded from archive payloads so a
backup does not recursively include earlier backups.

There are two ways to restore, and they are for different situations.

### Restore onto the running daemon

Rolling a live deployment back to one of its own backups needs no restart. Call
restore against the normal Admin API:

```bash
curl -sf \
  -H "Authorization: Bearer $FLIP_BOOTSTRAP_ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"archive":"/var/lib/flip/backups/flip-backup.tar.gz"}' \
  http://127.0.0.1:8173/admin/v1/restore_backup
```

The archive is extracted and validated before anything is replaced, so a bad
archive is rejected with the daemon still serving its current state. Once it
passes, the response returns and the daemon rebuilds itself against the
restored data directory. During that window `/admin/v1/*` returns 503 and
`/health` reports the restore; poll `/health` until it clears:

```bash
curl -sf http://127.0.0.1:8173/health
```

The state the restore displaced is moved to a `.<data-dir>.pre-restore-<ts>`
directory beside the data directory rather than deleted, and a restored state
that fails to start is rolled back to it automatically. Remove it yourself once
you are satisfied with the restore. Existing archives under `backups/` stay
where they are.

Because the public node id is derived from the provider identity, this path
refuses an archive whose provider identity differs from the running daemon's —
adopting another provider's state would move the daemon to an address its
published advertisements do not name. Use restore mode for that.

The live path also refuses an archive that omits or replaces any allocation
already accepted by the running daemon. Such an archive would erase the
idempotency record while external funding authority may already have acted.
Keep the current generation running, or choose an archive containing its
accepted allocation history. The refusal is a `failed_precondition` response
and happens before teardown.

Both paths also refuse an archive whose secret records cannot be decrypted with
the key the daemon will use. That happens when the backup was written under a
different `FLIP_SECRET_STORE_KEY`; without the check the restore would land and
the daemon would come up unable to read any secret, including the admin token,
locking you out of the API.

**A restore also restores the admin token.** It is stored with the other
secrets, so an archive predating a token rotation brings the older credential
back with it, and the token you are using now stops working when the daemon
comes back up. Keep the token that was current at backup time, or rotate again
afterwards using it.

### Locked out of the Admin API

A rotated admin token replaces the bootstrap token outright. If the secret
store becomes unreadable — a bad disk, or a `FLIP_SECRET_STORE_KEY` that does
not match the stored records — the daemon cannot read that token and answers
every authenticated route with 500 rather than falling back, so that breaking
storage is not a way to re-enable a credential you retired. Unauthenticated
`GET /health` keeps working.

To get back in, restart with the break-glass flag, which accepts the bootstrap
token again:

```bash
docker run --rm \
  -e FLIP_BOOTSTRAP_ADMIN_TOKEN=change-me \
  -e FLIP_ALLOW_BOOTSTRAP_TOKEN_FALLBACK=1 \
  ...
```

It only applies while the rotated token cannot be read at all, and it logs a
warning on every request it lets through. Fix the storage — usually by
restoring a backup — then restart without it. Requiring a restart is the point:
recovery should need control of the deployment, not just access to the port.

### Restore onto a fresh host

For disaster recovery, where there is no running daemon to restore onto, stop
the normal daemon and start the image in restore mode with an empty mounted
target data directory:

```bash
docker run --rm \
  -e FLIP_BOOTSTRAP_ADMIN_TOKEN=change-me \
  -e FLIP_MANIFOLD_ENVIRONMENT=production \
  -e FLIP_RESTORE_MODE=1 \
  -p 127.0.0.1:8173:8173 \
  -v /tmp/flip-liquidity-manager-restored:/var/lib/flip \
  -v /path/to/backups:/restore-backups:ro \
  flip-liquidity-manager:0.1.0
```

Then call restore against the restore-mode Admin API, which requires the target
data directory to be empty:

```bash
curl -sf \
  -H "Authorization: Bearer $FLIP_BOOTSTRAP_ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"archive":"/restore-backups/flip-backup.tar.gz"}' \
  http://127.0.0.1:8173/admin/v1/restore_backup
```

After restore returns, stop the restore-mode container and start the normal
daemon using the restored data directory. Restore mode does not start public
transport, background workers, or the normal live SQLite handles.

Unlike live restore, fresh-host restore has no newer running generation from
which to detect allocation rollback. Selecting the archive and reconciling it
with external funding history are operator recovery responsibilities.

## Health And Exposure

The image defines a Docker healthcheck against:

```text
http://127.0.0.1:8173/health
```

`/health` is unauthenticated and contains only daemon health summary data.
Protected Admin routes under `/admin/v1/*` require:

```text
Authorization: Bearer <FLIP_BOOTSTRAP_ADMIN_TOKEN>
```

The Operator Admin API is a private administration surface. Map port `8173`
only to loopback or private package networking unless an explicit external
access control layer is in front of it.

The Public Liquidity API is separate from the Admin API. Operators control
whether port `8174` is published outside the host. A mapping that publishes it
must be UDP: the listener is an Iroh endpoint over QUIC, so `-p 8174:8174`
publishes TCP, reaches no listener, and reports no error. The daemon still
readiness gates public request handling and public advertisement publishing
based on setup, dependency validation, and startup recovery.

External gatewayd and chain-observer dependencies are configured through the
Admin API setup flow, not through Docker image defaults.

## E2E And Packaging Tests

Required SelfCI packages the real operator UI with a CI-profile FLIP daemon and
checks the OCI runtime contract. Publishing uses the same image constructor
with the release-profile daemon through `release-container-images`; the
optimized image is first built in the trusted publish workflow. The Docker smoke
below remains the optional execution check.

Run the FLIP liquidity-manager E2E suite:

```bash
just flip-test-integration
```

The recipe starts an ephemeral `defe` server and runs the daemon smoke,
real-relay, and live liquidity test binaries serially through nextest. The suite
leases an exclusive Nostr relay and exclusive regtest Bitcoin Core node from
`defe`; it starts the locked Fedimint `gatewayd`, target `fedimintd`, and the
locally compiled FLIP daemon as native loopback processes. FLIP's Iroh server
publishes a direct loopback address for these tests without requiring a public
relay connection.

The `DEV_DEFE_SOCKET_PATH` environment set by `defe` caps the daemon's
production background-worker intervals at 100 milliseconds; the harness polls
external test state at the same interval. Production polling intervals are
unchanged. The same signal disables Iroh's public relay transport for these
loopback-only tests, avoiding external relay selection and dependency.

The Nix development shell supplies the relay, Bitcoin Core, gateway, and
Fedimint binaries. CI gives each long FMan and FLIP test its own `defe exec`
runner. Linux Nix sandboxes isolate their loopback networks and run the
partitions in parallel; Darwin shares host loopback, so CI orders its partitions
to prevent independent port-allocation ledgers from racing. `just
test-e2e-local` schedules them concurrently in one nextest phase, where one
allocator ledger coordinates the processes. Exact binary paths come from the
locked Nix inputs. `defe` isolates each test's resources within those scheduling
boundaries.

The daemon smoke integration test does not run Docker. It starts the compiled
`liquidity-manager-daemon` binary with random loopback ports, checks private
health and Admin API auth behavior, applies dummy setup state, verifies secret
redaction and encrypted SQLite persistence, creates and inspects a backup,
restores it into a fresh data directory through restore mode, restarts the
normal daemon from restored state, confirms setup state persists, and verifies
startup recovery health counts for seeded active durable work.

The Docker image smoke test remains an explicitly selected packaging check:

```bash
cargo test -p fedi-decentralized-liquidity-manager-daemon \
  --test integration_docker_image -- --ignored --nocapture
```

It builds
`path:.#liquidityManagerDockerImage`, loads `flip-liquidity-manager:<workspace-version>` into
the local Docker image store, starts the image with random loopback Admin and
Public ports, checks Admin API bearer auth and health, and waits for the image
health check to become healthy.
