# ARCH-fleet-manager-product-boundary: What v1 deliberately ships without

Status: unconfirmed

This record is the current v1 shipping boundary: the single list of
machinery v1 deliberately omits or replaces. An absence listed here is a
decision, not a finding; when an item is implemented (or ruled permanently
out of scope), its entry changes accordingly.

## Current product boundary

`InfiniteBestEffort` is a one-time payment for hosting at the operator's
discretion, with no protocol expiry. The operator can decommission the seat;
there is no refund, slashing, or dispute-resolution mechanism for a later
decommission. This asymmetric bargain assumes pre-existing mutual trust:
credential and social/reputation policy are the FI's protection, while the
signed seat commitment is evidence of what was bought, not an enforcement
mechanism. Operators must price the open-ended disk and availability obligation
into the one-time charge.

## Simpler mechanisms

- **Stateless quotes.** [SPEC-locked-payment](./SPEC-locked-payment.md) creates a
  seat only
  after payment, quotes allocate nothing, and there is no pre-seat
  state to expire or sweep. Terminal records are retained indefinitely.
- **Joined-federation check.** v1 does not probe every
  federation on a cadence with failure/recovery hysteresis and a
  re-advertise dwell. It checks the only thing offline
  verification needs — the payment federation's client config is
  present — at paid-plan `GetQuote` and ad publish. Payment acceptance
  itself never needs the federation reachable.

## Omitted

- **Rate limiting.** v1 has no rate limiting. Quotes hold nothing,
  capacity is consumed only by completed payments, and invalid
  `CreateSeat` garbage is rejected by millisecond-scale offline
  verification — but unsigned verbs remain uncounted compute.
- **Approval admission.** The admission axis is gone: every offered plan
  auto-accepts on a verified payment. `PendingApproval`, the approval TTL,
  and `max_pending_approvals` do not exist in this profile.
- **`SubscriptionBased` and the payment-expiry tail.** An operator states an
  offer as a price, so the plan cannot be offered at all: Grace → Suspended → Deleted, renewal
  payments, and the retention clock have no trigger and do not exist.
  `GetStatus` reports the no-grace wire fields as fixed values.
- **General intent journal.** v1 has no general side-effect journal;
  daemon-coupled children
  ([ARCH-fleet-manager-seat-processes](./ARCH-fleet-manager-seat-processes.md)) remove the
  re-adoption hazards it guarded against. Driven DKG leaves incomplete setup
  in child-owned staging and atomically creates the final data directory only
  for a complete config. Restart discards only staging and no path removes the
  final directory, so there is no reconstructible destructive intent
  ([SPEC-seat-lifecycle](./SPEC-seat-lifecycle.md)).
- **Ad region hint.** There is no operator-declared `region` field in the
  ad payload.
- **Operator metrics.** There is no fleet-level Prometheus endpoint (only each
  fedimintd's own localhost metrics port).

Seat-process consequences (no ordinary-log rotation, no terminal `Failed`
health, no hot reload) are scoped by the lifetime-coupling ruling and
recorded in [ARCH-fleet-manager-seat-processes](./ARCH-fleet-manager-seat-processes.md), not
here.

## Current parameter choice

- Ad `expires_at` is stamped 2 × the republish interval — the advertisement schema's recommended floor.
