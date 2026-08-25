# CLAIM-expired-request-is-newly-accepted: Expired request is newly accepted

FLIP cannot newly commit an allocation for a request whose `expires_at` has
passed. The adversary may choose a near-expiry valid signed request and delay
every verification dependency; retries after an allocation already exists are
excluded because they do not newly accept work.

## Status

Unverified.

## Assumptions

- **A1 — clock meaning.** `now_timestamp()` supplies the acceptance-time clock
  used by the protocol.
- **A2 — verification can outlast a request.** External verification may complete
  after a previously valid request expires.
