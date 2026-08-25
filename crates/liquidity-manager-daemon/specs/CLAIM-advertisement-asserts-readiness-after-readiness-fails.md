# CLAIM-advertisement-asserts-readiness-after-readiness-fails: Advertisement asserts readiness after readiness fails

FLIP cannot persist or publish an advertisement asserting public
readiness after the readiness conditions used for that advertisement have failed.
The adversary may race an authenticated setup/attestation update, recovery-state
change, or dependency failure with a publisher refresh; already published,
unexpired events are excluded because relay withdrawal is explicitly a no-op.

## Status

Unverified.

## Assumptions

- **A1 — publication makes a readiness assertion.** A newly signed advertisement
  is the public ready signal described by `SPEC-flip-advertisement`.
- **A2 — concurrent Admin operations can interleave at awaits.** A setup update
  may commit while the publisher is performing later database/network work.
