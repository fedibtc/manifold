# CLAIM-fleet-manager-relay-publication-durable: Fleet Manager relay publication is durable

Configured Nostr relays eventually accept and retain every required
advertisement, setup-payment, backup-document, and guardian-archive event.
The operator does not treat best-effort publication as durable until this has
been observed.

## Assumptions

- Configured Nostr relays eventually accept and retain every required
  advertisement event.
- Configured Nostr relays eventually accept and retain every required
  setup-payment event.
- Configured Nostr relays eventually accept and retain every required
  backup-document event.
- Configured Nostr relays eventually accept and retain every required
  guardian-archive event.
- A durable-publication observation for a required event establishes that every
  configured Nostr relay accepted and retains that exact event, rather than
  merely accepting a best-effort send.
- The operator does not treat a best-effort publication as durable until a
  durable-publication observation occurs for that exact required event.
