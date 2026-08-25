# SPEC-admin-socket: Local operator administration

## Status

Payment-federation acceptance is the authenticated common setup-payment set; the
admin surface only observes it. Removed payment scopes stay durable and lazily
reopen on the first wallet query or payout after restart.

## Record justification

No single artifact can own the operator contract because the CLI wire shape, daemon socket lifecycle and dispatch, wallet effects, and persisted fleet settings must change coherently across separate modules and binaries.

The running daemon exposes its operator control plane as newline-delimited JSON
on `admin.sock` in the fleet data root. Each connection serves exactly one
request: one non-empty input line as a serialized `AdminRequest`, one output
line as a serialized `Result<JSON value, AdminError>`, then the daemon is done
with the connection. Malformed requests and operation failures are returned on
the error side. One request per connection matches the CLI, which opens one
connection per verb.

`AdminError` is `{ kind, message }`. The message is the operator's sentence and
is the only thing the CLI prints. It is prose, and it may be reworded. `kind` is
a closed set of discriminants (`AdminErrorKind`) and is what a program branches
on: the browser setup wizard has to pick a recovery action from a failed
restore, and matching prose to do that made rewording an error a breaking
change. A refusal with no distinct operator action reports `other`, so a new
discriminant means a new action rather than a new sentence. The set is mirrored
in `operator-ui/packages/types/src/fleet.ts` and gated by the committed
`fman_admin_error_kinds` fixture, which is generated from an exhaustive walk
over the enum.

This is a machine-local ownership boundary, not a remotely authenticated
protocol. The optional browser surface reuses the same in-process operation
dispatcher but has its own authentication boundary
([SPEC-operator-http](./SPEC-operator-http.md)). The Unix socket is created with mode `0600` under the data root, which
also contains the root identity database. Anyone able to access that data already
controls the fleet identity, so an additional application credential would not
create a meaningful stronger boundary. One data root belongs to at most one
running daemon: a second instance fails fast at startup instead of waiting or
sharing, so exactly one fleet instance owns and replaces the socket. This is the reduced
operator surface.

## Onboarding is the first thing this socket serves

A daemon whose durable onboarding stage is not `complete` has no open fleet and
serves only the operations belonging to its current stage. The stages are
`identity`, `holder_authorization`, `initial_offer`, and `complete`; each
successful transition and the facts it establishes commit in one SQLite writer
transaction. This makes setup resumable after process or browser restart.

At `identity` it answers two choosing verbs:

- `OnboardAsNew` generates the root mnemonic and starts a Fleet Manager that
  has never existed before.
- `OnboardFromBackup` adopts a supplied mnemonic and rebuilds that Fleet
  Manager's seats from its encrypted backup documents
  ([SPEC-nostr-backup-restore](./SPEC-nostr-backup-restore.md)).

At `holder_authorization`, `RefreshHolderAuthorizations` performs and awaits one
bounded Nostr fetch. At least one structurally verified, subject-bound complete
event must be durably retained before the stage advances; there is no skip. An
empty or failed fetch leaves it at this stage. Issuer trust, credential proof,
revocation, and relying-party policy remain consumer responsibilities.

At `initial_offer`, `ConfigureInitialOffer` atomically records the first price
and maximum seat count, rotates the offer epoch, and advances to `complete`.
The publisher gate applied to later price changes also applies here. Every
unrelated verb is refused with `not_onboarded`, because the browser setup wizard
branches on that discriminant rather than error prose.

When configured, the browser listener serves the same phase and implementation
([SPEC-operator-http](./SPEC-operator-http.md)). Whichever transport commits a
transition, the other observes the new durable stage.

`OnboardAsNew` carries `if_needed`, which says whether an identity choice that
was already made is an acceptable answer. An orchestrator may set it true when
restarting an unknown data root; this does not skip the later stages.
`OnboardFromBackup` has no such flag: restoring into a host with an identity is
never what was meant.

Nothing mints a mnemonic implicitly. A daemon that generated one for itself on
first start would leave an operator who meant to recover holding a phrase
nobody asked for. The identity row records that first decision, but does not by
itself mean setup is complete. The daemon opens its wallet, workers, restored
guardian children, and RPC only after the durable stage is `complete`.

