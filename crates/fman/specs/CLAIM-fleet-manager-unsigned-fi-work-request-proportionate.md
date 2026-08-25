# CLAIM-fleet-manager-unsigned-fi-work-request-proportionate: Unsigned FI work is request-proportionate

Each unsigned FI verb—`GetAvailability`, `GetQuote`, and
`GetFederationTrustMaterial`—performs bounded, request-proportionate daemon work.
A cheap valid request does not fan out into an unbounded database scan, wallet
operation, child I/O, allocation, or signing work, and a bounded batch of such
requests cannot amplify one slow dependency into starvation of unrelated FI
verbs.

## Status

Falsified: `GetFederationTrustMaterial` probes every seat before filtering, so a
bounded handler-sized batch can serialize behind one withholding child while
retaining the shared FI permits. See the
[durable counterexample](CLAIM-fleet-manager-unsigned-fi-work-request-proportionate/falsification-trust-material-seat-fanout.md).

## Assumptions

- **A-abuse-controls:** Deployment abuse controls bound raw public-RPC connection
  and request volume, but do not excuse per-request amplification; the bound
  permits a fixed batch as large as the daemon’s shared handler pool.
- **A-local-cost:** SQLite indexed and aggregate queries, locally open Fedimint
  client-configuration reads, canonical serialization, and Schnorr operations
  terminate for bounded inputs unless an identified dependency is slow.
- **A-child-delay:** A hosted `fedimintd` may accept a local WebSocket request and
  withhold its response until the configured timeout; it does not forge a
  response.
