import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { BackupPhrasePage } from '../BackupPhrasePage';

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return {
    ...render(
      <QueryClientProvider client={client}>
        <MemoryRouter>
          <BackupPhrasePage />
        </MemoryRouter>
      </QueryClientProvider>
    ),
    client
  };
};

afterEach(() => {
  vi.restoreAllMocks();
});

it('should not fetch the mnemonic until the operator confirms', () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ mnemonic: 'a b c' });
  renderPage();

  expect(adminCallSpy).not.toHaveBeenCalled();
});

it('should fetch the phrase once per confirmation', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue({ mnemonic: 'abandon abandon about' });
  renderPage();

  fireEvent.click(screen.getByRole('button', { name: 'Reveal phrase' }));

  await waitFor(() => screen.getByText('abandon abandon about'));
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});

it('should not tell the operator the phrase can only ever be seen once', async () => {
  // ShowMnemonic is a deliberately repeatable recovery verb (crates/fman/core/src/admin.rs):
  // it answers with the phrase on every call. This page used to promise the opposite, so an
  // operator who missed the words believed they were gone and one who saw them leak believed
  // the exposure could not repeat. Both are wrong, and both change what they do next.
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ mnemonic: 'abandon abandon about' });
  renderPage();

  fireEvent.click(screen.getByRole('button', { name: 'Reveal phrase' }));

  await waitFor(() => screen.getByText('abandon abandon about'));
  expect(screen.queryByText(/exactly once|never re-displayed/i)).toBeNull();
});

it('should tell the operator the phrase can be revealed again', async () => {
  // Saying nothing is not neutral. An operator who closes the tab halfway through
  // writing the words down has to guess whether they are recoverable, and the
  // cautious guess — that they are gone — is the wrong one.
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ mnemonic: 'abandon abandon about' });
  renderPage();

  fireEvent.click(screen.getByRole('button', { name: 'Reveal phrase' }));

  await waitFor(() => screen.getByText('abandon abandon about'));
  screen.getByText(/come back and reveal it again/i);
});

it('should say before revealing that leaving the page hides the phrase', () => {
  renderPage();

  screen.getByText(/leaving the page hides it again/i);
});

it('should say the twelve words are a complete backup, restorable only at setup', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ mnemonic: 'abandon abandon about' });
  renderPage();

  fireEvent.click(screen.getByRole('button', { name: 'Reveal phrase' }));

  await screen.findByText(/twelve words are a complete backup/i);
  screen.getByText(/only during that host's setup/i);
});

it('should keep no copy of the phrase once the screen has gone', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ mnemonic: 'abandon abandon about' });
  const { client, unmount } = renderPage();

  fireEvent.click(screen.getByRole('button', { name: 'Reveal phrase' }));
  await waitFor(() => screen.getByText('abandon abandon about'));

  unmount();

  // This screen revealed the phrase, so it also disposes of it: nothing may hold
  // the mnemonic once the operator has left.
  await waitFor(() => expect(client.getMutationCache().getAll()).toHaveLength(0));
});
