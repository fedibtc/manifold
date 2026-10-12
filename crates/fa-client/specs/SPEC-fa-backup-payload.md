# SPEC-fa-backup-payload: Portable FA recovery payload

## Record justification

fa-client owns consumer-persisted formation recovery from Fedi seed-derived
backup keys while authoritative state spans MGs, FLIP, and federation
consensus, so this distributed ownership requires a durable record.

## Payload

One FA supports one permanent formed federation. Its payload contains a schema
version, monotonic snapshot generation, federation invite, and every seat's
MG identity, seat id, and stable locator. It optionally contains the exact
`RequestLiquidityDetailsCommitmentV1` for the sole non-terminal liquidity
request. The operation id and payload hash are recomputed from that commitment.
Completed and terminally rejected liquidity work is omitted.
It also lists every spare seat in the FA seat pool by MG identity, seat id,
and locator. An empty list is omitted from the encoding, so a payload without
spares keeps its earlier bytes and older readers.

The payload omits FA identity, formation id and intent, runtime checkpoints,
signed quotes, acceptances and attestations, guardian codes and fee accounts,
payment state, trust proofs and policy, leases, response/status signatures,
terminal liquidity history, and all secrets or spendable value. Restore keeps
locators exactly; later operation-specific trust checks do not refresh them
from advertisements.

A snapshot becomes eligible only after the existing formed-metadata target is
confirmed. There is no deletion, tombstone, or second federation payload.

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
15 days old, without advancing the snapshot generation. Publication never
blocks an FA operation. Fedi enforces one active writer.

Restore queries every configured relay through its bounded deadline, ignores
invalid, foreign, undecryptable, or unsupported candidates, and selects the
authenticated payload with the highest snapshot generation among completed
reads. When no authenticated payload is found, a strict majority of canonical
relays must complete their queries before FA reports no backup; fewer complete
reads report an incomplete lookup for retry. This is an availability policy,
not proof that the remaining relays hold no backup. FA records a successful
restored-or-empty lookup in its own FA database by Manifold environment.
An authenticated import and its completion record commit in one transaction;
an incomplete read commits neither. Reopening the same FA database skips a
completed lookup; a fresh database or a different environment checks again.
When opened with a restored-mnemonic hint, `fa-client` starts the lookup and
retries failures in its own task group while holding its mutation guard.
New-seed openings skip recovery.

A restored payload imports local recovery state as `Unsynced`; reconciliation
gates mutations until fresh guardian consensus confirms the saved federation
identity and complete signed seat-binding directory. It checks the saved MG
identities and any liquidity commitment against that directory. If the saved
invite cannot reach consensus, recovery signs invite requests through the saved
manager locators and tries alternatives for the same federation without
requiring every manager online or every seat healthy. Verified guardian
addresses are remembered only in memory for subsequent reads; the backup and
signed liquidity commitment remain unchanged.
Restore imports spare seats without their guardian-fee accounts. An open
seat purchase is not backed up.
Import derives a stable local formation handle
without reconstructing omitted formation transcript rows. Only after
reconciliation does FA expose fresh post-formed authority and hydrate the
optional liquidity commitment under its recomputed operation id and payload
hash. Reopen returns that authority to `Unsynced` until reconciliation runs
again.
