import type { GuardianLinkResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { PhoneLinkPage } from '@/pages/phone-link/PhoneLinkPage';
import * as adminCallModule from '@/shared/api/adminCall';

const unlinked: GuardianLinkResponse = { availability: 'ready', link: null, offer: null };
const phone: NonNullable<GuardianLinkResponse['link']> = {
  device_label: 'Pixel 8',
  linked_at: 1700000000,
  last_notified_at: null,
  notified_reasons: [],
  delivery: { state: 'active', reason: null }
};
const linked: GuardianLinkResponse = { ...unlinked, link: phone };
const offer = { uri: 'manifold:guardian-link?secret=mock-only', expires_at: 1700004200 };

let client: QueryClient;

const renderPageFor = (response: GuardianLinkResponse) => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <PhoneLinkPage />
    </QueryClientProvider>
  );
};

const renderPage = (response: GuardianLinkResponse) => {
  renderPageFor(response);
  return vi.mocked(adminCallModule.adminCall);
};

beforeEach(() => vi.spyOn(Date, 'now').mockReturnValue(1700004000000));

afterEach(() => vi.restoreAllMocks());

const terminalLink: NonNullable<GuardianLinkResponse['link']> = {
  ...phone,
  delivery: { state: 'terminal', reason: 'hook_expired_or_revoked' }
};

it('should send the host away and mint nothing where the policy keeps phone link off', async () => {
  const adminCall = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ availability: 'disabled', link: null, offer: null });
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createMemoryRouter(
    [
      { path: '/', element: <p>Overview</p> },
      { path: '/phone-link', element: <PhoneLinkPage /> }
    ],
    { initialEntries: ['/phone-link'] }
  );
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
  expect(await screen.findByText('Overview')).toBeInTheDocument();
  expect(router.state.location.pathname).toBe('/');
  expect(screen.queryByText('Stay connected to your guardian')).not.toBeInTheDocument();
  expect(adminCall).not.toHaveBeenCalledWith('CreateGuardianLinkOffer');
});

it('should name the setting when no push gateway is configured, without minting a code', async () => {
  const adminCall = renderPage({ ...unlinked, availability: 'no_push_gateway' });
  expect(await screen.findByText('Phone alerts are off on this host')).toBeInTheDocument();
  expect(screen.getByText('FLEET_MANAGER_PUSH_GATEWAY_ORIGIN')).toBeInTheDocument();
  expect(adminCall).not.toHaveBeenCalledWith('CreateGuardianLinkOffer');
});

it('should mint a code as soon as the page opens with no phone linked', async () => {
  const adminCall = renderPage(unlinked);
  await waitFor(() => expect(adminCall).toHaveBeenCalledWith('CreateGuardianLinkOffer'));
});

it('should show an open offer as a QR with the scan steps and never the URI', async () => {
  renderPage({ ...unlinked, offer });
  expect(await screen.findByRole('img', { name: 'Guardian link QR code' })).toBeInTheDocument();
  // The steps follow the phone: scan, confirm, then the notification prompt.
  const steps = within(screen.getByRole('list', { name: 'Link steps' })).getAllByRole('listitem');
  expect(steps.map((step) => within(step).getByRole('heading').textContent)).toEqual([
    'Scan this code',
    'Tap Link guardian',
    'Allow notifications'
  ]);
  expect(steps[0]).toHaveTextContent('Open the Fedi app on your phone, tap Scan/Paste');
  expect(screen.queryByText(/Scanning it replaces/)).not.toBeInTheDocument();
  expect(screen.queryByText(offer.uri)).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
});

