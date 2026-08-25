// Advertisement fixtures for the FLIP mock server. Deterministic timestamps —
// the mock never reads a wall clock so scenarios stay reproducible.

import type {
  LiquidityProviderAdvertisement,
  RelayPublicationState,
  Signed
} from '@operator-ui/types';
// The published/ready scenario (signed payload + first relay state) is the
// real Rust-generated contract fixture (see A4 remediation task), so it
// can't drift from what the daemon's serde impls actually produce.
// Regenerate via `just gen-contract-fixtures`; never hand-edit the JSON.
import advertisementFixture from '@operator-ui/types/fixtures/advertisement.json';

export const advertisementIssuedAt = 1784505600; // 2026-07-20T00:00:00Z
export const advertisementExpiresAt = 1784509200; // 2026-07-20T01:00:00Z
export const advertisementStaleExpiresAt = 1784419200; // 2026-07-19T00:00:00Z

const relayUrl = 'wss://relay.signet.example';
const secondaryRelayUrl = 'wss://nos.lol';

export const readyAdvertisement: Signed<LiquidityProviderAdvertisement> =
  advertisementFixture.advertisement as Signed<LiquidityProviderAdvertisement>;

export const publishedRelayStates: RelayPublicationState[] = [
  ...(advertisementFixture.relay_states as RelayPublicationState[]),
  {
    relay_url: secondaryRelayUrl,
    status: 'published',
    last_error: null,
    last_seen_at: advertisementIssuedAt
  }
];

export const staleRelayStates: RelayPublicationState[] = [
  {
    relay_url: relayUrl,
    status: 'disconnected',
    last_error: 'relay connection dropped',
    last_seen_at: advertisementStaleExpiresAt
  }
];

export const withdrawnRelayStates: RelayPublicationState[] = [
  {
    relay_url: relayUrl,
    status: 'disconnected',
    last_error: null,
    last_seen_at: advertisementIssuedAt
  }
];
