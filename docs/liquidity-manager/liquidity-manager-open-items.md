# FLIP Liquidity Manager Open Items

What is **not** implemented or **not** decided for FLIP: open spec decisions,
implementation gaps, and cross-component items.

This document tracks no history. An item is deleted when it lands, not archived
here. It also carries **no verdicts, dates, rulings, or record tallies**: those
live in the `CLAIM-*` records and
[`ARCH-liquidity-manager`](../../crates/liquidity-manager-daemon/specs/ARCH-liquidity-manager.md)
under [`specs/`](../../crates/liquidity-manager-daemon/specs/), and a copy of
them here drifts out of agreement with the records within days. Cite a record;
do not transcribe it.

## Spec open items

### Shared publication profile follow-ups

FLIP follows the shared FMan-side registry and attester publication conventions
([`SPEC-fman-nostr-events`](../../crates/nostr/specs/SPEC-fman-nostr-events.md),
[`SPEC-holder-trust-envelope`](../../crates/domain/specs/SPEC-holder-trust-envelope.md),
[`SPEC-advertisement`](../../crates/fman/specs/SPEC-advertisement.md)). Three
follow-ups remain. None blocks MVP.

- Event kind numbers stay provisional across FMan and FLIP. FLIP tracks the
  shared decision and renumbers with it.
- Cross-language conformance and test vectors for the adopted schema and
  publication profile. Tracked with the signing item below.
- Whether `fedi-credential-sdk-protocol::Revocation` grows optional reason or
  status fields. This bullet is the only trace of the question left in the tree.
  The SDK owner must either record the decision or record that the question is
  open, because the tree cannot answer it.

### Canonical FI signing profile

FLIP's own signing profile is settled and implemented. The canonical FI signing
byte layout and domain tags remain unpinned.

- Add cross-language conformance fixtures with fixed expected byte strings,
  digests, and signatures. `crates/domain/conformance` holds only
  `federation-config-hash-v1.json` and `fman-seat-bindings-v1.json`.
- Finish the endpoint actor and requester transport-binding policy once the
  shared production trust profile is complete. The server verifies each request
  signature against its declared `requester_pubkey`. No rule binds that key to
  the authenticated Iroh transport actor.
  [`SPEC-flip-rpc`](../../crates/liquidity-manager-daemon/specs/SPEC-flip-rpc.md)
  records the divergence in its `Status`.
- Add negative fixtures for provider and requester binding failures. They
  depend on that final policy.

## Implementation gaps

### Target-client value is not swept back to the provider wallet

The sweep is **not being built**. Manual recovery through
[`liquidity-manager-recovery-runbook.md`](./liquidity-manager-recovery-runbook.md)
is the intended route.

The capacity half is solved: `abandon_target_client_value` fails the item and
releases its reservation. What the decision accepts is the value itself when a
stability pool rejects provision permanently. Recovering it is a peg-out from
the target federation, which needs its own send-once fence, durable operation
records, and settlement evidence. `WalletClientModule::withdraw` mints its own
operation id, so it inherits the same pre-submit crash window, and there the
value moves outward: a lost id followed by a resubmit is a double send.

**The accepted limitation, in operator terms.** Severity: High. An FI can use an
endorsed federation whose stability pool rejects provision, and so lock
FLIP-funded e-cash. What that FI cannot do is consume provider capacity
permanently. Two records point here for this statement:
[`failed-stability-allocation-strands-ecash`](../../crates/liquidity-manager-daemon/specs/CLAIM-failed-stability-allocation-strands-ecash.md)
and
[`stability-deposit-rejection-releases-capacity`](../../crates/liquidity-manager-daemon/specs/CLAIM-stability-deposit-rejection-releases-capacity.md).

**Open work: rehearse the runbook against a live deployment.** Accepting a
manual route means the manual route must work, and it has never been exercised.
The rehearsal needs a real stability-pool federation and an item driven into the
abandoned state.

If a sweep is ever built, do **not** restore automatic submission from aggregate
balances or from an absent operation id. That is the duplicate-deposit hazard
these paths fail closed against.

