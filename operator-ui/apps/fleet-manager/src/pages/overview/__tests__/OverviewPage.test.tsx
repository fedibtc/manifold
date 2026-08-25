import type { GuardianFeesResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, vi } from 'vitest';
import { walletStatus } from '@/mocks/wallet-status';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { OverviewPage } from '../OverviewPage';

// Every case that is not about the authorization signpost stubs an authorized
// fleet, so the only attention items on screen are the ones under test.
const AUTHORIZED_NOSTR = {
  state: 'authorization_observed' as const,
  authorizations: 1,
  holders: [MOCK_HOLDER_PUBKEY]
};

const WAITING_NOSTR = { state: 'not_observed' as const, checked_at: 1_760_000_000 };

const guardianFeesResponse = (
  overrides: Partial<GuardianFeesResponse> = {}
): GuardianFeesResponse => ({
  seat_id: 'seat1',
  federation_id: 'fed1',
  remittance_account: '{}',
  collectable_msat: 0,
  staged_msat: 0,
  locked_msat: 0,
  idle_msat: 0,
  wallet: walletStatus(0),
  lifetime_remitted_msat: 0,
  policy: {
    configured: true,
    send_ppm: 1_000,
    recipients: null,
    share_matches_policy: true,
    our_weight: 1,
    total_weight: 4
  },
  remittances: [],
  ...overrides
});

const renderPage = (
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
) => {
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <OverviewPage />
      </MemoryRouter>
    </QueryClientProvider>
  );
  return client;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should render the Overview heading', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({});
  renderPage();

  screen.getByRole('heading', { name: 'Overview' });
});

it('should show an all-clear banner when everything is receivable', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (request === 'ListSeats') return Promise.resolve({ seats: [] });
    if (request === 'ListPaymentFederations') {
      return Promise.resolve({
        federations: [
          {
            federation_id: 'fed1',
            accepted: true,
            receivable: true,
            wallet: walletStatus(0)
          }
        ]
      });
    }
    if (request === 'ShowPlans') return Promise.resolve({ plans: [] });
    return Promise.resolve({
      fman_name: 'blissful-chiffchaff',
      service_pubkey: 'abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: AUTHORIZED_NOSTR
    });
  });
  renderPage();

  await waitFor(() => screen.getByText('Advertised and healthy'));
});

it('should list a non-receivable federation as an attention item', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (request === 'ListSeats') return Promise.resolve({ seats: [] });
    if (request === 'ListPaymentFederations') {
      return Promise.resolve({
        federations: [
          {
            federation_id: 'fed1',
            accepted: true,
            receivable: false,
            wallet: walletStatus(0)
          }
        ]
      });
    }
    if (request === 'ShowPlans') return Promise.resolve({ plans: [] });
    return Promise.resolve({
      fman_name: 'blissful-chiffchaff',
      service_pubkey: 'abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: AUTHORIZED_NOSTR
    });
  });
  renderPage();

  await waitFor(() => screen.getByText('Payment federation not receiving'));
  screen.getByRole('link', { name: 'Review' });
});

it('should signpost an unobserved authorization to the Authorization screen', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (request === 'ListSeats') return Promise.resolve({ seats: [] });
    if (request === 'ListPaymentFederations') {
      return Promise.resolve({
        federations: [
          {
            federation_id: 'fed1',
            accepted: true,
            receivable: true,
            wallet: walletStatus(0)
          }
        ]
      });
    }
    if (request === 'ShowPlans') return Promise.resolve({ plans: [] });
    return Promise.resolve({
      fman_name: 'blissful-chiffchaff',
      service_pubkey: 'abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: WAITING_NOSTR
    });
  });
  renderPage();

  await waitFor(() => screen.getByText('No holder has authorized this fleet'));
  expect(screen.getByRole('link', { name: 'Review' }).getAttribute('href')).toBe('/authorization');
});

it('should show a loading state instead of zero totals while queries are in flight', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(() => new Promise(() => {}));
  renderPage();

  screen.getByText(/loading/i);
  expect(screen.queryByText('Advertised and healthy')).not.toBeInTheDocument();
  expect(screen.queryByText('0 sats')).not.toBeInTheDocument();
});

it('should show an error banner, never the healthy banner, when a query fails', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (request === 'ListSeats') return Promise.reject(new AdminApiError('seats unavailable'));
    if (request === 'ListPaymentFederations') {
      return Promise.resolve({ federations: [] });
    }
    if (request === 'ShowPlans') return Promise.resolve({ plans: [] });
    return Promise.resolve({
      service_pubkey: 'abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: AUTHORIZED_NOSTR
    });
  });
  renderPage();

  await waitFor(() => screen.getByText('seats unavailable'));
  expect(screen.queryByText('Advertised and healthy')).not.toBeInTheDocument();
  expect(screen.queryByText('0 sats')).not.toBeInTheDocument();
  screen.getByRole('button', { name: 'Try again' });
});

