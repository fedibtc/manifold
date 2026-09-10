# CLAIM-liquidity-manager-production-ready: Liquidity Manager is ready for production

Within its documented single-process, single-gateway production envelope, FLIP
can accept and complete supported gateway and stability-pool allocations
unattended while preserving admission authorization and preventing operational
loss of technically authorized funds under its management across the supported
funds lifecycle, with item-specific settlement attribution, crash and retry
idempotency,
recoverability, failure visibility, secret confinement, and the canonical
service-adapter wire and signing contract. Durable allocation state grows by at
most one `allocations` row per admitted federation, and each admission spends a
valid unrevoked FMan endorsement for that federation. Incomplete RPC streams
cannot starve new streams from frame decoding.

This envelope does not bound the total number of qualifying federations or
retained target-client databases. `--max-open-target-clients` is a soft ceiling,
and pending opens can consume its separate fixed budget; four unanswering opens
can prevent every other target client from opening. Verification allowances
renew, so a valid endorsement can buy unbounded cumulative verification work.
These resource limits remain open defects except for retained on-disk databases
and the pending-open budget, which FLIP developers accept as operator-planning
and monitored upstream limitations.

A valid endorsement holder can also select an Iroh node it owns. Before
end-to-end authentication, pinned Iroh may send fixed discovery, QUIC, relay
handshake, and protocol-generated framing traffic to that node's published
addresses. The requester cannot choose arbitrary HTTP or protocol contents or
read the destination response. FLIP developers accept this constrained
blind-reachability residual; deployments with sensitive internal reachability
must apply egress controls.

Under the release's dependency-availability preconditions and workload limits,
accepted supported allocations reach completion or an actionable terminal state
within their documented deadline, and restart or restore recovers service within
the documented recovery objective.

## Assumptions

- Exactly one FLIP process and runtime generation owns one data directory and
  configured gateway. The Admin API binds to a local or private interface or is
  protected by operator-controlled access enforcement.
- The operator protects the data directory, local encryption key, Admin token,
  gateway credential, provider identity, and unencrypted backups, provisions
  durable host capacity and fee reserves, and follows the documented backup,
  restore, monitoring, and manual-recovery procedures.
- The configured gateway, chain observer, target Fedimint federations, Nostr
  relays, issuers, SQLite, filesystem, clock, and cryptographic primitives
  return authentic results with the documented durability or fail detectably.
- A supported FI supplies the required signed endorsement and trust material.
  FLIP's public RPC boundary verifies that material and binds the requester's
  signing key to the authenticated transport actor before admitting the request.
- Production disables `--trust-fixtures` and uses only release-pinned supported
  protocol and dependency versions. The release states workload limits,
  dependency-availability preconditions, allocation deadlines, and a recovery
  objective
  ([release envelope](../../../docs/liquidity-manager/liquidity-manager-release-envelope.md);
  its allocation deadlines and recovery figures are stated as the release's
  commitment and have not been measured against a running deployment).
- Every public allocation is admitted through a valid, unrevoked endorsement for
  its exact federation, and each federation has at most one allocation with at
  most one item per source
  ([claim](CLAIM-federation-capability-bounded-allocation.md)).
- FLIP fails closed when it cannot perform a required fresh credential-revocation
  check
  ([claim](CLAIM-missing-nostr-revocation-fails-open.md)).
- FI-supplied invite data cannot select an arbitrary method, path, body, header
  set, or subsequent frame contents before target-federation authentication, or
  expose the selected destination's response to the FI. Pinned Iroh's
  protocol-generated discovery/QUIC and relay handshake/framing traffic to
  node-owner-published destinations is included in the production envelope
  ([claim](CLAIM-endorsed-invite-endpoint-ssrf.md)).
- A caller without an allocation's requester key and details commitment cannot
  learn whether the allocation exists
  ([claim](CLAIM-allocation-existence-probe.md)).
- [CLAIM-liquidity-manager-managed-funds-not-lost](CLAIM-liquidity-manager-managed-funds-not-lost.md).
- [CLAIM-allocation-completion-has-item-specific-settlement](CLAIM-allocation-completion-has-item-specific-settlement.md).
- Fair worker execution eventually observes an upstream terminal
  `deposit_to_provide` result for an active stability-pool item
  ([claim](CLAIM-stability-deposit-terminal-state-not-observed.md)).
- Fair worker execution eventually observes an upstream terminal safe peg-in
  result for an active stability-pool item
  ([claim](CLAIM-stability-peg-in-terminal-state-not-observed.md)).
- One unresolved stability-pool target cannot prevent processing of later active
  allocations
  ([claim](CLAIM-stability-worker-single-target-starvation.md)).
- FI input cannot make FLIP retain an unbounded number of target clients or
  target-client databases
  ([claim](CLAIM-stability-target-client-retention-is-unbounded.md)).
- FI input cannot grow durable public-request state without consuming a
  configured finite per-requester or global admission allowance
  ([claim](CLAIM-fi-request-resource-exhaustion.md)).
- Within each runtime generation and configured verification window, one
  endorsed federation cannot exceed its configured outbound trust-verification
  run allowance, regardless of requester-authored request fields
  ([claim](CLAIM-fi-rejected-request-verification-amplification.md)).
- Incomplete unauthenticated RPC streams cannot indefinitely prevent all new
  streams from reaching frame decoding
  ([claim](CLAIM-public-rpc-slow-stream-exhaustion.md)).
- The supported service-adapter release accepts and emits only the documented
  canonical wire forms, and verifies the documented signing domain and bindings
  before treating a request or response as authenticated.
- Every supported allocation path records a non-secret, actionable failure or
  terminal outcome through the documented operator-visible monitoring and
  recovery interface.
- FLIP confines the protected data-directory contents, local encryption key,
  Admin token, gateway credential, provider identity, wallet material, and
  allocation secrets to the authorized local storage, operator, and protocol
  recipients defined by the supported deployment.
- Within the stated workload limits and dependency-availability preconditions,
  every conforming valid supported gateway and stability-pool allocation can be
  admitted and has an operator-free execution that completes successfully.
- Within the stated workload limits and dependency-availability preconditions,
  fair scheduling and each required supported gateway, wallet, target, and
  recovery operation produce an observed success, failure, or recoverable
  outcome early enough to record allocation completion or an actionable
  terminal state by the documented allocation deadline.
- Each retained durable public-request row holds one non-reusable unit of a
  configured finite global allowance for its full retention lifetime and,
  where configured, its finite per-requester allowance.
- Within one runtime generation and configured verification window, every
  delivery and wire variant for one federation consumes the same authenticated
  finite allowance.
- Configured finite limits bound concurrently retained target clients and
  target-client databases.
- Within the stated workload limits and fair-scheduling conditions, every new
  RPC stream reaches admission and frame decoding within its documented finite
  bound despite incomplete unauthenticated streams.
- Following a supported normal restart from protected durable state or the
  documented restore procedure with the protected durable backup, capacity,
  credentials, and dependencies available restores the supported service within
  the documented recovery objective.
- Every supported allocation-state, wallet-operation, target-client, and
  recovery transition has one durable semantic identity and remains idempotent
  across concurrent delivery, retry, crash, restart, and restore.
- Every admitted valid, unrevoked endorsement authorizes the exact requester
  signing key bound to the authenticated transport actor, its federation, and
  its semantic liquidity intent.
