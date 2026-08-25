# Falsification: Trust-material requests amplify one slow child

## Counterexample

Let a fleet contain one running seat whose intact local child accepts `status`
and withholds its answer until the ten-second request timeout. With the production
trust-material source bound, send and finish 128 valid, sub-4-KiB
`GetFederationTrustMaterial` requests over at least two accepted Iroh connections.
Each request may name a valid but nonmatching federation/configuration pair and
one unrelated peer.

All requests pass validation and take the 128 global stream permits.
`federation_bindings` clones every seat and probes each seat before applying the
request’s federation and peer filters. For the slow seat, one command executes,
one fits in its capacity-one channel, and the others wait to enqueue while retaining
their global permits. The seat loop drains at about one child timeout per command.

During the initial timeout interval no permit is available for another FI verb.
The fixed batch creates about 1,280 seconds of aggregate serialized child work and
last-request tail latency. After the first timeout, a permit is released and an
unrelated queued request may run; this is not a claim that unrelated FI work is
blocked for the entire tail. The finite eventual drain does not remove the
self-amplification or the initial cross-verb starvation.

## Granted assumptions and affected source

The counterexample grants every assumption in
[CLAIM-fleet-manager-unsigned-fi-work-request-proportionate](../CLAIM-fleet-manager-unsigned-fi-work-request-proportionate.md):
the batch is bounded by the daemon’s own fixed handler pool, local work terminates,
and the child performs only an allowed omission delay. The slow seat need not
serve availability, quotes, or unrelated seats, so retaining all shared permits
also expands the dependency’s blast radius.

The affected mechanisms are the trust-material source in
`crates/fman/core/src/service.rs`, fleet and seat traversal in
`crates/fman/core/src/{fleet,seat,fedimint_api}.rs`, and permit lifetime in
`crates/fedi-iroh-rpc/src/iroh_protocol.rs`.

## Reproduction evidence

The source-level reproducer requires a production-bound trust source, 128 detached
handlers across at least two Iroh connections (the transport permits 100 inbound
bidirectional streams per connection), and a child that withholds `status` until
the request timeout. Response encoding and writing occur after service execution;
they are not needed to retain the permits. A federation mismatch does not avoid
the pre-filter seat scan.
