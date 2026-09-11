# Proof for CLAIM-liquidity-manager-managed-funds-not-lost

## Scope

This is a compositional proof of
[CLAIM-liquidity-manager-managed-funds-not-lost](../CLAIM-liquidity-manager-managed-funds-not-lost.md).
It establishes only the local implication from its immediate assumptions. It
covers FLIP-triggered custody transitions and durable recovery authority, not
legal ownership, investment performance, purchasing power, or dependency
solvency.

## Model and quantifiers

For each durable semantic intent `i`, let `A(i)` be the source, target,
principal, fee, and reservation fixed by its configured or authenticated
operational authority. A managed effect is safe when it belongs to exactly one
`i`, uses those exact fields, and aggregate effects do not exceed `A(i)`.
`NotLost` means every supported execution maintains:

1. **authority conservation:** FLIP causes no effect outside a safe effect or the
   explicit authenticated audited abandonment boundary; and
2. **custody continuity:** every automatic transition retains the technical
   credentials and item-specific evidence needed to resume, recover, or
   reconcile the managed transition.

A protocol-authorized stability settlement may change the position's economic
value without violating this operational predicate. Conversely, accounting or
key possession alone does not establish legal ownership or actual redeemability.

## Assumptions

The immediate assumptions are listed in
[the claim record](../CLAIM-liquidity-manager-managed-funds-not-lost.md). The
first three premises define semantic authority, bind every actual effect to it,
and preserve technical recovery authority. Linked claims cover completion
attribution, duplicate effects, ambiguous-send retry, budget conservation,
stale work, correct stability targets, automatic write-off, ambiguous stability
recovery, and common-point backup. The final premise protects the credentials
and stores needed to exercise those paths.

## Argument

1. **`assumption + claim` — each debit belongs to one bounded authority.** The
   semantic-intent and actual-effect premises bind every irreversible effect,
   including one whose item never completes. The duplicate-withdrawal,
   duplicate-stability-deposit, and manual-retry claims prevent direct,
   automatic, and manual retry paths from turning one authority into duplicate
   effects.
2. **`claim` — completed debits and aggregate liabilities remain attributable.**
   The completion-attribution claim binds completed value to the exact persisted
   target. The stale-capacity and wallet-budget claims keep active and possibly
   spent authority charged against known spendable value. The post-cancellation
   claim prevents released or terminal authority from enabling a later effect.
3. **`claim` — stability funding uses the authorized target configuration.** The
   accepted-module and config-revision claims require the funded address to come
   from the authenticated accepted configuration with a usable stability module.
4. **`assumption + claim` — interruptions retain technical custody.** The
   automatic-transition premise retains credentials and item-specific evidence.
   The no-automatic-stranding and ambiguous-deposit claims supply the supported
   stability recovery boundary; the backup claim keeps allocation and
   target-client evidence at one recovery point; and the operator premise
   retains the credentials and stores needed to exercise those paths.

Lemmas 1–3 establish authority conservation. Lemma 4 establishes custody
continuity. Their conjunction is exactly `NotLost` at the operational boundary.

## Residuals

- The claim does not decide legal or economic ownership among the daemon,
  deployment operator, credential delegates, and stability provider.
- It does not guarantee stability principal, purchasing power, profitability,
  dependency solvency, or redemption.
- Authenticated audited abandonment deliberately ends FLIP's management; whether
  the economic owner authorized that delegation is a separate ownership claim.

## Weakest links

1. The semantic-intent, actual-effect, and automatic-transition premises are
   broad and not yet focused linked implementation claims.
2. Recovery depends on both the common-point archive property and retention of
   every required credential and target-client secret.
3. The implementation's provider/operator/position-owner boundary is not
   independently represented, as assessed by
   [CLAIM-flip-stability-provider-independent](../CLAIM-flip-stability-provider-independent.md).
