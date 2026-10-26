# SPEC-fa-backup-payload: Portable FA recovery payload

## Record justification

fa-client owns consumer-persisted formation recovery from Fedi seed-derived
backup keys while authoritative state spans MGs, FLIP, and federation
consensus, so this distributed ownership requires a durable record.

## Payload

One FA supports one permanent formed federation. Its payload contains a schema
version, monotonic snapshot generation, formed federation invite (f+1 guardian
addresses; backups from older releases hold one), and every seat's
MG identity, seat id, and stable locator. It optionally contains the exact
`RequestLiquidityDetailsCommitmentV1` for the sole non-terminal liquidity
request. The operation id and payload hash are recomputed from that commitment.
Completed and terminally rejected liquidity work is omitted.
It optionally contains the open guardian transfer whose destination runs the
guardian but is not in the seats yet, which is the transfer's
`publishing_binding` phase: the source seat index and the destination's MG
identity, seat id, and locator. A transfer in any other phase is not backed
up. An absent transfer is
omitted from the encoding, so a payload without one keeps its earlier bytes
and older readers. The writer applies the same validation as restore before
it seals a payload: a transfer destination that is already in the seats, or
any other payload that restore refuses, is never published.
It lists every past host: the MG identity, and the verified badge holder
when known, of each source that a guardian transfer moved a peer away from.
Destination selection excludes them, because a past host may still hold its
key share. An empty list is omitted in the same way.

The payload omits FA identity, formation id and intent, runtime checkpoints,
signed quotes, acceptances and attestations, guardian codes and fee accounts,
payment state, trust proofs and policy, leases, response/status signatures,
terminal liquidity history, and all secrets or spendable value. Restore keeps
locators exactly; later operation-specific trust checks do not refresh them
from advertisements.

The FA's formed federation record holds this payload. Formation writes the
record when consensus confirms the formed-metadata target, and import writes it
from a backup. A snapshot is eligible only from that record. There is no
deletion, tombstone, or second federation payload.

## Sealed document and relay reconciliation

The version and generation exist only inside ciphertext. The document is one
CBOR item in a u32 little-endian length frame, zero-padded to exactly 32 KiB,
then sealed by XChaCha20-Poly1305 with a fresh 24-byte nonce and FA-specific key
and AAD domains. Event content is standard base64. Oversize fails without
compression, slicing, or another event.

One stable addressable Nostr coordinate is authored by a dedicated FA backup
key. `fa-client` builds, seals, and signs desired events, while independent
per-relay workers decide whether their relay needs publication, read back,
retry, and durably record their own last confirmed plaintext SHA, generation,
event id, and time. To guard against relay retention pruning, each worker
publishes a freshly resealed replacement when its relay's confirmation reaches
15 days old, without advancing the snapshot generation. Publication blocks no
FA operation except one: a guardian transfer votes no binding until a publish
quorum of relays confirms the current backup, which names the transfer. The
transfer asks for an immediate publication and reports `publishing_binding`
while it waits. Fedi enforces one active writer.

Restore queries every configured relay through its bounded deadline, ignores
invalid, foreign, undecryptable, or unsupported candidates, and selects the
authenticated payload with the highest snapshot generation among completed
reads. A strict majority of canonical relays must complete their queries before
FA reports a backup or no backup; fewer complete reads report an incomplete
lookup for retry. Every such read set includes a relay of each publish quorum,
so a restore never misses a confirmed guardian transfer. This is not proof that
the remaining relays hold no newer backup. FA records a successful
restored-or-empty lookup in its own FA database by Manifold environment.
An authenticated import and its completion record commit in one transaction;
an incomplete read commits neither. Reopening the same FA database skips a
completed lookup; a fresh database or a different environment checks again.
When opened with a restored-mnemonic hint, `fa-client` starts the lookup and
retries failures in its own task group while holding its mutation guard.
New-seed openings skip recovery.

A restored payload imports the formed federation record as `Unsynced`; reconciliation
gates mutations until fresh guardian consensus confirms the saved federation
identity and complete signed seat-binding directory. It checks the saved MG
identities and any liquidity commitment against that directory. If the saved
invite cannot reach consensus, recovery signs invite requests through the saved
manager locators and tries alternatives for the same federation without
requiring every manager online or every seat healthy. Verified guardian
addresses are remembered only in memory for subsequent reads; the backup and
signed liquidity commitment remain unchanged.
Reconciliation of a restored transfer either opens the same move again at its
vote, while consensus names the source, or puts the destination in
the seats, once consensus names it. The destination's guardian-fee account
is not backed up. An open seat purchase is not backed up.
Import derives a stable local formation handle
without reconstructing omitted formation transcript rows. Only after
reconciliation does FA expose fresh post-formed authority and hydrate the
optional liquidity commitment under its recomputed operation id and payload
hash. Reopen returns that authority to `Unsynced` until reconciliation runs
again.
