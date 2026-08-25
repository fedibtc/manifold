import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import * as adminCallModule from '@/shared/api/adminCall';
import { AuthorizationPage } from '../AuthorizationPage';

const waiting = {
  fman_name: 'mutual-hamster',
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 },
  fman_version: { current: '0.1.0', latest: null, update_required: false }
};

const observed = {
  ...waiting,
  nostr: {
    state: 'authorization_observed',
    authorizations: 1,
    holders: [MOCK_HOLDER_PUBKEY],
    checked_at: 1_760_000_000
  }
};

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <AuthorizationPage />
      </MemoryRouter>
    </QueryClientProvider>
  );
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('AuthorizationPage', () => {
  it('should show the waiting state with the full key', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
    renderPage();

    await screen.findByText(MOCK_SERVICE_NOSTR_PUBKEY);
    expect(screen.getByText(/no authorization for this fleet/i)).toBeTruthy();
  });

  // The daemon reports hex; a holder application shows the npub. The operator
  // compares the two screens, so this one renders theirs.
  it('should list an observed holder as the npub a holder application shows', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText('npub1cswcupa4j23k78gvjnjcx7mz4uqet5l8c69jfg8huxw48jqzk6jqgqdz8m');
    expect(screen.getByText(/authorization observed/i)).toBeTruthy();
  });

  it('should fall back to the reported value when a holder key does not encode', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
      ...waiting,
      nostr: {
        state: 'authorization_observed',
        authorizations: 1,
        holders: ['not-a-key'],
        checked_at: 1_760_000_000
      }
    });
    renderPage();

    await screen.findByText('not-a-key');
  });

  it('should offer nothing to check once an authorization is observed', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
    renderPage();

    await screen.findByText(/authorization observed/i);
    expect(screen.queryByRole('button', { name: 'Check now' })).toBeNull();
  });

  it('should offer no way to skip or continue', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
    renderPage();

    await screen.findByText(MOCK_SERVICE_NOSTR_PUBKEY);
    expect(screen.queryByRole('button', { name: /skip/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /continue/i })).toBeNull();
  });
});
