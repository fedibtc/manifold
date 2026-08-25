# CLAIM-fleet-manager-production-ready: Fleet Manager is ready for production

Within its documented single-instance production envelope, Fleet Manager
confines every adversary-reachable interaction to its authorized effects,
including protection of secrets, seat and guardian authority, operator value,
payment policy, restore and trust state, and published-guardian deletion. It can
host paid guardian seats unattended. After loss of a host, the operator can
recover the Fleet Manager and every published guardian from the root mnemonic,
retained authentic Nostr backup documents, and federation peers. Under the
release's dependency-availability preconditions and workload limits, supported
FI and Admin operations complete within their documented deadlines, and restart
or recovery restores service within the documented recovery objective.

## Status

Unverified: the expanded production-readiness composition has not been verified.

## Assumptions

- [CLAIM-fleet-manager-production-deployment-envelope](CLAIM-fleet-manager-production-deployment-envelope.md)
- [CLAIM-fleet-manager-supported-release-envelope](CLAIM-fleet-manager-supported-release-envelope.md)
- Every release-identified supported FI or Admin operation and
  restart/recovery transition completes within its stated deadline or recovery
  objective when its stated workload and dependency-availability preconditions
  hold.
- Every action required to keep a paid guardian serving, including remediation
  of each monitored or detectable in-envelope failure, starts and completes
  automatically without operator action.
- Every release-identified supported FI or Admin operation and restart/recovery
  transition, and every hosted paid guardian's seat-local lifecycle and Fedimint
  protocol participation, is semantically correct: each satisfies all safety and
  liveness postconditions of its supported behavior when its stated workload and
  dependency-availability preconditions hold.
- [CLAIM-fleet-manager-relay-publication-durable](CLAIM-fleet-manager-relay-publication-durable.md)
- [CLAIM-fleet-manager-recovery-dependencies](CLAIM-fleet-manager-recovery-dependencies.md)
- [CLAIM-fleet-manager-interaction-security](CLAIM-fleet-manager-interaction-security.md)
- [CLAIM-fleet-manager-guardian-metrics-egress-confined](CLAIM-fleet-manager-guardian-metrics-egress-confined.md)
- Fleet Manager satisfies
  [SPEC-nostr-backup-restore](SPEC-nostr-backup-restore.md), including complete
  ordered publication before the discoverable seat document, and restores only
  authenticated, internally consistent documents bound to the recovered
  identity.
