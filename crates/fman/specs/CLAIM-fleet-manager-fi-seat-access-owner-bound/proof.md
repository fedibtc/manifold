# Proof: FI seat access is owner-bound

## Stale proof

The composition does not enumerate the current service's thirteen signed verbs
(most recently the environment-gated FI `DecommissionSeat`), so the local
completeness step below is not current.
Regenerate the signed-route and access-origin enumeration before relying on this
argument or removing the claim's `Unverified` status. A linked claim's evidence
or status does not establish or stale this local implication.

## Scope and model

This proof supports
[CLAIM-fleet-manager-fi-seat-access-owner-bound](../CLAIM-fleet-manager-fi-seat-access-owner-bound.md).
It composes the official daemon's FI-signed access-origin partition with these
exact immediate assumptions:

- [CLAIM-fleet-manager-selects-only-owned-seat-authority](../CLAIM-fleet-manager-selects-only-owned-seat-authority.md):
  every successfully verified post-creation request can reach seat-specific
  behavior only on its typed, K-owned seat, and `CreateSeat` can return,
  construct, register, or start only a seat durably inserted for K; the claim
  also states its exact carve-outs and continuation boundary.
- [CLAIM-fleet-manager-confines-seat-local-authority](../CLAIM-fleet-manager-confines-seat-local-authority.md):
  no operation causally triggered through a `Seat` for S obtains the defined
  local control-plane access to a distinct seat T in the same `Fleet`.

The model permits arbitrary valid signed inputs, replay, crash and restart, and
concurrent FI and trusted local activity. The two linked properties are granted
as axioms regardless of their status or evidence. This proof does not inspect,
verify, or depend on their proofs or descendant assumptions.

## Argument

1. **[enum, stale] Access origins.** The last-reviewed service partition put
   every signed invocation into `CreateSeat` or a post-creation seat-scoped verb.
   Local privileged access could arise either while selecting or constructing
   an initial seat authority, or through an operation on the selected seat. The
   current twelve-verb service has not been regenerated against this partition,
   so this completeness step is presently stale.
2. **[claim] Initial authority is owner-bound.** Granting
   [CLAIM-fleet-manager-selects-only-owned-seat-authority](../CLAIM-fleet-manager-selects-only-owned-seat-authority.md),
   a request attributed to K can select post-creation authority only for its
   named, durably K-owned seat. The same assumption excludes wrong-owner
   construction, registration, start, stored-commitment return, and the exact
   local continuation capabilities it names. Its ownership-comparison and
   aggregate-capacity carve-outs match this claim's carve-outs.
3. **[claim] A selected seat cannot retarget a sibling.** Granting
   [CLAIM-fleet-manager-confines-seat-local-authority](../CLAIM-fleet-manager-confines-seat-local-authority.md),
   operations through the correctly selected seat cannot read, mutate, invoke,
   or use the defined local facts, rows, credential, process, supervisor, key,
   or data-path authority of another seat in the same `Fleet`.
4. **[logic] Composition.** If step 1 exhausts the current access origins, step
   2 excludes wrong-owner access at initial selection and construction, while
   step 3 excludes it through a correctly selected seat. The conjunction yields
   the claimed owner-bound access and `UnknownSeat` behavior without importing
   any linked proof mechanism or status.

## Residuals

A captured, still-fresh victim envelope remains attributed to the victim K.
This limits actor attribution but does not violate the ownership predicate.
Registry absence and owner mismatch need not have constant response time.
Trusted operator, admin, startup, and supervision paths may independently
access all seats; they remain outside the FI-authenticated invocation boundary
but must not retarget that invocation. Ordinary recipient-side Fedimint traffic
is outside local seat access.

## Weakest links

The stale access-origin enumeration is the immediate gap. Once regenerated, the
remaining local weak point is the definition mapping: every privileged local
access medium must fall either within the initial-authority property or the
operation-through-a-seat property. The linked properties are assumptions, not
proof steps to reopen here.

## Regression attack

To attack this composition independently while granting both linked properties:

1. Enumerate every current FI-signed production route and every point where it
   can first obtain or retain local seat authority. Check whether selection or
   construction and operation through a selected `Seat` exhaust the origins.
2. Compare this claim's access definition and carve-outs clause by clause with
   the two exact imported properties. Look for a local capability or semantic
   output that lies between or outside their conclusions.
3. Exercise fresh and replayed `CreateSeat`, every post-creation route, spawned
   causal work, crash/restart boundaries, and concurrent trusted local actions.
4. Report a composition counterexample only if both linked properties remain
   granted while an invocation attributed to K obtains privileged local access
   to a seat not durably owned by K, or an absent or wrong-owner typed seat
   reaches seat-specific behavior before `UnknownSeat`. Attribute a trace that
   violates a linked property to that linked claim instead of this composition.
