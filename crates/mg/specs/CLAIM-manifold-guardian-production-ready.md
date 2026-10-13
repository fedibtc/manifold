# CLAIM-manifold-guardian-production-ready: Manifold Guardian is ready for production

Within its documented single-instance production envelope, Manifold Guardian
confines every adversary-reachable interaction to its authorized effects,
including protection of secrets, seat and guardian authority, operator value,
payment policy, restore and trust state, and published-guardian deletion. It can
host paid guardian seats unattended. After loss of a host, the operator can
recover the Manifold Guardian and every published guardian from the root mnemonic,
retained authentic Nostr backup documents, and federation peers. Under the
release's dependency-availability preconditions and workload limits, supported
FA and Admin operations complete within their documented deadlines, and restart
or recovery restores service within the documented recovery objective.

## Status

Unverified: the expanded production-readiness composition has not been verified.

## Assumptions

- [CLAIM-manifold-guardian-production-deployment-envelope](CLAIM-manifold-guardian-production-deployment-envelope.md)
- [CLAIM-manifold-guardian-supported-release-envelope](CLAIM-manifold-guardian-supported-release-envelope.md)
- Every release-identified supported FA or Admin operation and
  restart/recovery transition completes within its stated deadline or recovery
  objective when its stated workload and dependency-availability preconditions
  hold.
- Every action required to keep a paid guardian serving, including remediation
  of each monitored or detectable in-envelope failure, starts and completes
  automatically without operator action.
- Every release-identified supported FA or Admin operation and restart/recovery
  transition, and every hosted paid guardian's seat-local lifecycle and Fedimint
  protocol participation, is semantically correct: each satisfies all safety and
  liveness postconditions of its supported behavior when its stated workload and
  dependency-availability preconditions hold.
- [CLAIM-manifold-guardian-relay-publication-durable](CLAIM-manifold-guardian-relay-publication-durable.md)
- [CLAIM-manifold-guardian-recovery-dependencies](CLAIM-manifold-guardian-recovery-dependencies.md)
- [CLAIM-manifold-guardian-interaction-security](CLAIM-manifold-guardian-interaction-security.md)
- [CLAIM-manifold-guardian-guardian-metrics-egress-confined](CLAIM-manifold-guardian-guardian-metrics-egress-confined.md)
- Manifold Guardian satisfies
  [SPEC-nostr-backup-restore](SPEC-nostr-backup-restore.md), including complete
  ordered publication before the discoverable seat document, and restores only
  authenticated, internally consistent documents bound to the recovered
  identity.
