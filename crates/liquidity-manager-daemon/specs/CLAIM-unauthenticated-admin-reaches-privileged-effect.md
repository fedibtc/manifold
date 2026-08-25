# CLAIM-unauthenticated-admin-reaches-privileged-effect: Unauthenticated admin reaches privileged effect

A request lacking the configured Admin bearer cannot reach any enumerated normal-
or restore-mode Admin handler that changes state/value/trust/secrets or inspects
or restores an archive. The adversary can send arbitrary HTTP requests to either
listener but cannot guess the bearer token.

## Status

Unverified.

## Assumptions

- **A1 — router semantics.** Axum route layering applies the configured middleware
  to every route in the protected router.
- **A2 — bearer secrecy.** The configured bearer is not available to the
  unauthenticated caller.