Onboarding is a phase of a start, not a separate program: the same process
continues into the fleet, with no restart and nothing to re-read. It holds the
same single-instance data-root lock a running fleet does, binds the socket once,
and answers on the same one-request-per-connection protocol throughout. Each
connection samples the shared operator phase; after `ConfigureInitialOffer`
commits, status reports `runtime: starting`, and the phase switches in place to
the full dispatcher (`runtime: ready`) only after the fleet has actually
opened. Every start passes through this same sequence: a daemon that onboarded
in an earlier life binds its operator surfaces first, observes the durable
`complete` stage, and reports `runtime: starting` until its fleet reopens —
the operator can ask how startup is going on the same socket that will serve
the fleet.

## Operations

- `ShowPlans` returns the current complete plan list.
- `ShowCapacity` returns the durable maximum and currently available slots.
- `SetCapacity` replaces the durable maximum. It is refused below the number
  of active (not decommissioned) seats. A real change rotates the offer epoch,
  invalidating outstanding quotes; a no-op does not.
- `SetPrice` atomically replaces the offer with a price in millisatoshis, or
  with no price — the offer of a fleet that sells nothing, and the state a
  fresh or restored FMan starts in. It returns the plan list the new offer
  advertises, the same view `ShowPlans` serves. A nonzero price is refused in
  an environment whose profile carries no setup-payment publisher: such an
  FMan could never be paid.

  An offer is a price the whole way: the operator states one, the database
  stores one, and beyond the publisher gate nothing validates or rejects it
  on the way in, because there is nothing else it could be. Only the
  advertisement states the offer as a plan list, because the plan vocabulary
  may grow — `InfiniteBestEffort` at the stored price, or no plan at all.
- `ListPaymentFederations` is read-only: acceptance is the authenticated
  common setup-payment set
  ([SPEC-setup-payment-federations](../../../specs/SPEC-setup-payment-federations.md)),
  not an operator choice. It returns every accepted member plus wallet-only
  durable leftovers of removed members, each with an `accepted` flag, its
  current receivable flag, and a `wallet` projection. Querying a retained scope
  lazily reopens its wallet from the durable prefix rather than hiding it.

  The wallet projection keeps monetary meanings separate:
  `available_ecash_msat` is only notes currently available to a new
  transaction; `economically_sweepable_recipient_msat` is a point-in-time
  fee-aware maximum for one currently usable gateway;
  `encumbered_outgoing_msat` is payout contract or refund value known not
  currently available as ecash (null when cached state cannot establish that
  amount); and `outgoing` contains the native operation id, rail,
  recipient and contract amounts, cached terminal classification, and whether
  state machines remain active. None of these values is added to another.
  Query failure yields a null affected value and a closed `query_errors`
  entry, never zero. The wallet observations are repeated around the
  affordability query; any balance, operation-log, outcome, or active-set
  change makes the whole projection unknown rather than combining snapshots.
  A nonzero residue becomes known uneconomical only when typed gateway bounds
  prove that it cannot fund even the minimum contract. The pinned fee-quote API
  does not distinguish every other unaffordable denomination/fee shape from an
  internal query failure, so those cases remain unknown rather than being
  guessed to zero.

  `drain_state` is `drained` only when every required query succeeded, no
  operation is active or has an unresolved outcome, and the known
  fee-aware recipient amount is zero. It is otherwise `sweepable`,
  `pending_wallet_work`, or `unknown`. A cached Lightning success does not
  override active mint change/refund state machines.
- `PayoutDestination` returns the one LNURL-pay or Lightning Address used for
  all payment and guardian-fee sweeps, or null when none is configured.
  `SetPayoutDestination` replaces or clears it.