### Retained target-client databases are unbounded

Nothing deletes `federations_dir/<federation_id>/`, so one RocksDB per distinct
federation an FI gets endorsed accumulates on disk for the life of the
deployment.

This is the accepted consequence of the decision above: that database is what
the manual recovery route reads after `abandon_target_client_value`, so deleting
it is how abandoned value stops being recoverable at all.

An operator sizing a FLIP host must plan for an on-disk set that never shrinks.
The growth rate is set by how fast an FI can obtain endorsements for distinct
qualifying federations, not by anything FLIP configures. **No verdict will go red
if disk growth becomes the binding constraint.** Only this item carries it. If
the release envelope ever measures the growth rate, that measurement is what
should gate a deployment.

### A stuck target-client open holds its slot for the life of the process

Pending opens have their own budget of four, separate from the client ceiling.
A stuck open never terminates, because the `api_version` negotiation loop cannot
be bounded at that layer, so it holds a pending slot until restart. **Four
targets that serve their config and then stop answering fill the budget
permanently**, after which FLIP opens no further target client. Installed clients
keep working. Recovery is a restart; nothing in the Admin surface reclaims a
pending open. Filed as
[`pending-open-budget-wedges-target-clients`](../../crates/liquidity-manager-daemon/specs/CLAIM-pending-open-budget-wedges-target-clients.md).

The fault is reported and attributable: a pending open past five minutes logs
once at `warn` with its federation id and age, a capacity refusal names every
occupying federation oldest first, and the `target_client_pool` health component
carries the standing occupancy — installed clients against their ceiling,
pending opens against their budget, how many have passed the stuck threshold,
and the oldest occupants by age. It warns on a full budget and goes unhealthy
only on the wedge, meaning a full budget whose every occupant is stuck, which is
the state a restart is the sole exit from.

**The real fix is still a pinned-Fedimint change** bounding api-version
negotiation. It closes the original unbounded wait as well. Until then an
operator alerts on the health component and restarts; nothing in the Admin
surface reclaims a pending slot.

### Backup archives are not authenticated

An archive carries `backup-checksums.json`, a SHA-256 digest per archived file,
and restore verifies every one. That closes **accidental corruption only**.

The digests are stored inside the archive they describe, so a writer who can
modify the archive recomputes them to match. A successful restore establishes
that the archive is internally consistent, not that FLIP wrote it. This is what
underwrites the "hostile write to the database" traces that several records treat
as reachable: a hostile edit to a restored SQLite file stays undetected.

Closing it needs a signature or a MAC over a key the archive does not carry.
That raises three unanswered questions:

- where the key lives so that it survives the disaster the backup exists for
- whether a fresh-host restore can verify at all, when the key was in the data
  directory that was lost
- what an operator does with an archive that fails verification during a real
  outage

**Archives are also unencrypted.** They hold identity secrets and possibly the
local secret-store key. Custody is the only control, and the checksums do not
change that. `SECURITY.md` states the same boundary.

### The shared peer-badge verifier is built and never consumed

`main` constructs a `PeerBadgeVerifier` from the selected environment profile,
`run_daemon` checks its provenance, and `DaemonContext` retains it. Nothing reads
it; `run_daemon`'s own doc comment says the verifier "is intentionally not
invoked yet".

The selected profile governs the minimum-level semantics on FLIP's direct
envelope path through `PeerBadgeTrustPolicy`, so those cannot drift from the
verifier-backed ones. It does not govern the issuer set: `verification.rs` reads
trusted issuer authorities from `attestation_store`, which an operator installs.

[`SPEC-peer-badge-verifier`](../../crates/peer-badge-verifier/specs/SPEC-peer-badge-verifier.md)
and `ARCH-liquidity-manager` both record the gap, so the state is intended. The
integration is what is missing.

### Admin verbs the operator dashboard does not reach

The dashboard covers most of the Admin API, including `retry_funding_step` and
`cancel_allocation`, so an operator works from it rather than from `curl`. Two
groups do not reach it. Regenerate the sets from
`crates/liquidity-manager-daemon/src/admin.rs` against `operator-ui`
rather than trusting the lists below.

