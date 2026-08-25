# Current argument

## Residual windows

The claim is falsified by an in-scope execution, so the counterexample is not
filed as a residual. Outside the quantifiers are a compromised bearer token
issuing deliberately distinct intent keys and a Byzantine gatewayd producing
multiple payments from one RPC invocation; those require authorization and
gateway-integrity claims respectively. A hard process death can leave the
create-new daemon lock file requiring operator cleanup before restart. That is a
deployment/restart liveness residual; the decisive duplicate-delivery execution
requires neither a crash nor a restart. A crash after row commit but before the
operator send may leave a `pending` withdrawal indefinitely because recovery
only observes it and no operator-specific resume/resolution surface exists; this
is a named liveness residual, not the duplicate-payment counterexample.

## Weakest links

1. **L1/L2/L4/L5 (`enum`)** — completeness of DTO fields, entry/exit channels,
   submission call sites, recovery/sync paths, and manual writers must be
   regenerated for every scoped change.
2. **L3/L4/L5/L6 (`code`)** — fresh-id generation, transaction/call ordering,
   and the absence of a join across retries are local readings vulnerable to a
   newly added guard or caller.
3. **L3 (`schema`)** — the current indexes enforce server-row identities and one
   allocation wallet row per item/type, not an operator economic-intent key.
4. **L1 (`type`)** — the DTO mechanically fixes the currently representable
   request fields, but absence of a field is only meaningful together with the
   enumerated adapters and persistence writers.
5. **A3 (axiom; weakest for the two-settlement half)** — settlement
   multiplicity bottoms out in gatewayd/client behavior; a dependency-side
   idempotency contract would change the two-payment half, though not the two-call counterexample.