- `SweepPaymentFees` requires a caller-generated `request_id` and a configured
  payout destination. It uses the
  Fedimint client's native, persisted Lightning
  operation to send as much of that federation's balance as can economically
  fund the recipient amount, gateway fee, federation Lightning output fee,
  and mint input fees. It accepts no amount: uneconomical notes and rounding
  residue remain rather than making a best-effort sweep fail. Gateway
  selection is automatic: Lightning v2's own selection supplies its vetted
  route; when v2 is unavailable the v1 path prefers the federation metadata's
  currently available `vetted_gateways` and falls back to another available
  gateway only when none of those are usable, matching Fedi's selection
  policy. The response is a durable job carrying the request, its immutable
  payment-federation scope and destination snapshot, and the committed native
  operation id and recipient amount, not bearer ecash.

  Before native start, FMan inserts the job keyed by `request_id`. Reusing that
  id with another scope fails. Reusing it in the same scope returns the same job
  and never creates another native operation; it retains the original destination
  snapshot even when the global destination setting has since changed. Both
  payment-wallet and guardian-fee wallets expose native request-aware start,
  status, and await primitives. Start returns the
  native operation id and recipient amount only after the pinned Fedimint
  client has committed the operation log entry, transaction submission, request
  metadata, and state machines in its local database. Within each wallet scope,
  the request lookup and possible start share one process-lifetime fence. A
  serialized v1 start rejects an LNURL
  invoice with an already completed payment operation instead of returning the
  old success as a new start. Status and await accept that exact id,
  verify from durable metadata that it is an FMan payout in the selected wallet
  scope, and never request an invoice or submit another payment. Their common
  projection keeps the Lightning rail state (`pending`, `succeeded`,
  `failed_or_refunded`, or `unknown`) separate from whether any state machine
  for that operation remains active. In particular, rail success does not imply
  that mint change has finished, and rail failure/refund does not imply that
  mint input or refund work has finished. Await waits only for the rail's
  terminal outcome and then reports both facts. Await does not require the
  best-effort terminal-outcome cache write to succeed: directly observed success
  determines success, active state machines are reread independently, and cached
  v1 state remains only an input to the refund distinction hidden by v1's
  aggregate failure result.

  Process death after the native commit but before the SQLite job link leaves a
  pending job and a request-marked operation. Repeating the sweep or reading
  `PayoutStatus` reconciles that exact operation into the job before doing
  anything else. `PayoutStatus` never starts work; it reports a pending job when
  no native commit exists. `AwaitPayout` also reconciles, then requires and
  awaits the exact committed operation. Both verbs use `request_id`, so a lost
  response is recovered without another invoice or outgoing payment.
- `ListSeats` returns durable summary facts for every accepted seat, including
  decommissioned records, sorted by creation time. Expired or otherwise
  refused quotes never create seats. Each summary carries a sanitized
  `completion_callback` projection with one exact key set:
  `{state:not_configured}`;
  `{state:pending, attempts, next_attempt_at_ms, last_reason}`;
  `{state:operator_blocked, attempts, reason}`;
  `{state:delivered, attempts, at_ms}`; or
  `{state:terminal, attempts, at_ms, reason}`.
  Reasons come only from the closed sanitized reason vocabulary.
  The callback URL, hook bearer, and idempotency key never appear in `ListSeats`
  or `SeatStatus`.
- `SeatStatus` validates the textual seat id and returns its summary plus a live
  report. The report distinguishes active and decommissioned seats; active
  seats include health and their created, DKG-in-progress, running, or data-loss
  phase, with the formed invite where applicable. It also carries
  `guardian_fee`: the seat's remittance account, and — once it has a federation
  — whether that federation still pays this FMan, at what rate and share.
  The seat reads consensus metadata through its own `fedimintd`, without
  joining a wallet client. Reading it is best-effort and reports `policy_error`
  rather than failing the status, because a seat before DKG and a seat whose
  metadata is unreadable are both distinct from a seat that has been cut out.
  The daemon never acts on this; it is the input to the operator's own decision
  to keep hosting or to decommission
  ([REQ-guardian-fee-remittance](../../../specs/REQ-guardian-fee-remittance.md)).
- `DecommissionSeat` validates and finds the seat, durably marks decommissioning
  as terminal, stops its child, and frees capacity while retaining its lifetime
  port allocation. Repeating the operation succeeds and reports
  `already_decommissioned: true`.
- `ReenrollTelemetry` durably advances the one FMan-wide telemetry capability
  generation and immediately schedules verified registration of the replacement.
  It returns no bearer. The previous capability stops authorizing discovery,
  metrics, and journals before the command returns.
