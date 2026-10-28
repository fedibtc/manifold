import type { GuardianLinkResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { GuardianLink } from '@/features/phone-link/components/guardian-link/GuardianLink';
import * as adminCallModule from '@/shared/api/adminCall';

const unlinked: GuardianLinkResponse = { available: true, link: null, offer: null };
const onboarding = { fman_name: 'blissful-chiffchaff' };
const offer = { uri: 'fedi://guardian-link?secret=mock-only', expires_at: 1700004200 };

const renderLink = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GuardianLink />
    </QueryClientProvider>
  );
};

beforeEach(() => vi.spyOn(Date, 'now').mockReturnValue(1700004000000));

afterEach(() => vi.restoreAllMocks());

it('should mint a code on its own when no phone is linked and show it as a QR', async () => {
  let status = unlinked;
  const call = vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request) => {
    if (request === 'CreateGuardianLinkOffer') status = { ...unlinked, offer };
    return (request === 'Onboarding' ? onboarding : status) as never;
  });
  renderLink();

  expect(await screen.findByRole('img', { name: 'Guardian link QR code' })).toBeInTheDocument();
  expect(call.mock.calls.filter(([request]) => request === 'CreateGuardianLinkOffer')).toHaveLength(
    1
  );
  expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
});

it('should report a failed mint inline once, without claiming there is a code', async () => {
  const call = vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request) => {
    if (request === 'CreateGuardianLinkOffer') throw new Error('gateway offline');
    return (request === 'Onboarding' ? onboarding : unlinked) as never;
  });
  renderLink();

  expect(await screen.findByRole('alert')).toHaveTextContent('gateway offline');
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  await waitFor(() =>
    expect(
      call.mock.calls.filter(([request]) => request === 'CreateGuardianLinkOffer')
    ).toHaveLength(1)
  );
});

it('should treat an expired code the daemon still returns as missing and mint a new one', async () => {
  let status: GuardianLinkResponse = { ...unlinked, offer: { ...offer, expires_at: 1 } };
  const call = vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request) => {
    if (request === 'CreateGuardianLinkOffer') status = { ...unlinked, offer };
    return (request === 'Onboarding' ? onboarding : status) as never;
  });
  renderLink();

  await waitFor(() => expect(call).toHaveBeenCalledWith('CreateGuardianLinkOffer'));
  expect(await screen.findByRole('img', { name: 'Guardian link QR code' })).toBeInTheDocument();
});
