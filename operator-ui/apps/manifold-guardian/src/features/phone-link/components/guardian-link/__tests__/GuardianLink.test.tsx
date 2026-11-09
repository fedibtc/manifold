import type { GuardianLinkResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { GuardianLink } from '@/features/phone-link/components/guardian-link/GuardianLink';
import * as adminCallModule from '@/shared/api/adminCall';

const unlinked: GuardianLinkResponse = { available: true, link: null, offer: null };
const offer = { uri: 'manifold:guardian-link?secret=mock-only', expires_at: 1700004200 };

const renderLink = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GuardianLink />
    </QueryClientProvider>
  );
  return client;
};

const linkedPhone = (state: 'active' | 'inactive' | 'terminal'): GuardianLinkResponse => ({
  available: true,
  link: {
    device_label: 'Pixel 8',
    linked_at: 1700003900,
    last_notified_at: null,
    notified_reasons: [],
    delivery: { state, reason: state === 'terminal' ? 'hook_target_gone' : null }
  },
  offer: null
});

beforeEach(() => vi.spyOn(Date, 'now').mockReturnValue(1700004000000));

afterEach(() => vi.restoreAllMocks());

it('should mint a code on its own when no phone is linked and show it as a QR', async () => {
  let status = unlinked;
  const call = vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request) => {
    if (request === 'CreateGuardianLinkOffer') status = { ...unlinked, offer };
    return status as never;
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
    return unlinked as never;
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
    return status as never;
  });
  renderLink();

  await waitFor(() => expect(call).toHaveBeenCalledWith('CreateGuardianLinkOffer'));
  expect(await screen.findByRole('img', { name: 'Guardian link QR code' })).toBeInTheDocument();
});

it('should drop the connected banner once the new phone stops getting alerts', async () => {
  let status: GuardianLinkResponse = { ...unlinked, offer };
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async () => status as never);
  const client = renderLink();
  await screen.findByRole('img', { name: 'Guardian link QR code' });

  status = linkedPhone('active');
  await client.refetchQueries();
  expect(await screen.findByText('Phone connected')).toBeInTheDocument();

  status = linkedPhone('terminal');
  await client.refetchQueries();
  expect(await screen.findByText('Notifications stopped')).toBeInTheDocument();
  expect(screen.queryByText('Phone connected')).not.toBeInTheDocument();
});

it('should ask to open the Fedi app while alerts are not reaching the phone', async () => {
  const call = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockImplementation(async (request) =>
      request === 'TestGuardianLinkNotification'
        ? ({ outcome: 'retryable', reason: 'target_inactive' } as never)
        : (linkedPhone('inactive') as never)
    );
  renderLink();

  expect(await screen.findByText('Not reaching your phone')).toBeInTheDocument();
  expect(screen.getByRole('alert')).toHaveTextContent('Open the Fedi app on your phone');
  expect(screen.queryByText('Notifications active')).not.toBeInTheDocument();
  // Still linked: no new code, and the test alert and Unlink stay.
  expect(screen.queryByRole('button', { name: 'Reconnect phone' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Unlink' })).toBeInTheDocument();
  screen.getByRole('button', { name: 'Send a test alert' }).click();
  expect(await screen.findByRole('status')).toHaveTextContent(
    'Test alert did not reach your phone. Open the Fedi app on it.'
  );
  expect(call).not.toHaveBeenCalledWith('CreateGuardianLinkOffer');
});
