# tests-e2e

`tests-e2e` is an internal crate for general-purpose end-to-end tests that
exercise multiple workspace components together. It is part of the workspace so
it can use the same local crates, but it is not published.

Current tests cover normal `defe-client` usage against a real `defe` server.
They are ordinary, non-ignored Cargo integration tests. They expect a running
`defe` server and may request real Nostr relay and push-gateway resources
through that server, so build the resource binaries first and run them either
under a one-shot server:

```bash
cargo build -p defe -p defe-client -p fedi-decentralized-push-gateway
target/debug/defe --binary-path target/debug exec cargo test -p tests-e2e
```

or from another terminal while the persistent development server is running:

```bash
just defe-serve
cargo test -p tests-e2e
```

Plain workspace checks that do not start `defe` should exclude this crate, for
example `cargo test --workspace --exclude tests-e2e`.

## Manifold Guardian 0.1 formation gate

`tests/manifold_guardian_0_1_formation.rs` is the concrete 7-MG release-gate
harness. It is checked in as a normal Cargo integration test, but it is a no-op
unless explicitly enabled:

`fedimintd` and `fedimint-cli` are not workspace crates; supply the
flake-pinned builds (the same ones the CI test derivation and the OCI image
use). The ordinary `nix develop` shell does *not* export the `MG_E2E_*`
variables — only the `.#ci.<system>.tests` derivation does — so set them
yourself:

```bash
cargo build -p defe -p mg -p fa-cli
MG_E2E=1 \
  MG_E2E_FEDIMINT_CLI_BIN="$(nix build --no-link --print-out-paths .#fedimint-cli)/bin/fedimint-cli" \
  target/debug/defe --binary-path target/debug exec \
  cargo test -p tests-e2e manifold_guardian_0_1_forms_seven_guardian_federation_under_defe -- --nocapture
```

The harness allocates an exclusive `defe` regtest bitcoind resource, starts seven
`fleet-manager` daemons onboarded with test PeerBadges on an exclusive `defe`
Nostr relay, and runs `fa-cli`'s verified registry selection over their
advertisements. Advertisements name only Iroh endpoint ids, so the harness
passes each daemon's logged loopback addresses in `MG_E2E_MG_ENDPOINT_ADDRS`. It then uses `fedimint-cli join-federation` to prove the returned
invite code is joinable, and checks every guardian's committed consensus config
contains exactly `mintv2`, `walletv2`, `lnv2`, `meta`, and the Manifold
stability-pool module. It also parses every retained safe-event journal record,
checks the typed marker and absence of span context, enforces the production
segment count/size and Unix permission bounds, and proves both every MG and
its embedded `fedimintd` wrote the expected StartDKG durability, consensus,
invite, peer-checksum, module-generation, and configuration-persistence
milestones. Each daemon runs the `fedimintd`
compiled into its own binary, so only `fedimint-cli` has to come from outside;
if the `MG_E2E_*` vars are unset the harness falls back to `target/debug` and
`PATH`.
The full set of overrides:

- `MG_E2E_MANIFOLD_GUARDIAN_BIN`
- `MG_E2E_FA_CLI_BIN`
- `MG_E2E_FEDIMINT_CLI_BIN`
- `MG_E2E_BITCOIN_CLI_BIN` (paid test only)
- `MG_E2E_ESPLORA_BIN` (paid test only; an esplora/electrs binary)

The same file carries the paid gate
(`manifold_guardian_0_1_paid_formation_settles_real_ecash_under_defe`, same
`MG_E2E` opt-in): seven MGs first form a free federation, which then serves
as the OOB ecash payment federation for a second, paid (`InfiniteBestEffort`)
formation. The separate formation gate retains the seven-guardian scale
coverage, while this gate covers the money lifecycle — the harness leases a
defe Nostr relay, points the MG at it through the development-only
`MANIFOLD_DEV_NOSTR_RELAYS`/`MANIFOLD_DEV_SETUP_PAYMENT_PUBLISHER` overrides,
and publishes a signed kind-37707 setup-payment event naming the free
federation; the MG admits it and its wallet joins on its own, with the test
polling `payment-federations list` until the member is accepted and
receivable. An on-chain peg-in funds the FA wallet (hence `bitcoin-cli`). The
harness passes the same signed event file and pinned publisher key to
`fa-cli`; the client authenticates and persists the policy before requesting
paid quotes. The harness exports exactly one large mint-v2 ecash note and
passes it through a
mode-0600 funding-token file which `fa-cli` moves into its restart journal
before import and deletes only after the wallet confirms receipt;
the same live client joins and receives, then repeats the ordered per-seat loop:
submit one signed quote payment, wait for its payer change to become spendable,
call `CreateSeat`, and checkpoint that accepted seat before starting the next
payment. If the initial exact aggregate reservation reports the recoverable
pre-output readiness failure, the harness preserves the FA state and wallet
and proves one `resume` reaches the same formation rather than starting over.
DKG begins only after every seat completes that loop. The gate reconciles the
final payer balance as setup prices plus Fedimint fees plus returned spendable
change, and waits for an accepted payment to settle into an MG wallet. The
harness also runs an esplora indexer
over the defe bitcoind (hence the esplora binary): the Fedi fedimint
(`v0.11.2-fedi4`) wallet *client* can only watch the chain through esplora, so
the FA wallet's peg-in would never confirm without one.

