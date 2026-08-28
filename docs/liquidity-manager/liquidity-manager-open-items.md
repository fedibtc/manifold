# FLIP Liquidity Manager Open Items

What is **not** implemented or **not** decided for FLIP: open spec decisions,
implementation gaps, and cross-component items.

This document tracks no history. An item is deleted when it lands, not archived
here. It also carries **no verdicts, dates, rulings, or record tallies**: those
live in the records under
[`claims/`](../../crates/liquidity-manager-daemon/claims/) and
[`specs/`](../../crates/liquidity-manager-daemon/specs/ARCH-liquidity-manager.md),
and a copy of them here drifts out of agreement with the records within days.
Cite a record; do not transcribe it.

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
- Add Phase 11B negative fixtures for provider and requester binding failures.
  They depend on that final policy.

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
[`failed-stability-allocation-strands-ecash`](../../crates/liquidity-manager-daemon/claims/failed-stability-allocation-strands-ecash.md)
and
[`stability-deposit-rejection-releases-capacity`](../../crates/liquidity-manager-daemon/claims/stability-deposit-rejection-releases-capacity.md).

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
[`pending-open-budget-wedges-target-clients`](../../crates/liquidity-manager-daemon/claims/found-bugs/pending-open-budget-wedges-target-clients.md).

The fault is reported and attributable: a pending open past five minutes logs
once at `warn` with its federation id and age, and a capacity refusal names every
occupying federation oldest first.

Two things remain open.

- **The real fix is a pinned-Fedimint change** bounding api-version negotiation.
  It closes the original unbounded wait as well.
- **These are log lines, not a metric.** FLIP has no admin verb or gauge exposing
  pool occupancy, so an operator alert has to come from log matching. Worth
  adding when FLIP gains a metrics surface; not worth inventing one for this
  alone.

### Concurrent restore-mode restores can merge two archives

`restore_backup` checks the data directory is empty, stages the archive, checks
again, and then moves the staged contents in. Staging is safe: `staging_dir_for`
carries the pid and a unique suffix, so each call stages into its own directory.
**The unguarded window is between the second check and the move.** Two concurrent
authenticated calls can both pass the check and then both move, and
`move_staged_contents` renames entries into the data directory with no
predicate, so the result is two archives merged into one data root.

This is the **restore-mode** verb, gated on `args.mode == DaemonMode::Restore`.
The live-restore path is separately serialised: `DaemonShell::queue_restore`
refuses with "another live restore is already pending" while it holds the
pending-restore lock.

No claim record covers it. It attacks restore atomicity rather than confinement,
which is what
[`restore-mode-starts-generation-machinery`](../../crates/liquidity-manager-daemon/claims/restore-mode-starts-generation-machinery.md)
and
[`unauthenticated-admin-reaches-privileged-effect`](../../crates/liquidity-manager-daemon/claims/unauthenticated-admin-reaches-privileged-effect.md)
cover. Filed as
[`concurrent-restore-merges-two-archives`](../../crates/liquidity-manager-daemon/claims/found-bugs/concurrent-restore-merges-two-archives.md).

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
`cancel_allocation`, so an operator works from it rather than from `curl`. Three
groups do not reach it. Regenerate the sets from
`crates/liquidity-manager-daemon/src/admin.rs` against `operator-ui`
rather than trusting the lists below.

**One screen has a dead path, and this is the one that strands money.**
`ManualReviewPanel` documents itself as "the only exit from manual review, inside
the product", and its `completed` outcome now fails: the daemon refuses a
`completed` resolution that lacks chain evidence and directs the operator to
`complete_review_without_evidence`. `packages/types` carries that verb's request
and response types; no hook or screen calls it. An operator meeting a reviewed
send with no chain evidence has no route out inside the product.

