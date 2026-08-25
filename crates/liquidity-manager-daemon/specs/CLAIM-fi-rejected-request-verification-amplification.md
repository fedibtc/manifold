# CLAIM-fi-rejected-request-verification-amplification: Fi rejected request verification amplification

Within one runtime generation and configured verification window, a hostile FI
holding one valid FMan endorsement cannot make FLIP begin more than the
configured per-federation allowance of outbound trust-verification runs by
sending any number of signed requests which pass local admission and are
rejected later.

The FI may vary every requester-authored field, including its declared federation
id, request signature, and expiry; retry sequentially or concurrently; and hold
an endorsement for a federation whose later FMan-policy verification fails. It
cannot forge credentials, alter configured authorities/relays, or use Admin
verbs. Renewal in later windows is outside this per-window rate property.

## Status

Unverified.

## Assumptions

- **A1 — outbound lookup effects.** Invite preview, FMan advertisement lookup,
  and revocation lookup each may perform network work when invoked.
- **A2 — feasible late rejection.** A valid endorsement can pass admission while
  the target fails a later policy stage (for example, an untrusted/missing other
  FMan advertisement).