The Linux-only
`fa_client_resumes_real_dkg_after_sigkill_under_defe` gate converts the
crash-during-DKG coverage into the same Rust/`defe` runtime. It kills three
validated `fedimintd` child PIDs after their safe journals show DKG began,
sends SIGKILL to the exact live `fa-cli` PID, confirms a fresh CLI process
reopens durable guardian-code preparation, waits for the abandoned database
lease, and resumes the same formation through an idempotent `StartDkg` replay
to a persisted invite. It then requires each killed guardian to have observed
both its original and replacement starts while every intact guardian has
observed at least its original start. The same run
passes one mode-0600 callback bearer file into `fa-cli`, returns HTTP 500 for
all first attempts, restarts one MG with its pending callback row, then
recovers the endpoint and requires all seven callbacks to reach `delivered`
with the FA's unchanged idempotency key. The test skips on non-Linux hosts
because direct child validation uses `/proc` rather than an unsafe process-name
or process-group signal.

The Linux-only
`mg_recovers_a_real_child_and_terminalizes_data_loss_under_defe` gate forms
the same real seven-guardian federation, kills one exact `fedimintd` child by
pidfd, and requires the seat loop's replacement to serve a guardian-policy API
read and participate in a verified metadata-consensus update. It then removes
only that temporary seat's final data after clean shutdown, requires restart to
project `data_loss`, decommissions the seat twice to prove idempotency, and
restarts once more to prove the terminal fact persists without spawning a
child.

`mg_configures_guardian_fees_and_registers_gateway_under_defe` forms another
real federation, configures the canonical guardian-fee split through `fa-cli`,
carries that policy through a verified generic metadata update, and requires
every guardian to observe its exact share. It also registers then idempotently
replays one FA-signed gateway URL against every real MG and embedded
`fedimintd`. Against one guardian it reads and no-op collects the fresh
stability-pool remittance account through the real `mg-fedimint` wallet,
then exercises the dedicated telemetry ALPN: invalid and valid bearer checks,
formed-seat discovery, raw metrics, safe-journal discovery and fetch, and
immediate global bearer rotation through the operator socket.

The harness gives every Iroh endpoint a deterministic loopback route, so all
formations also run in network-isolated Nix builds without public relay or DNS
access. Its private, collision-safe temporary directory uses a compact name so
the nested MG admin sockets remain below Darwin's Unix-domain socket path
limit. SelfCI and `just test-e2e-local` run all five formation tests through `defe`
alongside the rest of the E2E suite.

If `fedimintd` or a real bitcoind-capable `defe` server is unavailable, leave
`MG_E2E` unset; the test will report that it was skipped instead of
pretending the formation gate ran.

## Guardian transfer

`fa_moves_a_formed_guardian_to_a_new_mg_under_defe` (same `MG_E2E` opt-in, in
`tests/manifold_guardian_0_1_formation/guardian_transfer.rs`) drives a
guardian transfer only through `fa-cli`, as a consumer would. Seven MGs form
a free federation, and an eighth MG joins. With the source MG offline,
`fa-cli transfer start` buys a free seat on the new MG and asks it to pull
the guardian of seat 0. The FA then stays offline: the new MG keeps its pull
across a restart and runs the guardian, with the source's config, once the
source is back. `fa-cli transfer resume` then completes the move. The test
checks that:

- the source deleted the guardian data, reports `Removed`, stays removed
  after a restart without a `fedimintd` child, and keeps its payment wallet;
- the consensus seat bindings name the new MG instead of the old one;
- a metadata update runs through all seven guardians;
- a restore of the new MG from its mnemonic and the relay runs the same
  guardian;
- with two other MGs stopped, a consensus read still returns the update, so
  the moved guardian serves current consensus.

A moved guardian keeps its Iroh ids but listens on the new seat's ports. This
harness routes Iroh ids through static overrides, so the new MG starts with a
route to those ports, and the MGs that stay restart with it. The source MG
pins its service endpoint when it restarts, so the new MG has its route in
advance.

## Advertisement trust flow

`tests/mg_holder_authorization_flow.rs` uses a defe-managed Nostr relay for
the issuer → holder authorization → advertisement → FA selection path. Alongside
seven valid advertisements it publishes malformed JSON, a missing holder
authorization, an unsupported federation size, and a cheapest advertisement
that reuses another MG's badge. Discovery reports the three static admission
failures; lazy badge verification rejects the stolen badge for subject mismatch
and still selects exactly the seven valid operators.


## MG release upgrade qualification

Following Fedimint's binary-path upgrade tests, the ignored
`mg_upgrades_existing_guardians_wallet_and_pending_payout_under_defe` test
creates a seven-guardian federation with the old MG executable, funds and
collects a real guardian-fee remittance, interrupts an outgoing payout after
its native commit, and replaces all seven executables without replacing their
data directories. It checks durable identity and seat records, payout recovery
and replay without a second invoice, all guardians returning healthy, and
preserved fee policy/account/history. These are release binaries with isolated
development/regtest resources, not production data or platform-device tests.

Run the last production 0.1.1 image against the proposed 0.1.2 image:

```bash
nix develop --command scripts/test-mg-upgrade.sh \
  d87b6dd2d984b3f15bc214718debfcc9648654d8 \
  395e3b6ab673b51f00a949644d641f6697ebdb13
```

The script reads public images, extracts their actual native executables, and
runs the existing Defe harness locally. It needs Linux, Docker, and executable
Nix runtime dependencies; it does not deploy or publish anything. The old
formation uses the 0.11 guardian range. This expensive qualification is separate
from routine SelfCI. A failed run is not upgrade approval; retain its test data
and logs to diagnose the unsupported transition.
