# Testing

This crate owns deterministic unit tests for Nostr constants, complete event
admission, signature and authority checks, content bounds, and native
addressable-event replacement ordering.

Cross-program event shapes require fixed, fully signed JSON fixtures in addition
to events built with production helpers. Boundary tests use literal protocol
values where practical so changing a production constant cannot silently change
both implementation and expectation.

The crate performs no relay I/O. Relay behavior, fetch retries, durable storage,
restart recovery, and component reactions to an admitted event belong to the
publisher and consumer crates that implement those mechanisms.