**Three verbs have types and no screen** — `get_verification_summary`,
`get_holder_authorization_state`, and `refresh_holder_authorizations`. All three
are pure views. Building the screen is what is left.

- `get_verification_summary` returns the per-federation trust verification
  summary that decided admission. It reads beside the allocation detail view, and
  it is the one absent verb an operator would consult routinely rather than
  during an incident.
- `get_holder_authorization_state` reports which Holder authorizations FLIP
  enrolled from the relay and what the last relay read concluded. It answers "why
  is my advertisement not carrying a badge", which is a setup question. It
  reports an empty identity rather than failing before the provider identity is
  installed, precisely so a console can poll it throughout setup.
  `refresh_holder_authorizations` re-reads the relay and returns the same state,
  so it is the button beside that view.

**Six verbs appear nowhere in `operator-ui`**, not in a feature and not in
`packages/types`. That last part is worth stating plainly, because `operator-ui`
describes `packages/types` as mirroring the Rust admin surface verb for verb.
Adding the types is the concrete first step.

- `inspect_target_client` — the natural first piece: the only pure view among the
  recovery verbs, and the one whose output an operator most needs to read, since
  the other two target-client operations are decisions taken *from* it
- `bind_target_deposit`
- `abandon_target_client_value` — the one that needs care in a UI. It fails an
  item and writes off FLIP's ability to manage funds it already sent, so it wants
  a confirmation step and the abandoned amount shown before the operator commits,
  not a button beside the others
- `install_provider_identity`, `reopen_federation_client`, `rotate_admin_token` —
  whether these belong in a browser has not been recorded either way. A decision
  to keep credential and runtime-surgery operations out of a browser is
  reasonable; it just needs writing down, because the omission is currently
  indistinguishable from an oversight

Note that the `operator-ui` toolchain (`pnpm`, `node_modules`) is not installed
in every development environment.

### Periodic workers have no per-instance phase offset

Every periodic worker runs through `run_interval_task` in `lib.rs`, which builds
a `tokio::time::interval`. That fires immediately and then holds an exact fixed
period. A fleet restarted together keeps hitting shared relays and dependencies
in recurring bursts, and the four workers inside one daemon tick in step with
each other.

This looks like a one-function change and is not. Three things decide it.

- **Where the per-instance seed comes from.** The crate has no `rand` dependency,
  `daemon_metadata` carries no instance id, and the data directory path is
  typically identical across containerised deployments. The only distinct value
  available is the provider pubkey through `identity::find_provider_identity`,
  which is absent early in startup, so the offset needs a lazy computation with a
  fallback and would not upgrade after an operator installs an identity without a
  restart. The alternative is a new `daemon_metadata` instance-id row.
- **Whether within-daemon de-phasing is worth doing on its own.** An offset
  derived from the worker name needs no identity, no dependency, and no
  migration, and is unconditionally safe. It stops the four workers in one daemon
  ticking together, and does nothing for the fleet case.
- **Placement.** The first pass must stay immediate and the schedule must shift
  afterwards. Sleeping inside the `select!` arm makes shutdown unresponsive for
  the length of the offset; avoiding that means restructuring the loop around
  `interval_at`. Four production workers run through this path, and a mistake
  there is a worker that stops ticking or stops answering shutdown.

Randomized failure backoff is the same problem on the retry path, and is a
separate, larger change.

## Upstream dependency gaps

### A pin bump can silently reinstate a repaired defect

`repeated-target-peg-in-allocation-after-crash` passes because upstream
`allocate_deposit_address_pooled_stateless` reuses an address while
`claimed.is_empty()`. Changing `<=` to `<` in `fedimint-wallet-client`'s
pooled-address reuse is **one character, no compile error, no failing test**, and
it reinstates the defect. The per-item budget ruling rests on that repair, so
treat a pin bump touching that allocation as touching
`stability-deposit-terminal-state-not-observed` too.