// W2.2. The page used to render a banner INSTEAD of its content on any error,
// and query-core clears `error` only when `data === undefined` — so once the
// page had flipped it stayed blank through every later retry, for the whole
// outage rather than one render. Four failed attempts get there, which is one
// refresh under react-query's default `retry: 3`, which a daemon restart
// supplies comfortably.
it('should keep the earnings figures under a staleness marker for a whole outage', async () => {
  let isDaemonDown = false;
  let failedBalanceReads = 0;
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (isDaemonDown) {
      if (request === 'ListPaymentFederations') failedBalanceReads += 1;
      return Promise.reject(new AdminApiError('daemon restarting'));
    }
    if (request === 'ListSeats') return Promise.resolve({ seats: [] });
    if (request === 'ListPaymentFederations') {
      return Promise.resolve({
        federations: [
          {
            federation_id: 'fed1',
            accepted: true,
            receivable: true,
            wallet: walletStatus(12_000)
          }
        ]
      });
    }
    if (request === 'ShowPlans') return Promise.resolve({ plans: [] });
    return Promise.resolve({
      fman_name: 'blissful-chiffchaff',
      service_pubkey: 'abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: AUTHORIZED_NOSTR
    });
  });
  const client = renderPage(
    new QueryClient({ defaultOptions: { queries: { retry: 3, retryDelay: 0 } } })
  );

  await waitFor(() => screen.getByText('12 sats'));

  isDaemonDown = true;
  await act(async () => {
    await client.refetchQueries();
  });

  expect(failedBalanceReads).toBe(4);
  await waitFor(() => screen.getByText('Showing last-known data'));
  screen.getByText('12 sats');
  screen.getByText('Advertised and healthy');
  expect(screen.queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();

  await act(async () => {
    await client.refetchQueries();
  });
  await act(async () => {
    await client.refetchQueries();
  });

  expect(failedBalanceReads).toBe(12);
  await waitFor(() => screen.getByText('Showing last-known data'));
  screen.getByText('12 sats');
  screen.getByText('Advertised and healthy');
});

// ListSeats and ListPaymentFederations resolve, so the page renders past its
// loading branch — but the per-seat fee queries have not answered. Summing those
// as zero would state a total the fleet never earned.
const mockAdminCallWithFees = (guardianFees: () => Promise<GuardianFeesResponse>) => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (request === 'ListSeats') {
      return Promise.resolve({
        seats: [
          {
            seat_id: 'seat1',
            fi_id: 'fi1',
            plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
            created_at_ms: 0,
            payment_claim: { state: 'success', at_ms: 1_700_000_000_000 },
            decommissioned: false
          }
        ]
      });
    }
    if (request === 'ListPaymentFederations') {
      return Promise.resolve({
        federations: [
          {
            federation_id: 'fed1',
            accepted: true,
            receivable: true,
            wallet: walletStatus(12_000)
          }
        ]
      });
    }
    if (request === 'ShowPlans') return Promise.resolve({ plans: [] });
    if (typeof request === 'object' && request !== null && 'GuardianFees' in request) {
      return guardianFees();
    }
    return Promise.resolve({
      service_pubkey: 'abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: AUTHORIZED_NOSTR
    });
  });
};

it('should show an em dash, not zero, while the per-seat fee lookups are pending', async () => {
  mockAdminCallWithFees(() => new Promise(() => {}));
  renderPage();

  await waitFor(() => screen.getByText('12 sats'));

  expect(screen.getAllByText('—').length).toBeGreaterThan(0);
  expect(screen.queryByText('0 sats')).not.toBeInTheDocument();
});

it('should show an em dash, not zero, when every fee lookup failed', async () => {
  mockAdminCallWithFees(() => Promise.reject(new AdminApiError('no fee account')));
  renderPage();

  await waitFor(() => screen.getByText(/Fee revenue could not be read for 1 seat/));

  expect(screen.getAllByText('—').length).toBeGreaterThan(0);
  expect(screen.queryByText('0 sats')).not.toBeInTheDocument();
});

