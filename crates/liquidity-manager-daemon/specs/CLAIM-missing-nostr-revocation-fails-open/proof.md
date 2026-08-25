# Current argument

## Argument

**L1 (`test`) — a non-Nostr-only authority makes the revocation stage
unavailable.** `no_nostr_locations_is_unavailable` builds a valid installed
authority whose only location is HTTPS, runs `run_revocation_stage` for its
valid credential, and requires `unavailable == true` plus a failed
`revocation_freshness` check. The test fails if an empty Nostr filter regains a
vacuous success.

**L2 (`test`) — both production trust boundaries reject an unsupported
authority.** `no_nostr_endorsement_authority_is_unavailable` replaces the
endorsement issuer's installed authority with an HTTPS-only authority and
requires the complete pipeline to return `provider_unavailable` with a failed
`revocation_freshness` check. `no_nostr_advertisement_authority_is_unavailable`
keeps the endorsement issuer on a responding Nostr relay, then uses a distinct
installed HTTPS-only issuer for the accepted live advertisement envelope; it
requires the later policy path to return the same code and failed check for
that distinct issuer. These tests fail if either current consumer ignores
`stage.unavailable`.

**L3 (`enum` + `code`) — every production allocation insertion runs the
pipeline before its write.** Each verification boundary reads its issuer
authorities before starting its revocation stage, and that immutable read is
the authority snapshot quantified by this claim. Regenerating
`allocation_store::insert_allocation` references across the complete scoped
production source domain gives its definition and exactly one caller:
`public::accept_or_reject_request`. Regenerating that caller's exits
gives: an existing-allocation response, pre-validation rejection, pipeline
rejection, capacity rejection, a concurrent winner's existing-allocation
response, and the successful `insert_allocation` path. The sole write path
also has error exits: request serialization; both existing-allocation lookup
or signing calls; pre-validation; missing setup configuration; stateless
rejection signing; transaction opening; planning; either rollback; insertion;
the concurrent-winner lookup; commit; and final accepted-response signing.

Every pre-verification error and the existing-allocation path returns before a
write transaction opens. The sole write path calls
`verification_provider.verify` first and returns its rejection (or a
stateless-rejection signing error) before opening that transaction. All
post-verification error and semantic paths are therefore reachable only after
an outcome without a rejection. For an in-claim request without an existing
allocation, L2 instead returns `provider_unavailable` before
`insert_allocation` for the authority snapshot that pass read.

## Residual windows

## Weakest links

1. **L3 (`enum`/`code`)** — a new production allocation writer or successful
   allocation path must preserve verification before its write.
2. **L2 (`test`)** — admission and policy must both preserve the unavailable
   rejection.
3. **L1 (`test`)** — the empty-supported-location branch must remain
   unavailable.
