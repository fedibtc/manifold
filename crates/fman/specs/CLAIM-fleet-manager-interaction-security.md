# CLAIM-fleet-manager-interaction-security: Fleet Manager confines remote interactions

Against arbitrary remote bytes and identities, replay, collusion, races, crashes,
and concurrent verbs on its public RPC, Nostr, and guardian-network surfaces,
Fleet Manager confines each interaction to the authority and effects granted by
that surface's authenticated or intentionally unauthenticated contract. It does
not disclose Fleet Manager-held secrets; grant cross-seat or fleet authority;
allocate a paid seat without its exact payment; settle a quote more than once;
consume unrelated operator value; admit attacker-controlled payment policy,
trust, or restore state; misuse guardian authority for fee collection; or delete
guardian data after exposing its federation invite.

This property covers interaction authority, confidentiality, integrity, and
value safety. It does not assert availability, resource or capacity bounds,
latency or operating-cost bounds, or resistance to traffic analysis.

## Status

Unverified. The callback-aware signed RPC expands the remote
method/effect roster, while the proof still enumerates the preceding 3+10
surface. Durable Holder-authorization enrollment also adds an Admin-triggered
Nostr-to-SQLite effect not covered by the proof roster. The complete
current 3+11 roster, including both effects, before treating this parent claim
as current.

## Assumptions

- Standard signature, hash, mint blinding, BIP-39, and labelled key-derivation
  schemes satisfy their stated security properties.
- The host is single-tenant; its filesystem permissions protect the data root;
  operator-controlled local processes and the owner-only Admin socket are
  trusted; and the pre-local-parameters localhost window of a new `fedimintd`
  is not adversarial.
- The pinned `fedimintd`, Fedimint client, iroh, SQLite, RocksDB, filesystem,
  operating-system process isolation, and Nostr dependencies satisfy their
  documented security contracts or fail detectably.
- The configured Bitcoin node has an honest chain view and uses credentials the
  operator is authorized to provide; the host clock meets protocol freshness
  bounds; committed SQLite writes survive crashes; and the data-root lock
  excludes another daemon using that root.
- The Admin caller is the honest operator, including when asserting that the
  original host is gone before restore.
- The pinned setup-payment publisher is uncompromised and publishes no malicious
  policy, and the chosen setup-payment federation retains an honest threshold.
- An FI-authenticated request obtains privileged access only to a seat whose
  durable owner is that request's verified signer; this imports only that part
  of
  [CLAIM-fleet-manager-fi-seat-access-owner-bound](CLAIM-fleet-manager-fi-seat-access-owner-bound.md).
- [CLAIM-fleet-manager-confines-seat-local-authority](CLAIM-fleet-manager-confines-seat-local-authority.md)
- [CLAIM-fleet-manager-confines-secret-dependent-content](CLAIM-fleet-manager-confines-secret-dependent-content.md)
- Fleet Manager creates a paid seat only after verifying its exact quote-bound
  payment and value; this imports only that part of
  [CLAIM-fleet-manager-paid-seat-payment-verified](CLAIM-fleet-manager-paid-seat-payment-verified.md).
- [CLAIM-fleet-manager-quote-settlement-exclusive](CLAIM-fleet-manager-quote-settlement-exclusive.md)
- [CLAIM-fleet-manager-preserves-published-guardian-data](CLAIM-fleet-manager-preserves-published-guardian-data.md)
- Only the current, complete, valid publication from the pinned publisher
  determines admitted setup-payment policy; relay withholding or replay cannot
  select an older authentic policy (see
  [CLAIM-fleet-manager-payment-policy-publisher-controlled](CLAIM-fleet-manager-payment-policy-publisher-controlled.md)).
- Only holder-authentic authorizations bound to this Fleet Manager enter its
  served trust material. After this daemon version normalizes and exposes an
  in-bound authorization, relay withholding cannot erase every enrolled row
  while the receiver maximum issue time does not move backward; replay cannot replace an
  enrolled same-credential authorization with an equal or older one. Relying
  FIs check current issuer policy and revocation. The
  authentication part is recorded by
  [CLAIM-fleet-manager-holder-authorization-bound](CLAIM-fleet-manager-holder-authorization-bound.md).
- Restore adopts only current, authentic, internally consistent documents bound
  to the recovered identity; relay withholding or replay cannot select older
  authentic recovery state (see
  [CLAIM-fleet-manager-restore-adopts-authentic-consistent-state](CLAIM-fleet-manager-restore-adopts-authentic-consistent-state.md)).
- The bundled, pinned `fedimintd` and its dependencies retain their intended
  implementation integrity under hostile network input. This is an explicit
  FMan TCB premise, not malicious-child containment: the preserved
  [counterfactual containment claim is falsified](CLAIM-fleet-manager-compromised-child-contained.md).
- Guardian-fee collection and sweeping use only authorized, attributable
  effects. Collection output counts only terminally confirmed value, preserves a
  structured incomplete outcome after any durable operation exists, and exposes
  no dependency error text in that successful operator response (see
  [CLAIM-fleet-manager-guardian-fee-revenue-accounted](CLAIM-fleet-manager-guardian-fee-revenue-accounted.md)
  and
  [CLAIM-fleet-manager-value-moves-use-client-authority](CLAIM-fleet-manager-value-moves-use-client-authority.md)).
- Production cannot select development trust roots, placeholder identities, or
  development-only trust overrides.
- Each guarded federation retains an honest threshold, and its client API
  satisfies the protocol semantics Fleet Manager uses for guardian-fee
  inspection, collection, and withdrawal.
