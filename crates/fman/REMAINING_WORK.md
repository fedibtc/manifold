# FMan remaining work

This document keeps known gaps and future work outside the current-state Linked Specs records.

Items are ordered by dependency and risk, not promised for a release.

### 1. Protect existing seats and operator value

- **Disk-exhaustion behavior.** Track data-root free space and storage-write
  health. When durable writes are unavailable, publish zero availability and
  refuse new paid quotes before an FI pays; never acknowledge a state change
  whose write did not commit. Existing guardians should keep running as long as
  their own storage permits, with the condition visible in operator status and
  logs.
- **Abuse controls.** Bound unsigned availability/quote traffic and invalid
  paid presentations before expensive work. Stateless quotes prevent capacity
  theft, but do not make CPU, bandwidth, or wallet-configuration work free.

### 2. Complete the declared cross-component surface

The shared protocol declares four verbs beyond the ceremony surface. Three now
ship; `GetFedimintStats` remains `UnsupportedVerb`
([SPEC-fi-rpc](./specs/SPEC-fi-rpc.md)). Enable the remaining verb only with its full
trust and failure boundary:

- **Peer attestations — shipped.** `GetPeerAttestation` binds the FMan service
  identity to the final federation config and this seat's peer once consensus
  is running. It stays a diagnostic/recovery read for the FI, not FLIP's trust
  source: verifiers read the directory out of consensus metadata.
- **Public trust material — shipped.** `GetFederationTrustMaterial` returns the
  FMan's signed public material for the requested federation. It is
  intentionally unauthenticated because an invite-code verifier needs to fetch
  it; [SPEC-fi-rpc](./specs/SPEC-fi-rpc.md) and `SECURITY.md` define its validation
  and relying-verifier boundary.
- **`SetMetaField` — shipped.** The compiled, typed per-key validator set is in
  `meta_fields`, and unknown keys fail closed. It serves the guardian-fee rate plus the narrow Guardianito-compatible FI
  maintenance keys. Formation-owned directory and recipient keys are refused. Unknown keys and per-key oversized raw values are refused
  before a child probe. Every signed request commits to the exact raw consensus
  merge base; stale requests are refused, and the FI serializes all metadata
  writers and confirms threshold adoption. This is the required whole-object merge story
  ([SPEC-fi-metadata-maintenance](./specs/SPEC-fi-metadata-maintenance.md)).
  `fedi:fman_api_urls`, privacy-policy keys, and the guardian-fee recipient list
  remain deliberately absent; formation installs the fixed recipient policy.
- **`GetFedimintStats`.** Keep the FI-facing filtered diagnostic unsupported
  until its own statistic set is defined. Federation telemetry now has the
  separate, raw, capability-scoped Iroh path in
  [SPEC-guardian-telemetry-proxy](./specs/SPEC-guardian-telemetry-proxy.md).
- **Guardian-fee metadata.** Collection itself now exists: each seat's
  federation has a mnemonic-recoverable remittance account, and the operator can
  read balances, opened remittance breakdowns, and unlock what accrued
  ([REQ-guardian-fee-remittance](../../specs/REQ-guardian-fee-remittance.md),
  [SPEC-admin-socket](./specs/SPEC-admin-socket.md)). `ProposeFormationMeta`
  validates endpoint-proved seat bindings and installs the directory, fixed
  recipient list, and initial rate atomically. Later rate-only changes use the
  generic `SetMetaField` path; directory and recipient keys remain reserved
  ([SPEC-guardian-fee-policy](./specs/SPEC-guardian-fee-policy.md)).

The advertisement binds the commitment-signing key to the Nostr identity in
payload v1 (`service_pubkey`,
[SPEC-advertisement](./specs/SPEC-advertisement.md)), so commitments can be
verified from discovery alone
([ARCH-fleet-manager-identity](./specs/ARCH-fleet-manager-identity.md)).

### 3. Make operation diagnosable and recoverable

- **FMan-side ceremony timeouts are omitted.** `DkgInProcess` does not
  self-expire. The FI owns patience and timeout policy, and product retry
  remains an explicit human decision.
- **Restart history is omitted.** After an FMan restart there is no distinction
  between never-started and started-then-died ceremonies; both are
  `New` because no ceremony fact is durable.
- **Cross-guardian split formation remains an FI foot-gun.** An FI can cancel
  and begin a second ceremony after the first has partially formed at other
  peers. A seat that has formed refuses, and `RestartDkg` exposes the race, but
  FMan has no distributed prepare/commit protocol. The operator must
  decommission and replace any formed straggler before retrying.
- Add fleet-level metrics for storage health/free space, advertisement
  publication, wallet receivability and balances, child restart state, and
  request rejection classes. Keep labels bounded.
- Extend seat inspection with runtime ceremony inspection, child restart/backoff
  information, and the exact version/config needed for support, without
  exposing secrets or payment bearer material.
- Define explicit fedimintd upgrades before bundling multiple versions. A seat
  must never silently move to another version; an upgrade is operator-driven
  and coordinated with the other guardians.
- Add operator workflows only where they expose a real capability: backup and
  restore, diagnostics, version transition, and trust/advertisement status.
  Universal CLI/API parity is not itself a requirement.

### 4. Add commercial and lifecycle modes deliberately

- **Free admission.** Free seats left the plan vocabulary, because a free plan
  offered by default is an invitation to exhaust the fleet. What remains is a
  price of zero, which an operator must set deliberately and which then applies
  to everyone: that is the deployment bootstrap, not a commercial mode. A
  targeted give-away means an out-of-band admission path
  (an operator-issued quote the FI presents), which needs its own bearer/named
  binding, expiry, capacity bound, and offer-epoch rules. Approval-based
  admission on top of that would additionally need pending-capacity bounds,
  operator notification, typed FI status, and the exact point at which a free
  seat becomes committed — not the old reservation mechanism restored merely
  to hold an approval.
- **Subscriptions.** Activating `SubscriptionBased` requires signed renewal
  terms, price-consent rules, grace/suspension behavior, wallet-outage handling,
  retention, and recovery tests. The dormant wire variants are not an
  implementation.
- **Resource-aware capacity.** Extend the existing seat/port capacity gate with
  measured disk, memory, or CPU limits only after defining stable, conservative
  signals. Disk safety and observability come before an adaptive sales limit.
  Deferred here is *enforcement*; the non-enforcing recommended default of
  [REQ-seat-capacity-default](./specs/REQ-seat-capacity-default.md) is implemented
  (onboarding recommends an initial maximum from available RAM).

### 5. Extend discovery and identity only with sibling agreement

- Add a region hint only after the FI selection policy and region vocabulary
  have a common owner; availability hints are not trust claims.
- Define service-identity replacement or rotation with the component owners before promising
  continuity from an old advertisement identity to a new one.
- Credential expiry and revocation evaluation remain FI policy today. Moving
  any of that policy into the FMan requires an explicit trust-boundary change,
  not merely introducing issuer-allowlist machinery.