- `GuardianFees` validates the seat id, resolves the federation it guards from
  its running invite code, and returns that seat's remittance account, its
  stability-pool balances (staged, locked, idle, and their sum as
  `collectable_msat`), and the most recent remittances with each payer's
  breakdown opened. A remittance whose sealed breakdown does not decrypt is
  still reported, with `breakdown_error` in place of `breakdown`: the amount is
  real money regardless of whether its paperwork is readable. The remittance
  account is derived from the mnemonic and the seat id, so it is answerable for
  a seat with no federation yet; the balances and remittances are not, and the
  verb fails until the seat has one.
  It also projects the federation's current guardian-fee metadata as `policy`:
  `configured`, `send_ppm`, the raw `recipients` value, and this FMan's own
  entry in it as `our_weight` out of `total_weight`.
  `share_matches_policy` summarizes policy integrity: no recipient value is
  acceptable because guardian fees are optional, but if one exists this FMan
  must appear at exactly the compiled guardian weight. A malformed value, an
  omission, or any other weight reports `false`.
  The share is read through the seat's `fedimintd` from the meta module's
  consensus value only; config metadata is never consulted. A recipient list
  the payer refuses (an unknown version, a zero or overflowing weight, or more
  recipients than it honours) yields no share here either, rather than promising
  money that will never be sent. Because that metadata is mutable,
  `share_matches_policy: false` is how an operator sees an attempted omission or
  change to this guardian's fixed weight, distinct from an optional unset policy.
- `CollectGuardianFees` moves what was remitted into that seat's account out of
  the pool: idle balance is claimed directly and staged plus locked deposits go
  through an unlock, because neither module operation subsumes the other. It
  reports a complete outcome only after every required operation reaches terminal
  success and the resulting balance read succeeds. The complete JSON shape remains
  `claimed_msat` (now ordinary ecash) plus `awaiting_cycle_msat` (still locked,
  collectable after the next cycle turnover).
  Once any collection operation has a durable operation ID, a later failure is a
  successful structured incomplete response rather than an Admin error, even when
  no amount has reached terminal success. In that response `claimed_msat` counts
  only the exact amount confirmed by terminal success, `awaiting_cycle_msat` is
  the optional result of a post-failure balance read, and `incomplete` gives the
  failed phase, whether that phase itself produced a durable operation ID, and an
  operator-safe error. A failure remains an Admin error only when no durable
  operation exists and no earlier progress was confirmed.
- `SweepGuardianFees` requires a caller-generated `request_id` and sends as much
  already-collected ecash as is economically
  sweepable to the same global payout destination, using the same automatic
  gateway selection as `SweepPaymentFees`. Collection remains a separate explicit
  operation: a sweep never unlocks or claims stability-pool funds. Like the
  payment-wallet sweep, its durable record is the native Fedimint Lightning
  operation and the response carries the same durable job shape as
  `SweepPaymentFees`, with a guardian scope containing both federation and seat.
  `GuardianFees.wallet` exposes the same drain projection for that seat's
  separate guardian-client scope; stability-pool staged, locked, and idle
  balances remain separate fields and are not ordinary ecash.
- `Onboarding` returns `service_pubkey`, the commitment-signing public key,
  `fman_name`, the deterministic two-word presentation name derived from the
  public FMan identity
  ([ARCH-service-fleet-manager](../../service-fleet-manager/specs/ARCH-service-fleet-manager.md)),
  and projects `fman-nostr`'s `service_nostr_pubkey` and durably enrolled
  Holder-authorization status (the Nostr boundary is always constructed from
  the environment profile). The operator uses the configured identity when
  arranging a Holder authorization. It also returns `fman_version`: the
  running package version, the latest version from the authenticated
  setup-payment publication (or null before one is admitted), and whether
  SemVer ordering requires an update. Consumers decide how to present that
  information; the daemon does not stop its guardian children.
- `RefreshHolderAuthorizations` is available only during the onboarding
  Holder-authorization stage. It awaits one bounded Nostr reconciliation and
  returns the resulting onboarding projection. Verified events merge into
  durable enrollment state; failures and empty answers retain the last accepted
  state. There is no post-fleet manual refresh operation.
- `ShowMnemonic` returns the root mnemonic phrase as `mnemonic`, for the
  operator's recovery material (the full backup also requires the FMan
  database and each running seat's non-derivable fedimintd state,
  [ARCH-fleet-manager-identity](./ARCH-fleet-manager-identity.md)). It exists only
  in the response to the connected operator: the daemon never logs the phrase — generation included,
  because logs outlive the data root's permissions; this verb is its retrieval path.

The `fman-cli --data-dir <path> <verb>` commands are a thin client of this
same socket.
They reserve stdout for the successful structured JSON response and initialize
ordinary filtered tracing on stderr for human-facing warnings. The one-shot CLI
does not initialize the daemon's separate explicitly shareable event journal.
The daemon removes a stale socket path before binding. Socket accept failures
are logged; per-connection transport failures are not — the connected CLI
surfaces its own transport errors. Operation errors are returned to the
requesting local operator.
