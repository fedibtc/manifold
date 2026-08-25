# CLAIM-superseded-setup-authorizes-allocation: Superseded setup authorizes allocation

A new public allocation cannot commit after an Admin setup/trust/policy revision
has superseded the setup snapshot under which that request was verified. The
adversary may delay verification and race an authenticated Admin update with a
public request, but cannot write SQLite directly.

## Status

Unverified.

## Assumptions

- **A1 — SQLite serialization.** Committed write transactions serialize and
  preserve their committed rows.
- **A2 — verification may block.** Preview and trust/revocation verification can
  remain in flight while another request commits.
