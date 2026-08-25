import type { GetAdvertisementStateResponse } from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { deriveAdvertisement, formatRelative, sourcesLabel } from '../format';

const NOW = Date.parse('2026-07-20T02:00:00Z');

describe('formatRelative', () => {
  it('should render a dash for a null timestamp', () => {
    expect(formatRelative(null, NOW)).toBe('—');
  });

  it('should render a numeric Unix-seconds timestamp as a relative string, not a dash', () => {
    expect(formatRelative(1721476800, NOW)).not.toBe('—');
  });

  it('should render past timestamps as "ago"', () => {
    expect(formatRelative(1784505600, NOW)).toBe('2h ago'); // 2026-07-20T00:00:00Z
  });

  it('should render future timestamps as "in"', () => {
    expect(formatRelative(1784584800, NOW)).toBe('in 20h'); // 2026-07-20T22:00:00Z
  });

  it('should render day-scale gaps in days', () => {
    expect(formatRelative(1784340000, NOW)).toBe('2d ago'); // 2026-07-18T02:00:00Z
  });
});

describe('sourcesLabel', () => {
  it('should join known source labels', () => {
    expect(sourcesLabel(['gateway', 'stability_pool'])).toBe(
      'Gateway (Lightning) · Stability pool'
    );
  });

  it('should render a dash when no sources are offered', () => {
    expect(sourcesLabel([])).toBe('—');
  });
});

describe('deriveAdvertisement', () => {
  const withAd = {
    advertisement: {
      payload: {
        provider_pubkey: 'npub1qy352euf40x77abcdefghijkzsx',
        api_endpoints: ['https://flip.example/very/long/endpoint/path'],
        supported_sources: ['gateway']
      }
    },
    publication_status: 'published',
    last_published_at: 1784505600, // 2026-07-20T00:00:00Z
    expires_at: 1784592000, // 2026-07-21T00:00:00Z,
    withdrawn_at: null,
    relay_states: [],
    ready: true,
    readiness: null,
    unverified_holder_authorization_count: 0
  } as unknown as GetAdvertisementStateResponse;

  it('should label the sources and truncate the opaque identifiers', () => {
    const view = deriveAdvertisement(withAd, NOW);

    expect(view.sources).toBe('Gateway (Lightning)');
    expect(view.provider).toContain('…');
    expect(view.endpoint).toContain('…');
    expect(view.isWithdrawn).toBe(false);
  });

  it('should dash out details and flag withdrawal when there is no advertisement', () => {
    const view = deriveAdvertisement(
      {
        advertisement: null,
        publication_status: 'withdrawn',
        last_published_at: null,
        expires_at: null,
        withdrawn_at: 1784505600, // 2026-07-20T00:00:00Z
        relay_states: [],
        ready: false,
        readiness: null,
        unverified_holder_authorization_count: 0
      },
      NOW
    );

    expect(view.provider).toBe('—');
    expect(view.endpoint).toBe('—');
    expect(view.sources).toBe('—');
    expect(view.isWithdrawn).toBe(true);
    expect(view.withdrawnAt).toBe('2026-07-20 00:00');
  });

  // The status reports the publisher's last action and the publisher moves on.
  // `withdrawn_at` is the operator's standing decision and the field the daemon
  // itself reads to stay off the relays, so the screen reads the same one.
  it('should not report a withdrawal from the publication status alone', () => {
    const view = deriveAdvertisement(
      {
        advertisement: null,
        publication_status: 'withdrawn',
        last_published_at: null,
        expires_at: null,
        withdrawn_at: null,
        relay_states: [],
        ready: false,
        readiness: null,
        unverified_holder_authorization_count: 0
      },
      NOW
    );

    expect(view.isWithdrawn).toBe(false);
    expect(view.withdrawnAt).toBeNull();
  });
});
