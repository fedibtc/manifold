import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { SetupPhrase } from '../SetupPhrase';

const renderPhrase = (onSaved = vi.fn()) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SetupPhrase onSaved={onSaved} />
    </QueryClientProvider>
  );
  return { onSaved };
};

const continueButton = () =>
  screen.getByRole('button', { name: "I've written it down — continue" }) as HTMLButtonElement;

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SetupPhrase', () => {
  it('should not fetch the phrase until the operator asks for it', () => {
    const adminCallSpy = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ mnemonic: 'a b c' });
    renderPhrase();

    expect(adminCallSpy).not.toHaveBeenCalled();
  });

  it('should block continuing until the phrase has been revealed', () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ mnemonic: 'a b c' });
    renderPhrase();

    expect(continueButton().disabled).toBe(true);
  });

  it('should show the phrase and allow continuing once revealed', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ mnemonic: 'abandon abandon about' });
    const { onSaved } = renderPhrase();

    fireEvent.click(screen.getByRole('button', { name: 'Reveal phrase' }));

    await screen.findByText('abandon abandon about');
    await waitFor(() => expect(continueButton().disabled).toBe(false));
    fireEvent.click(continueButton());
    expect(onSaved).toHaveBeenCalled();
  });

  it('should warn that the phrase is the only backup', () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ mnemonic: 'a b c' });
    renderPhrase();

    expect(screen.getByText(/only backup/i)).toBeTruthy();
  });
});
