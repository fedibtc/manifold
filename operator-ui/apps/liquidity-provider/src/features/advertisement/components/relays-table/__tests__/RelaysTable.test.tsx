import type { RelayPublicationState } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { RelaysTable } from '../RelaysTable';

const NOW = Date.parse('2026-07-20T02:00:00Z');

const relays: RelayPublicationState[] = [
  {
    relay_url: 'wss://relay.fedi.social',
    status: 'published',
    last_error: null,
    last_seen_at: 1784505600 // 2026-07-20T00:00:00Z
  },
  {
    relay_url: 'wss://relay.damus.io',
    status: 'disconnected',
    last_error: 'timeout after 10s',
    last_seen_at: 1784426400 // 2026-07-19T02:00:00Z
  }
];

describe('RelaysTable', () => {
  it('should render the Relays section heading', () => {
    render(<RelaysTable relays={relays} now={NOW} />);
    expect(screen.getByRole('heading', { name: 'Relays' })).toBeTruthy();
  });

  it('should render a row per relay with its url and relative last-seen', () => {
    render(<RelaysTable relays={relays} now={NOW} />);
    expect(screen.getByText('wss://relay.fedi.social')).toBeTruthy();
    expect(screen.getByText('2h ago')).toBeTruthy();
    expect(screen.getByText('1d ago')).toBeTruthy();
  });

  it('should append the last error to a failed relay status', () => {
    render(<RelaysTable relays={relays} now={NOW} />);
    expect(screen.getByText('Disconnected · timeout after 10s')).toBeTruthy();
  });
});
