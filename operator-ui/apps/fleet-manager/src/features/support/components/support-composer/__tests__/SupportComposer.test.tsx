import type { AdminRequest } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { SupportComposer } from '../SupportComposer';

const renderComposer = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SupportComposer />
    </QueryClientProvider>
  );
};

const box = () => screen.getByLabelText('Message to Fedi support');
const type = (value: string) => fireEvent.change(box(), { target: { value } });

afterEach(() => {
  vi.restoreAllMocks();
});

it('should send the trimmed text on Enter and clear the box', async () => {
  const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({} as never);
  renderComposer();

  type('  Seat 2 is down\n');
  fireEvent.keyDown(box(), { key: 'Enter' });

  await waitFor(() => expect(box()).toHaveValue(''));
  expect(adminCall).toHaveBeenCalledWith({
    SendSupportMessage: { body: 'Seat 2 is down' }
  } satisfies AdminRequest);
});

it('should not send on Shift+Enter or with nothing written', () => {
  const adminCall = vi.spyOn(adminCallModule, 'adminCall');
  renderComposer();

  type('First line');
  fireEvent.keyDown(box(), { key: 'Enter', shiftKey: true });
  type('   ');
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));

  expect(adminCall).not.toHaveBeenCalled();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

// The daemon counts Unicode scalar values. 4000 emoji are 8000 UTF-16 units,
// which a `.length` check would wrongly refuse; 4001 are one too many.
it('should count characters the way the daemon does', async () => {
  const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({} as never);
  renderComposer();

  type('😀'.repeat(4001));
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  expect(screen.getByRole('alert')).toHaveTextContent(
    'A message can have at most 4000 characters.'
  );
  expect(adminCall).not.toHaveBeenCalled();

  type('😀'.repeat(4000));
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(adminCall).toHaveBeenCalledTimes(1));
});

it('should show the daemon refusal and keep the text to send again', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(
    new AdminApiError('No Nostr relay accepted the message. Try again.')
  );
  renderComposer();

  type('Any update?');
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));

  expect(await screen.findByRole('alert')).toHaveTextContent(
    'No Nostr relay accepted the message. Try again.'
  );
  expect(box()).toHaveValue('Any update?');
});