**Four verbs have types and no screen** — `get_verification_summary`,
`get_holder_authorization_state`, `refresh_holder_authorizations`, and
`complete_review_without_evidence` above. The first two are pure views. Building
the screen is what is left.

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
each other. Reported by the FLIP underwriting review
([#334](https://github.com/fedibtc/manifold/pull/334)).

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
  ticking together, and does nothing for the fleet case the finding describes.
- **Placement.** The finding asks for the first pass to stay immediate and the
  schedule to shift afterwards. Sleeping inside the `select!` arm makes shutdown
  unresponsive for the length of the offset; avoiding that means restructuring
  the loop around `interval_at`. Four production workers run through this path,
  and a mistake there is a worker that stops ticking or stops answering shutdown.

Randomized failure backoff is part of the same finding and is a separate, larger
change.

### Packaging beyond Docker

Umbrel and StartOS packaging is planned and unimplemented. The Docker image in
[`liquidity-manager-docker.md`](./liquidity-manager-docker.md) is the only
packaging path. Recorded in
[`ARCH-liquidity-manager`](../../crates/liquidity-manager-daemon/specs/ARCH-liquidity-manager.md)'s
`Status`.

## Live defects with no repair

One more sits in the upstream section below. Regenerate this set by reading the
last `## Verdict` heading of every record under `claims/`; do not maintain the
list by hand.

- [`forged-provider-authorization-admission`](../../crates/liquidity-manager-daemon/claims/forged-provider-authorization-admission.md)
  — `expire_advertisement` copies `holder_authorizations` verbatim out of
  `provider_advertisements`, re-signs them under the live provider key, and
  publishes to every relay, with no re-verification on that path. **Nostr
  enrollment rests on this record.** The repair is to re-verify envelopes when an
  advertisement payload is reloaded, or to rebuild `holder_authorizations` from
  the verified store instead of copying them. Its enumeration of persisted
  envelope locations has been refuted twice; regenerate it from the write side.
- [`manual-safe-to-retry-duplicates-provider-send`](../../crates/liquidity-manager-daemon/claims/manual-safe-to-retry-duplicates-provider-send.md)
  — an authenticated `SafeToRetry` returns the operation to `pending`, so a
  resumed worker passes the send fence a second time. Admin authentication proves
  authority to assert the send did not happen, not that it did not. `SECURITY.md`
  carries that boundary; nothing enforces it.

## Verification work

**Independent argument checking is the highest-yield unblocked work here.** It
needs no ruling, no upstream dependency, and no deployment. Several records hold
an author-side `pass` or a `provisional` verdict for one reason only: no
independent hostile reader has checked them. When that check has run, it has
moved the verdict roughly as often as not.

Do not list the set here. Regenerate it from the records: a record needs a check
when its last `## Verdict` heading says `provisional`, says `unverified`, or says
`pass` and marks itself author-side. Each such record names, in its own verdict
section, what a checker should attack first.

**Two conventions govern those records**, and each has already misled a reader.
`claims/<name>.md` is the proof record; `claims/found-bugs/<name>.md` is the
finding summary, and 17 names appear in both, so a citation that adds or drops
`found-bugs/` retargets silently while a link check still passes. Records carry
their verdicts oldest-first, so **the last `## Verdict` heading is the current
one**.

### PR #334 needs a ruling, not a rebase

The FLIP underwriting record
([#334](https://github.com/fedibtc/manifold/pull/334)) carries 14 findings, all
verified live, rebasing cleanly. Two review points block it.

- **The top-ranked finding challenges a confirmed decision.**
  `bearer-endorsement-can-be-front-run` prescribes requester and expiry binding.
  [`SPEC-flip-rpc`](../../crates/liquidity-manager-daemon/specs/SPEC-flip-rpc.md)
  says possession *is* authorization and explicitly accepts the disclosure. The
  finding cites that decision nowhere, so it needs an authority ruling rather
  than an edit.
- **The evidence labels have no methodology.** `Tier: blinded convergence` and
  `Found by: policy-court, ops-drill` name no procedure a reader can check. That
  requires a FLIP developer response.

Three points a session can fix without a ruling: record the reviewed SHA and
define staleness against it; link the governing records and name the violated
contract for the three divergence findings; and split the namespace.

### Release envelope measurement

The envelope's allocation deadlines and recovery objective
([`liquidity-manager-release-envelope.md`](./liquidity-manager-release-envelope.md))
are stated as the release's commitment and **have not been measured against a
running deployment**. Its other values are derived from code and need no separate
work.

## Upstream dependency gaps

### A pin bump can silently reinstate a repaired defect

`repeated-target-peg-in-allocation-after-crash` passes because upstream
`allocate_deposit_address_pooled_stateless` reuses an address while
`claimed.is_empty()`. Changing `<=` to `<` in `fedimint-wallet-client`'s
pooled-address reuse is **one character, no compile error, no failing test**, and
it reinstates the defect. The per-item budget ruling rests on that repair, so
treat a pin bump touching that allocation as touching
`stability-deposit-terminal-state-not-observed` too.
