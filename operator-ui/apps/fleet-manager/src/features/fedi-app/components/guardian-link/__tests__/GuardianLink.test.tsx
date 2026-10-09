import type { GuardianLinkResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { GuardianLink } from '@/features/fedi-app/components/guardian-link/GuardianLink';
import * as adminCallModule from '@/shared/api/adminCall';

const unlinked: GuardianLinkResponse = { available: true, link: null, offer: null };
const offer = { uri: 'fedi://guardian-link?secret=mock-only', expires_at: 1700004200 };

beforeEach(() => vi.spyOn(Date, 'now').mockReturnValue(1700004000000));

afterEach(() => vi.restoreAllMocks());

it('should replace the no-phone state with the created QR and remove it after canceling', async () => {
  let status = unlinked;
  const call = vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request) => {
    if (request === 'CreateGuardianLinkOffer') status = { ...unlinked, offer };
    if (request === 'RevokeGuardianLink') status = unlinked;
    return status as never;
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GuardianLink />
    </QueryClientProvider>
  );

  fireEvent.click(await screen.findByRole('button', { name: 'Link a phone' }));
  expect(await screen.findByRole('img', { name: 'Guardian link QR code' })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Link a phone' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(await screen.findByRole('button', { name: 'Link a phone' })).toBeInTheDocument();
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  expect(call).toHaveBeenCalledWith('RevokeGuardianLink');
});

it('should report a failed creation inline without claiming there is an offer', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request) => {
    if (request === 'CreateGuardianLinkOffer') throw new Error('gateway offline');
    return unlinked as never;
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GuardianLink />
    </QueryClientProvider>
  );

  fireEvent.click(await screen.findByRole('button', { name: 'Link a phone' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('gateway offline');
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Link a phone' })).toBeEnabled();
});

it('should not show an expired code even if the daemon still returns it', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    ...unlinked,
    offer: { ...offer, expires_at: 1 }
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GuardianLink />
    </QueryClientProvider>
  );

  expect(await screen.findByRole('button', { name: 'Link a phone' })).toBeInTheDocument();
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
});
