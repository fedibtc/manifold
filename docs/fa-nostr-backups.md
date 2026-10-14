# FA Nostr backups

The shipping FA backup contract is
[`SPEC-fa-backup-payload`](../crates/fa-client/specs/SPEC-fa-backup-payload.md).

`fa-client` derives protocol signing, backup author, and backup encryption keys
as child IDs 0, 1, and 2 of the consumer-scoped FA root. It owns one
addressable kind-37706 coordinate,
builds one fixed 32 KiB encrypted document for the current desired snapshot,
and independently reconciles that exact signed event to every relay in the
canonical Manifold profile. Per-relay read-back confirmations are durable;
relay failures never block FA operations.

Restore enumerates the configured relays under the derived author and stable
`d` tag, rejects foreign signatures, ignores malformed or undecryptable event
content, and imports the highest authenticated `snapshot_generation` once
federation consensus confirms it. Fedi supplies one active writer; the protocol has no
delete event, tombstone, writer fork, or conflict-resolution machinery.

The encrypted payload contains only the permanent formed federation invite,
each seat's MG identity, seat id and exact stable locator, and optionally the
exact commitment for the sole live liquidity request. It is not a database
backup and contains no payment state, signed operational responses, policy,
secrets, or spendable value.