it('should show the linked device and never claim a notification was sent when none was', async () => {
  renderPage(linked);
  expect(await screen.findByText('Pixel 8')).toBeInTheDocument();
  expect(screen.getByText('None yet')).toBeInTheDocument();
  expect(screen.getByText('Notifications active')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send a test alert' })).toBeInTheDocument();
  // Links have no end date to show.
  expect(screen.queryByText('Link expires')).not.toBeInTheDocument();
  const notified = within(screen.getByRole('list', { name: 'What you’ll get notified about' }));
  expect(notified.getAllByRole('listitem').map((item) => item.textContent)).toEqual([
    'A Health check fails',
    'An update is recommended',
    'Support writes to you'
  ]);
  expect(screen.getByText(/one alert when a problem starts/)).toBeInTheDocument();
});

it('should say Today only for a notification within the current local day', async () => {
  // Date.now is pinned, so one second before and a full day before it fall on
  // either side of the label regardless of the runner's timezone.
  const nowSeconds = 1700004000;
  const first = renderPageFor({
    ...linked,
    link: { ...phone, last_notified_at: nowSeconds - 1 }
  });
  expect(await screen.findByText(/^Today, \d\d:\d\d$/)).toBeInTheDocument();
  first.unmount();
  renderPageFor({
    ...linked,
    link: { ...phone, last_notified_at: nowSeconds - 86_400 * 2 }
  });
  expect(await screen.findByText(/^1[2-3] Nov 2023, \d\d:\d\d$/)).toBeInTheDocument();
});

it('should ask on the page before unlinking and name the phone', async () => {
  const adminCall = renderPage(linked);
  fireEvent.click(await screen.findByRole('button', { name: 'Unlink' }));
  expect(adminCall).not.toHaveBeenCalledWith('RevokeGuardianLink');
  expect(
    screen.getByText('Unlink Pixel 8? It stops getting alerts from this guardian.')
  ).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send a test alert' })).not.toBeInTheDocument();

  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(adminCall).not.toHaveBeenCalledWith('RevokeGuardianLink');
  expect(screen.getByRole('button', { name: 'Send a test alert' })).toBeInTheDocument();

  fireEvent.click(screen.getByRole('button', { name: 'Unlink' }));
  fireEvent.click(screen.getByRole('button', { name: 'Unlink' }));
  await waitFor(() => expect(adminCall).toHaveBeenCalledWith('RevokeGuardianLink'));
});

it('should offer only Reconnect on a stopped phone and mint when pressed, not before', async () => {
  const adminCall = renderPage({ ...linked, link: terminalLink });
  expect(await screen.findByRole('alert')).toHaveTextContent('Your phone stopped getting alerts');
  expect(screen.getByText('Pixel 8')).toBeInTheDocument();
  expect(screen.getByText('Notifications stopped')).toBeInTheDocument();
  expect(screen.queryByText('Notifications active')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Unlink' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send a test alert' })).not.toBeInTheDocument();
  expect(adminCall).not.toHaveBeenCalledWith('CreateGuardianLinkOffer');

  fireEvent.click(screen.getByRole('button', { name: 'Reconnect phone' }));
  await waitFor(() => expect(adminCall).toHaveBeenCalledWith('CreateGuardianLinkOffer'));
  expect(adminCall).not.toHaveBeenCalledWith('RevokeGuardianLink');
});

it('should not mint a code while the linked phone can still be reached', async () => {
  const adminCall = renderPage(linked);
  expect(await screen.findByText('Pixel 8')).toBeInTheDocument();
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  expect(adminCall).not.toHaveBeenCalledWith('CreateGuardianLinkOffer');
});

it('should show a stopped phone and its new code together, without the button', async () => {
  renderPage({ ...linked, link: terminalLink, offer });
  expect(await screen.findByText('Pixel 8')).toBeInTheDocument();
  expect(screen.getByRole('img', { name: 'Guardian link QR code' })).toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Reconnect your phone' })).toBeInTheDocument();
  expect(screen.getByText(/Scanning it replaces this phone and the warning/)).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Reconnect phone' })).not.toBeInTheDocument();
});

it('should announce a link that lands while the page is open, until dismissed', async () => {
  const adminCall = renderPage({ ...unlinked, offer });
  expect(await screen.findByRole('img', { name: 'Guardian link QR code' })).toBeInTheDocument();
  expect(screen.queryByText('Phone connected')).not.toBeInTheDocument();

  adminCall.mockResolvedValue(linked as never);
  await act(() => client.invalidateQueries());
  expect(await screen.findByText('Phone connected')).toBeInTheDocument();
  expect(screen.getByText('Pixel 8 will now get guardian alerts.')).toBeInTheDocument();

  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(screen.queryByText('Phone connected')).not.toBeInTheDocument();
});

it('should not announce a link that was already there when the page opened', async () => {
  renderPage(linked);
  expect(await screen.findByText('Pixel 8')).toBeInTheDocument();
  await act(() => client.invalidateQueries());
  expect(screen.queryByText('Phone connected')).not.toBeInTheDocument();
});

it('should announce a stopped phone replaced by a new scan', async () => {
  const adminCall = renderPage({ ...linked, link: terminalLink, offer });
  expect(await screen.findByText('Notifications stopped')).toBeInTheDocument();

  adminCall.mockResolvedValue({
    ...linked,
    link: { ...phone, linked_at: phone.linked_at + 60 }
  } as never);
  await act(() => client.invalidateQueries());
  expect(await screen.findByText('Phone connected')).toBeInTheDocument();
  expect(screen.getByText('Notifications active')).toBeInTheDocument();
});

it.each([
  ['delivered', null, 'Test alert sent. Check your phone.'],
  ['retryable', 'network', 'Test alert could not be sent. Try again.'],
  ['retryable', 'rate_limited', "You've used this hour's test alerts. Try again later."],
  ['terminal', 'hook_not_found', 'Notifications stopped. Reconnect your phone.']
] as const)('should show the %s %s test alert outcome', async (outcome, reason, message) => {
  const adminCall = renderPage(linked);
  adminCall.mockImplementation(async (request) =>
    request === 'TestGuardianLinkNotification' ? ({ outcome, reason } as never) : (linked as never)
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Send a test alert' }));
  expect(await screen.findByRole('status')).toHaveTextContent(message);
  expect(adminCall).toHaveBeenCalledWith('TestGuardianLinkNotification');
});
