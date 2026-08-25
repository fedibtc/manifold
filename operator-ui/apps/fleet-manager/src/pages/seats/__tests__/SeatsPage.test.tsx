import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { SeatsPage } from '../SeatsPage';

const renderPage = (
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
) => {
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <SeatsPage />
      </MemoryRouter>
    </QueryClientProvider>
  );
  return client;
};

const SEAT = {
  seat_id: 'seat-01',
  fi_id: 'fi-01',
  plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
  created_at_ms: 1_753_000_000_000,
  payment_claim: { state: 'success', at_ms: 0 },
  completion_callback: { state: 'not_configured' },
  decommissioned: false
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should render the Seats heading', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ seats: [] });
  renderPage();

  screen.getByRole('heading', { name: 'Seats' });
});

it('should show the empty state when there are no seats', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ seats: [] });
  renderPage();

  await waitFor(() => screen.getByText(/no seats yet/i));
});

it('should point the empty state at the offer route the router actually serves', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ seats: [] });
  renderPage();

  await waitFor(() => screen.getByText(/no seats yet/i));
  expect(screen.getByRole('link', { name: 'Your offer' }).getAttribute('href')).toBe('/offer');
});

it('should list every seat with a link to its detail page', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (request === 'ListSeats') return Promise.resolve({ seats: [SEAT] });
    return Promise.resolve({
      seat_id: 'seat-01',
      report: {
        state: 'active',
        health: 'healthy',
        phase: 'running',
        invite_code: 'fed11testinvite'
      }
    });
  });
  renderPage();

  await waitFor(() => {
    const link = screen.getByRole('link', { name: 'seat-01' });
    expect(link.getAttribute('href')).toBe('/seats/seat-01');
  });
  await waitFor(() => screen.getByText('Healthy'));
});

// The lie this page told: one branch served "the daemon answered and you have no
// seats", "the daemon has not answered yet" and "the read failed" alike, and the
// boot gate only promotes a 401 — so a 403, any 5xx, a protocol error and a
// transport error all reached an operator as a reassuring claim about their fleet.
it('should not claim an empty fleet when the seat read failed', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AdminApiError('seats unavailable'));
  renderPage();

  await waitFor(() => screen.getByText('seats unavailable'));
  expect(screen.queryByText(/no seats yet/i)).not.toBeInTheDocument();
});

it('should offer a retry when the seat read failed', async () => {
  const adminCall = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockRejectedValue(new AdminApiError('seats unavailable'));
  renderPage();

  await waitFor(() => screen.getByRole('button', { name: 'Try again' }));
  const attemptsBefore = adminCall.mock.calls.length;
  fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

  await waitFor(() => expect(adminCall.mock.calls.length).toBeGreaterThan(attemptsBefore));
});

it('should not claim an empty fleet before the daemon has answered', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(() => new Promise(() => {}));
  renderPage();

  screen.getByText('Loading…');
  expect(screen.queryByText(/no seats yet/i)).not.toBeInTheDocument();
});

it('should keep the seat list under a staleness marker when a refresh fails', async () => {
  let isDaemonDown = false;
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) => {
    if (isDaemonDown) return Promise.reject(new AdminApiError('seats unavailable'));
    if (request === 'ListSeats') return Promise.resolve({ seats: [SEAT] });
    return Promise.resolve({
      seat_id: 'seat-01',
      report: {
        state: 'active',
        health: 'healthy',
        phase: 'running',
        invite_code: 'fed11testinvite'
      }
    });
  });
  const client = renderPage(
    new QueryClient({ defaultOptions: { queries: { retry: 3, retryDelay: 0 } } })
  );

  await waitFor(() => screen.getByRole('link', { name: 'seat-01' }));

  isDaemonDown = true;
  await act(async () => {
    await client.refetchQueries();
  });

  await waitFor(() => screen.getByText('Showing last-known data'));
  screen.getByRole('link', { name: 'seat-01' });
  expect(screen.queryByText(/no seats yet/i)).not.toBeInTheDocument();
});