it('should show an em dash, not a partial sum, when one federation balance is unreadable', async () => {
  // The daemon reports unreadable available ecash as null in the wallet projection.
  // Adding that in as a zero would state a fleet balance under "Wallet balance" that the
  // fleet cannot vouch for — and the Wallet screen would show a different figure
  // for the same wallets.
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (request === 'ListSeats') return Promise.resolve({ seats: [] });
    if (request === 'ListPaymentFederations') {
      return Promise.resolve({
        federations: [
          {
            federation_id: 'fed1',
            accepted: true,
            receivable: true,
            wallet: walletStatus(12_000)
          },
          {
            federation_id: 'fed2',
            accepted: true,
            receivable: true,
            wallet: walletStatus(null)
          }
        ]
      });
    }
    if (request === 'ShowPlans') return Promise.resolve({ plans: [] });
    return Promise.resolve({
      service_pubkey: 'abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: AUTHORIZED_NOSTR
    });
  });
  renderPage();

  await waitFor(() => screen.getByText('Advertised and healthy'));

  expect(screen.getByText('—')).toBeInTheDocument();
  expect(screen.queryByText('12 sats')).not.toBeInTheDocument();
});

it('should still total the balance when every federation reported one', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (request === 'ListSeats') return Promise.resolve({ seats: [] });
    if (request === 'ListPaymentFederations') {
      return Promise.resolve({
        federations: [
          {
            federation_id: 'fed1',
            accepted: true,
            receivable: true,
            wallet: walletStatus(12_000)
          },
          {
            federation_id: 'fed2',
            accepted: true,
            receivable: true,
            wallet: walletStatus(8_000)
          }
        ]
      });
    }
    if (request === 'ShowPlans') return Promise.resolve({ plans: [] });
    return Promise.resolve({
      service_pubkey: 'abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: AUTHORIZED_NOSTR
    });
  });
  renderPage();

  await waitFor(() => screen.getByText('20 sats'));
});

// W1.3. "Earned, all time" used to be the sum of the `remittances` list, which
// the daemon caps at 20 entries per seat, so the tile understated a busy seat
// forever. This seat has remitted 41,500,000 msat over its life and shows
// 16,000,000 msat of it in its window; with a 50,000,000 msat seat sale the tile
// reads 91,500 sats, where the old windowed sum read 66,000 sats.
it('should read the all-time tile from the lifetime figure, not the remittance window', async () => {
  // A day after the seat sale, so no timeline bucket happens to hold both and
  // the old windowed total is a number nothing else on the page produces.
  const remittedAt = Math.floor(1_700_000_000_000 / 1000) + 86_400;
  mockAdminCallWithFees(() =>
    Promise.resolve(
      guardianFeesResponse({
        lifetime_remitted_msat: 41_500_000,
        remittances: [
          { amount_msat: 6_000_000, txid: 'tx-newest', remitted_at_unix: remittedAt },
          { amount_msat: 4_000_000, txid: 'tx-middle', remitted_at_unix: remittedAt - 60 },
          { amount_msat: 6_000_000, txid: 'tx-oldest-shown', remitted_at_unix: remittedAt - 120 }
        ]
      })
    )
  );
  renderPage();

  await waitFor(() => screen.getByText('91,500 sats'));

  screen.getByText('Earned, all time');
  screen.getByText('41,500 sats');
  // The window's own sum, 16,000,000 msat, still shows on the timeline as recent
  // activity — but the total it used to produce appears nowhere.
  expect(screen.queryByText('66,000 sats')).not.toBeInTheDocument();
});

it('should render the signed-off earnings presentation unchanged once populated', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (request === 'ListSeats') {
      return Promise.resolve({
        seats: [
          {
            seat_id: 'seat1',
            fi_id: 'fi1',
            plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
            created_at_ms: 0,
            payment_claim: { state: 'success', at_ms: 1_700_000_000_000 },
            decommissioned: false
          }
        ]
      });
    }
    if (request === 'ListPaymentFederations') {
      return Promise.resolve({
        federations: [
          {
            federation_id: 'fed1',
            accepted: true,
            receivable: true,
            wallet: walletStatus(12_000)
          }
        ]
      });
    }
    if (request === 'ShowPlans') return Promise.resolve({ plans: [] });
    if (typeof request === 'object' && request !== null && 'GuardianFees' in request) {
      return Promise.resolve(guardianFeesResponse());
    }
    return Promise.resolve({
      service_pubkey: 'abc',
      service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
      nostr: AUTHORIZED_NOSTR
    });
  });
  renderPage();

  await waitFor(() => screen.getByText('Advertised and healthy'));
  screen.getByText('12 sats');
  expect(screen.getAllByText('50,000 sats').length).toBeGreaterThan(0);
  screen.getByText('gross');
  screen.getByText('accepted payment claims');
});
