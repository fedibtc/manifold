import type { AdminRequest } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AutoUpdateCard } from '../AutoUpdateCard';

let storedEnabled = true;
let writeError: Error | null = null;

const mockDaemon = () =>
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request: AdminRequest) => {
    if (request === 'ShowAutoUpdate') return { enabled: storedEnabled } as never;
    if (typeof request !== 'string' && 'SetAutoUpdate' in request) {
      if (writeError) throw writeError;
      storedEnabled = request.SetAutoUpdate.enabled;
      return { enabled: storedEnabled } as never;
    }
    throw new Error('unexpected admin request');
  });

const renderCard = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <AutoUpdateCard />
    </QueryClientProvider>
  );
};

afterEach(() => {
  vi.restoreAllMocks();
  storedEnabled = true;
  writeError = null;
});

it('should show automatic updates as on when the daemon allows them', async () => {
  mockDaemon();
  renderCard();

  expect(await screen.findByLabelText('Install updates automatically')).toBeChecked();
  expect(screen.queryByText(/Automatic updates are off/)).not.toBeInTheDocument();
});

it('should turn automatic updates off and say so', async () => {
  const adminCall = mockDaemon();
  renderCard();
  const box = await screen.findByLabelText('Install updates automatically');

  fireEvent.click(box);

  await waitFor(() =>
    expect(adminCall).toHaveBeenCalledWith({ SetAutoUpdate: { enabled: false } })
  );
  await waitFor(() => expect(box).not.toBeChecked());
  expect(await screen.findByText(/Automatic updates are off/)).toBeInTheDocument();
});

it('should turn automatic updates back on from a stored opt-out', async () => {
  storedEnabled = false;
  const adminCall = mockDaemon();
  renderCard();
  const box = await screen.findByLabelText('Install updates automatically');
  expect(box).not.toBeChecked();

  fireEvent.click(box);

  await waitFor(() => expect(adminCall).toHaveBeenCalledWith({ SetAutoUpdate: { enabled: true } }));
  await waitFor(() => expect(box).toBeChecked());
  expect(screen.queryByText(/Automatic updates are off/)).not.toBeInTheDocument();
});

it('should keep the stored choice and report a refused write', async () => {
  writeError = new Error('database is locked');
  mockDaemon();
  renderCard();
  const box = await screen.findByLabelText('Install updates automatically');

  fireEvent.click(box);

  expect(await screen.findByText('database is locked')).toBeInTheDocument();
  expect(box).toBeChecked();
});
