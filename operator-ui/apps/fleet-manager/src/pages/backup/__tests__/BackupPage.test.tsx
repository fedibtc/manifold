import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, expect, it, vi } from 'vitest';
import { MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { BackupPage } from '../BackupPage';

const onboarding = (servicePubkey: string) => ({
  service_pubkey: servicePubkey,
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed' as const, checked_at: 1_760_000_000 }
});

const LONG_PUBKEY = '02aabbccddeeff0011223344556677889900112233445566778899001122334455';

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <BackupPage />
      </MemoryRouter>
    </QueryClientProvider>
  );
  return client;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should render the Backup heading', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(onboarding('02aabbccddeeff00'));
  renderPage();

  screen.getByRole('heading', { name: 'Backup' });
});

it('should show the truncated derived service pubkey', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(onboarding(LONG_PUBKEY));
  renderPage();

  await waitFor(() => screen.getByText(/02aabbccdd…/));
});

it('should say the phrase is the whole backup', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(onboarding('02aabbccddeeff00'));
  renderPage();

  await waitFor(() => screen.getByText(/recovery phrase is the whole backup/i));
});

it('should say recovery only happens during setup, and offer no restore action', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(onboarding('02aabbccddeeff00'));
  renderPage();

  await waitFor(() => screen.getByText(/Recovery happens only while setting up a host/i));
  expect(screen.queryByRole('button', { name: /restore/i })).toBeNull();
});

it('should link to the recovery phrase reveal page', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(onboarding('02aabbccddeeff00'));
  renderPage();

  const link = await screen.findByRole('link', { name: 'Reveal recovery phrase' });
  expect(link.getAttribute('href')).toBe('/backup/phrase');
});

it('should state that the browser did not keep the recovery phrase', async () => {
  // The wizard's step lives in memory only, so a reload during setup loses it. The
  // phrase is still reachable here, and this line says so without claiming the
  // operator ever wrote it down.
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(onboarding('02aabbccddeeff00'));
  renderPage();

  await screen.findByText(/did not save your recovery phrase/i);
  expect(screen.getByRole('link', { name: 'Reveal recovery phrase' })).toBeTruthy();
});

it('should show a loading state instead of a dash while the query is pending', () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(() => new Promise(() => {}));
  renderPage();

  screen.getByText(/loading/i);
  expect(screen.queryByText('—')).not.toBeInTheDocument();
});

it('should show an error banner, not a dash, when the query fails', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(
    new AdminApiError('pubkeys unavailable')
  );
  renderPage();

  await waitFor(() => screen.getByText('pubkeys unavailable'));
  expect(screen.queryByText('—')).not.toBeInTheDocument();
});

it('should offer a retry when the read has never answered', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockRejectedValueOnce(new AdminApiError('pubkeys unavailable'))
    .mockResolvedValue(onboarding(LONG_PUBKEY));
  renderPage();

  await waitFor(() => screen.getByText('pubkeys unavailable'));
  fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

  await waitFor(() => screen.getByText(/02aabbccdd…/));
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

// W3.1: an identity key that was correct a minute ago is more use than a blank
// page. The page used to delete these for the whole outage.
it('should keep the identity keys under a staleness marker when a refresh fails', async () => {
  vi.spyOn(adminCallModule, 'adminCall')
    .mockResolvedValueOnce(onboarding(LONG_PUBKEY))
    .mockRejectedValue(new AdminApiError('pubkeys unavailable'));
  const client = renderPage();

  await waitFor(() => screen.getByText(/02aabbccdd…/));
  await act(async () => {
    await client.refetchQueries();
  });

  await screen.findByText('Showing last-known data');
  expect(screen.getByText(/02aabbccdd…/)).toBeTruthy();
  expect(screen.queryByText('pubkeys unavailable')).toBeNull();
});
