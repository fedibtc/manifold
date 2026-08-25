# CLAIM-fleet-manager-quote-settlement-exclusive: A quote settles to one outcome

For each quote, Fleet Manager creates at most one durable acceptance or refund
outcome. Every emitted refund matches that quote's refund outcome, and Fleet
Manager starts claiming a payment only after the corresponding acceptance is
durable.

## Status

Unverified.

## Assumptions

- Quote identifiers are collision-resistant.
- The data-root lock excludes concurrent Fleet Manager daemons, and committed
  SQLite transactions survive process crashes.
- The configured payment federation and Fedimint client preserve the transaction
  and locked-note identities supplied by Fleet Manager.
