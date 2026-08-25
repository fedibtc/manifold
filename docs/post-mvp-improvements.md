# Post-MVP improvements

Items deliberately deferred out of the MVP (target: end of August 2026). Each entry records the deferral decision plus enough design shape to pick it up later.

## Referral attribution

Attribute each Manifold-created federation to the referring Fedi Order member, for per-member revenue and transaction-volume tracking.

- **Status:** deferred out of the MVP (2026-07-17) to protect the end-of-August target; even the happy-path-only version (app already installed) adds roughly half a week.
- **Design:** opaque ref code carried in a `fedi://…?ref=NNNN` deep link into the existing FI-run federation-creation flow, reported via a nostr-signed (NIP-98) `POST /referral` on the `push-gateway`, with a client-side (MMKV) retry queue so attribution survives a failed report.
- **Rough effort:** happy path ~4-6 engineer-days; +3-4 days for deferred (post-install) attribution.
- **Open questions:** multi-referrer / overwrite rules; a review gate before any payout (self-declared codes are farmable); whether deferred-install falls back to a manual code.
