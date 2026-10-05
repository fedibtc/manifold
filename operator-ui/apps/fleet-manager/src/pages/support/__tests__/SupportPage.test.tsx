import type { AdminRequest, SupportChatResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { SupportPage } from '../SupportPage';

const chat = (overrides: Partial<SupportChatResponse>): SupportChatResponse => ({
  available: true,
  messages: [
    { id: 'a'.repeat(64), author: 'fedi', body: 'Is the host online?', created_at: 1_700_000_100 },
    { id: 'b'.repeat(64), author: 'fedi', body: 'Any news?', created_at: 1_700_000_200 },
    { id: 'c'.repeat(64), author: 'operator', body: 'Yes.', created_at: 1_700_000_300 }
  ],
  unread: 2,
  ...overrides
});

const renderPage = (response: SupportChatResponse) => {
  const adminCall = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockImplementation(async (request) =>
      request === 'SupportChat' ? (response as never) : ({ unread: 0 } as never)
    );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SupportPage />
    </QueryClientProvider>
  );
  return adminCall;
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should mark read up to the newest Fedi message it shows', async () => {
  const adminCall = renderPage(chat({}));

  expect(await screen.findByText('Any news?')).toBeInTheDocument();
  await waitFor(() =>
    expect(adminCall).toHaveBeenCalledWith({
      MarkSupportRead: { up_to: 1_700_000_200 }
    } satisfies AdminRequest)
  );
});

it('should not mark anything read when nothing is unread', async () => {
  const adminCall = renderPage(chat({ unread: 0 }));

  expect(await screen.findByText('Any news?')).toBeInTheDocument();
  expect(adminCall).toHaveBeenCalledTimes(1);
});

it('should pin the safety line and say when there is no Fedi support to reach', async () => {
  renderPage(chat({ available: false, messages: [], unread: 0 }));

  expect(
    await screen.findByText('Fedi support chat is not available for this deployment yet.')
  ).toBeInTheDocument();
  expect(screen.getByText(/never ask for your recovery phrase/)).toBeInTheDocument();
  expect(screen.queryByLabelText('Message to Fedi support')).not.toBeInTheDocument();
});
