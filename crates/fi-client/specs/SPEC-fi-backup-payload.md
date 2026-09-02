# SPEC-fi-backup-payload: Portable FI recovery payload

## Record justification

fi-client owns consumer-persisted formation recovery from Fedi seed-derived
backup keys while authoritative state spans FMans, FLIP, and federation
consensus, so this distributed ownership requires a durable record.

## Payload

One FI supports one permanent formed federation. Its payload contains a schema
version, monotonic snapshot generation, federation invite, and every seat's
FMan identity, seat id, and stable locator. It optionally contains the exact
`RequestLiquidityDetailsCommitmentV1` for the sole non-terminal liquidity
request. The operation id and payload hash are recomputed from that commitment.
Completed and terminally rejected liquidity work is omitted.

The payload omits FI identity, formation id and intent, runtime checkpoints,
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
then sealed by XChaCha20-Poly1305 with a fresh 24-byte nonce and FI-specific key
and AAD domains. Event content is standard base64. Oversize fails without
compression, slicing, or another event.

One stable addressable Nostr coordinate is authored by a dedicated FI backup
key. `fi-client` builds, seals, and signs desired events, while independent
per-relay workers decide whether their relay needs publication, read back,
retry, and durably record their own last confirmed plaintext SHA, generation,
event id, and time. To guard against relay retention pruning, each worker
publishes a freshly resealed replacement when its relay's confirmation reaches
15 days old, without advancing the snapshot generation. Publication never
blocks an FI operation. Fedi enforces one active writer.

Restore queries every configured relay, ignores invalid, foreign,
undecryptable, or unsupported candidates, and selects the authenticated
payload with the highest snapshot generation. It imports local recovery state
as `Unsynced`; existing reconciliation gates mutations until authoritative
services confirm it. Reconciliation signs status and invite requests for every
stored seat through its exact stored locator, requires every seat to be healthy
and running and to report one federation matching the backed-up invite, then
verifies every stored FMan identity and seat id against the fresh federation
consensus seat-binding directory. Import derives a stable local formation handle
without reconstructing omitted formation transcript rows. Only after
reconciliation does FI expose fresh post-formed authority and hydrate the
optional liquidity commitment under its recomputed operation id and payload
hash. Reopen returns that authority to `Unsynced` until reconciliation runs
again.
