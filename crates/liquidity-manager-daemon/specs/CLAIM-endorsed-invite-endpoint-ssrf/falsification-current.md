# Current counterexample

The endorsement authenticates only the federation id. An attacker can retain
that id in a syntactically valid invite and replace its guardian endpoint with
the canonical Iroh locator of a node it owns. Before FLIP authenticates the
returned federation configuration, the pinned Iroh transport sends discovery,
QUIC, or relay handshake and framing traffic to whatever destinations that
node publishes.

Under the production `GlobalOnly` endpoint policy, every location-bearing
`ws`/`wss` invite URL is rejected before any connector runs, so the requester
can no longer choose an arbitrary request stream or read the destination's
response. [CLAIM-liquidity-manager-production-ready](../CLAIM-liquidity-manager-production-ready.md)
accepts the remaining Iroh traffic as a residual.
