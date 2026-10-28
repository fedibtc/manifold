import type { GuardianLinkResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { PhoneLinkPage } from '@/pages/phone-link/PhoneLinkPage';
import * as adminCallModule from '@/shared/api/adminCall';

const unlinked: GuardianLinkResponse = { available: true, link: null, offer: null };
const phone: NonNullable<GuardianLinkResponse['link']> = {
  device_label: 'Pixel 8',
  linked_at: 1700000000,
  callback_expires_at: 1702592000,
  last_notified_at: null,
  notified_reasons: [],
  delivery: { state: 'active', reason: null }
};
const linked: GuardianLinkResponse = { ...unlinked, link: phone };
const offer = { uri: 'fedi://guardian-link?secret=mock-only', expires_at: 1700004200 };

const renderPage = (response: GuardianLinkResponse) => {
  const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PhoneLinkPage />
    </QueryClientProvider>
  );
  return adminCall;
};

beforeEach(() => vi.spyOn(Date, 'now').mockReturnValue(1700004000000));

afterEach(() => vi.restoreAllMocks());

it('should explain when no push gateway is configured without minting a code', async () => {
  const adminCall = renderPage({ ...unlinked, available: false });
  expect(await screen.findByText(/no push gateway configured/)).toBeInTheDocument();
  expect(adminCall).not.toHaveBeenCalledWith('CreateGuardianLinkOffer');
});

it('should mint a code as soon as the page opens with no phone linked', async () => {
  const adminCall = renderPage(unlinked);
  await waitFor(() => expect(adminCall).toHaveBeenCalledWith('CreateGuardianLinkOffer'));
});

it('should show an open offer as a QR with the scan steps and never the URI', async () => {
  renderPage({ ...unlinked, offer });
  expect(await screen.findByRole('img', { name: 'Guardian link QR code' })).toBeInTheDocument();
  expect(screen.getByText(/tap Scan/)).toBeInTheDocument();
  expect(screen.queryByText(offer.uri)).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
});

it('should show the linked device and never claim a notification was sent when none was', async () => {
  renderPage(linked);
  expect(await screen.findByText('Pixel 8')).toBeInTheDocument();
  expect(screen.getByText('Never')).toBeInTheDocument();
  expect(screen.getByText('Link renews by')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send test notification' })).toBeInTheDocument();
});

it('should require confirmation before unlinking', async () => {
  const adminCall = renderPage(linked);
  const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
  fireEvent.click(await screen.findByRole('button', { name: 'Unlink' }));
  expect(adminCall).not.toHaveBeenCalledWith('RevokeGuardianLink');
  confirm.mockReturnValue(true);
  fireEvent.click(screen.getByRole('button', { name: 'Unlink' }));
  await waitFor(() => expect(adminCall).toHaveBeenCalledWith('RevokeGuardianLink'));
});

it('should show the stopped link and mint a new code without forgetting the device', async () => {
  const adminCall = renderPage({
    ...linked,
    link: { ...phone, delivery: { state: 'terminal', reason: 'hook_expired_or_revoked' } }
  });
  expect(await screen.findByText(/Notifications stopped/)).toBeInTheDocument();
  expect(screen.getByText('Pixel 8')).toBeInTheDocument();
  await waitFor(() => expect(adminCall).toHaveBeenCalledWith('CreateGuardianLinkOffer'));
  expect(adminCall).not.toHaveBeenCalledWith('RevokeGuardianLink');
});

it('should not mint a code while the linked phone can still be reached', async () => {
  const adminCall = renderPage(linked);
  expect(await screen.findByText('Pixel 8')).toBeInTheDocument();
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  expect(adminCall).not.toHaveBeenCalledWith('CreateGuardianLinkOffer');
});

it('should show a stopped phone and its new code together', async () => {
  renderPage({
    ...linked,
    link: { ...phone, delivery: { state: 'terminal', reason: 'hook_expired_or_revoked' } },
    offer
  });
  expect(await screen.findByText('Pixel 8')).toBeInTheDocument();
  expect(screen.getByRole('img', { name: 'Guardian link QR code' })).toBeInTheDocument();
});

it.each([
  ['delivered', 'Test notification delivered.'],
  ['retryable', 'Test notification could not be delivered. Try again.'],
  ['terminal', 'Notifications stopped. Link your phone again.']
] as const)('should show the %s test notification outcome inline', async (outcome, message) => {
  const adminCall = renderPage(linked);
  adminCall.mockImplementation(async (request) =>
    request === 'TestGuardianLinkNotification'
      ? ({ outcome, reason: null } as never)
      : (linked as never)
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Send test notification' }));
  expect(await screen.findByRole('status')).toHaveTextContent(message);
  expect(adminCall).toHaveBeenCalledWith('TestGuardianLinkNotification');
});
