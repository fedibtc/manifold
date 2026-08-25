/**
 * The canonical nostr public keys of the mock world.
 *
 * `crates/fman/core/src/admin.rs` serialises `nostr_sdk::PublicKey` with
 * `to_string()`, which yields 64 lowercase hexadecimal characters. It is never an
 * `npub`. Fixtures used to carry `npub` values, so every screen was reviewed
 * against a wire value the daemon cannot produce.
 *
 * One module owns these so the mock scenarios and the unit-test fixtures cannot
 * drift apart again. Imported by `@/mocks/*` and by `__tests__/*` only — never by
 * production components, hooks, pages or utils.
 */
export const MOCK_SERVICE_NOSTR_PUBKEY =
  'a7f3c19e4b6d02581cae37b90f4d6152ce8b41a09d7e3f26b5c08d419e2a6f3b';

export const MOCK_HOLDER_PUBKEY =
  'c41d8e07b592a36f1d0c94e5837b62af0195d3e7c68b24a0f7e19d53c802b6a4';
