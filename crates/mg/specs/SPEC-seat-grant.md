# SPEC-seat-grant: Operator-issued seat grants

## Status

Not implemented. `GetQuote` accepts no grant, `QuoteTerms` carries no grant
nonce, and the operator surfaces cannot issue grants. Formation from grants in
`fa-client` and the Fedi app does not exist yet.

## Record justification

The grant contract spans operator issuance on the MG's admin and browser
surfaces, the MG's `GetQuote` and `CreateSeat` handling and seat backup, the
shared wire types in `service-manifold-guardian`, and the code that
`fa-client` and the Fedi app parse and redeem, so no single implementation
artifact can own it coherently.

A SeatGrant lets an MG operator give one seat on their MG to whichever FA
redeems the grant, for a code-based federation
([REQ-bring-your-own-guardians](../../../specs/REQ-bring-your-own-guardians.md)).
The Fedi app calls it a "guardian code". It is unrelated to Fedimint's DKG
setup code (`GuardianCode`) and to the transfer grant of
[SPEC-guardian-transfer](./SPEC-guardian-transfer.md).

## The grant

A grant is a payload that the MG signs with its commitment-signing key, using
the manager response signature with its own `seat_grant` label
([SPEC-signed-envelopes](../../service-manifold-guardian/specs/SPEC-signed-envelopes.md)).
The payload holds a random 32-byte `grant_nonce`, `issued_at`, and an optional
operator label of at most 32 characters. A grant carries no FA identity,
federation size or expiry: all of those are delegated to whoever redeems it.

The shared form is one URI:

`manifold:seat-grant?v=1&env=<manifold environment>&node=<iroh endpoint id>&key=<commitment-signing key>&grant=<payload>&sig=<signature>`

`node` and `key` together are the MG's `Locator`. The binding between them is
trusted because the operator's own dashboard produced the code, the same way a
locator is trusted out of band. `grant` is the exact signed payload bytes in
unpadded base64url, and `sig` is the signature in hex. The redeemer verifies
against those exact bytes. An FA refuses a grant whose `env` is not its own
environment.

## Issuing and revoking

The operator creates grants on the MG dashboard. Issuing a grant makes no
decision about who redeems it. The MG keeps a list of issued grants (nonce,
label, issue time, and whether each is unused, used or revoked) so the
operator can see them and revoke one individually. That list grants nothing:
authority comes from the signature, and refusal from the used and revoked
nonces.

A restore must not make a used or revoked grant usable again
([SPEC-nostr-backup-restore](./SPEC-nostr-backup-restore.md)). Each seat
created from a grant carries its grant nonce and label in its seat document,
which is backed up from the moment the seat exists, so restore rebuilds the used
nonces with their seats. Revoked nonces are carried in a fleet-level backup
document, republished on each revocation. An unused, unrevoked grant needs no
backup, because its signature stays valid.

## Redeeming

`GetQuoteRequest` takes an optional grant. With a grant, the MG:

- verifies its own signature over the grant and refuses a nonce that is
  already used or revoked;
- requires the free `InfiniteBestEffort` plan and no payment fields, and quotes
  it free whatever price the MG offers publicly;
- applies its usual seat capacity, the release's fedimintd version, and the
  offer epoch;
- applies the readiness gate's connectivity, Bitcoin, release-version and
  pressure checks, but not its historical uptime thresholds, which measure
  fitness for the Fedi-verified pool
  ([SPEC-advertisement](./SPEC-advertisement.md#readiness-gate));
- skips the advertised federation-size list and setup-payment membership.
  The size must still be 4 through 20.

It returns an ordinary signed free quote bound to the FA, size, fedimintd
version and current offer epoch. The quote's terms carry the `grant_nonce`.
`GetQuote` still writes nothing, so the FA uses it as the preview of a code as
often as it likes.

`CreateSeat` is unchanged for the FA. For a quote that carries a grant nonce,
the MG refuses a revoked nonce and records the nonce as used in the same
transaction that admits the seat. Replaying the same quote returns the same
seat. Any other quote for a used nonce is refused with its own error code. The
seat records that it came from a grant, and its label, in its backup.

An FA previews every code before it creates any seat, and creates seats only
once the set of codes is final, because each seat is bound to its federation
size. It rejects two codes for the same MG key. On `OfferChanged`, it quotes
again with the same grant and retries.
