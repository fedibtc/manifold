import type { GuardianLinkResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useMobileNotificationsNews } from '@/features/phone-link/hooks/use-mobile-notifications-news/useMobileNotificationsNews';
import { PhoneLinkPage } from '@/pages/phone-link/PhoneLinkPage';
import * as adminCallModule from '@/shared/api/adminCall';
import { markWhatsNewSeen } from '@/shared/whats-new/whatsNew';

const unlinked: GuardianLinkResponse = { availability: 'ready', link: null, offer: null };

const News = () => <p>{useMobileNotificationsNews()?.title ?? 'nothing'}</p>;

const renderWith = (response: GuardianLinkResponse, element: ReactNode) => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(response);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <RouterProvider router={createMemoryRouter([{ path: '*', element }])} />
    </QueryClientProvider>
  );
};

const expectNothing = async () => {
  await waitFor(() => expect(adminCallModule.adminCall).toHaveBeenCalled());
  expect(screen.getByText('nothing')).toBeInTheDocument();
};

beforeEach(() => window.localStorage.clear());

afterEach(() => vi.restoreAllMocks());

it('should announce Mobile notifications while a phone can be linked', async () => {
  renderWith(unlinked, <News />);
  expect(await screen.findByText('Get notified on your phone')).toBeInTheDocument();
});

it('should stop once Not now was chosen', async () => {
  markWhatsNewSeen('mobile-notifications');
  renderWith(unlinked, <News />);
  await expectNothing();
});

it('should stop once Mobile notifications has been opened', async () => {
  const page = renderWith(unlinked, <PhoneLinkPage />);
  await screen.findByText('Stay connected to your guardian');
  page.unmount();

  renderWith(unlinked, <News />);
  await expectNothing();
});

it.each<[string, GuardianLinkResponse]>([
  ['the network has no push gateway', { ...unlinked, availability: 'no_push_gateway' }],
  ['the policy keeps it off', { ...unlinked, availability: 'disabled' }],
  [
    'a phone is already linked',
    {
      ...unlinked,
      link: {
        device_label: 'Pixel 8',
        linked_at: 1700000000,
        last_notified_at: null,
        notified_reasons: [],
        delivery: { state: 'active', reason: null }
      }
    }
  ]
])('should not announce it when %s', async (_case, response) => {
  renderWith(response, <News />);
  await expectNothing();
});
