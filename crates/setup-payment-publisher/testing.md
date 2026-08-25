# Testing

Unit tests cover shared-schema drift, multiple federations, unknown fields,
publisher mismatch, explicit stop-set acknowledgement, receipt write ordering
and bounds, replacement ordering, opaque future-schema/future-time high-water,
preflight failure, fan-out after partial failure, canonical readback, and
keyless republishing. Relay seams use local fakes and never resolve or contact
the Production environment.

The ignored integration test publishes only to an exclusive local `defe` relay.
From a development shell with `just defe-serve` already running, lease a relay
for the test command:

```bash
DEV_DEFE_RUN_REAL_NOSTR_RELAY_TESTS=1 \
  target/debug/defe-cli --request-relay=exclusive -- \
  cargo test -p setup-payment-publisher publishes_and_verifies_on_a_leased_defe_relay -- --ignored
```

No test key, relay override, or fallback is accepted by the production command.
The ignored integration test also rejects any relay that is not insecure `ws`
on an IP loopback host before publishing.
