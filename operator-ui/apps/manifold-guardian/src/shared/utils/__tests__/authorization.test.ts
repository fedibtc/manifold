import type { OnboardingResponse } from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import { buildAuthorizationRequest, isAuthorized } from '@/shared/utils/authorization';

const waiting: OnboardingResponse = {
  stage: 'holder_authorization',
  runtime: 'starting',
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: '0.1.0', update_required: false }
};

const observed: OnboardingResponse = {
  ...waiting,
  nostr: {
    state: 'authorization_observed',
    authorizations: 1,
    holders: [MOCK_HOLDER_PUBKEY],
    checked_at: 1_760_000_000
  }
};

describe('buildAuthorizationRequest', () => {
  it('should encode the SDK request naming the service nostr key as the subject', () => {
    expect(buildAuthorizationRequest(waiting)).toBe(
      `{"subject_pubkey":"${MOCK_SERVICE_NOSTR_PUBKEY}"}`
    );
  });

  it('should produce JSON a holder app can parse', () => {
    expect(JSON.parse(buildAuthorizationRequest(waiting))).toEqual({
      subject_pubkey: MOCK_SERVICE_NOSTR_PUBKEY
    });
  });

  // Load-bearing, not tidiness: credential-app parses this with a `.strict()`
  // Zod schema (`parseHolderAuthorizationRequest`), so any added key — a
  // `type`, a `version`, an environment tag — makes the holder application
  // reject the whole request. BE-FMAN-AUTH-002 has to change both sides at once.
  it('should carry no field beyond the subject the holder signs over', () => {
    expect(Object.keys(JSON.parse(buildAuthorizationRequest(waiting)))).toEqual(['subject_pubkey']);
  });
});

describe('isAuthorized', () => {
  it('should be false while the relay still reports waiting', () => {
    expect(isAuthorized(waiting)).toBe(false);
  });

  it('should be true once an authorization is observed', () => {
    expect(isAuthorized(observed)).toBe(true);
  });

  it('should be false with no response at all', () => {
    expect(isAuthorized(undefined)).toBe(false);
  });
});
