import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError, AuthError, NetworkError } from '@/shared/api/errors';
import { SetupRestore } from '../SetupRestore';

const PHRASE = 'abandon abandon abandon abandon abandon abandon abandon abandon about';

const renderRestore = (onRestored = vi.fn(), onCancel = vi.fn()) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SetupRestore onRestored={onRestored} onCancel={onCancel} />
    </QueryClientProvider>
  );
  return { onRestored, onCancel, client };
};

const submitButton = () =>
  screen.getByRole('button', { name: 'Recover this fleet' }) as HTMLButtonElement;

const acknowledgement = () => screen.getByLabelText(/permanently offline/i) as HTMLInputElement;

const fillAndSubmit = () => {
  fireEvent.change(screen.getByLabelText('Recovery phrase'), { target: { value: PHRASE } });
  fireEvent.click(acknowledgement());
  fireEvent.click(submitButton());
};

const restored = { onboarded: 'restored', seats: 2, formed: 1 };

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SetupRestore', () => {
  it('should block submitting until both the phrase and the acknowledgement are given', () => {
    renderRestore();

    expect(submitButton().disabled).toBe(true);

    fireEvent.change(screen.getByLabelText('Recovery phrase'), { target: { value: PHRASE } });
    expect(submitButton().disabled).toBe(true);

    fireEvent.click(acknowledgement());
    expect(submitButton().disabled).toBe(false);
  });

  it('should send the phrase with the acknowledgement the daemon requires', async () => {
    const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(restored);
    renderRestore();

    fireEvent.change(screen.getByLabelText('Recovery phrase'), {
      target: { value: ` ${PHRASE} ` }
    });
    fireEvent.click(acknowledgement());
    fireEvent.click(submitButton());

    await waitFor(() =>
      expect(adminCallSpy).toHaveBeenCalledWith({
        OnboardFromBackup: { mnemonic: PHRASE, acknowledge_original_host_is_gone: true }
      })
    );
  });

  it('should warn that two hosts on one identity equivocate', () => {
    renderRestore();

    expect(screen.getByText(/equivocate/i)).toBeTruthy();
  });

  it('should go back to the doors without restoring', () => {
    const { onCancel } = renderRestore();

    fireEvent.click(screen.getByRole('button', { name: 'Back' }));

    expect(onCancel).toHaveBeenCalled();
  });

  it('should disable browser text services on the phrase field', () => {
    renderRestore();

    const field = screen.getByLabelText('Recovery phrase') as HTMLTextAreaElement;
    expect(field.getAttribute('autocomplete')).toBe('off');
    expect(field.getAttribute('autocapitalize')).toBe('none');
    expect(field.getAttribute('autocorrect')).toBe('off');
    // The attribute, not the `spellcheck` IDL property: jsdom does not implement
    // that property, so reading it would assert nothing.
    expect(field.getAttribute('spellcheck')).toBe('false');
  });

  it('should show the success state with the daemon counts', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(restored);
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery finished' });
    expect(screen.getByText(/seat records recovered/i)).toBeTruthy();
  });

  it('should keep showing the result after the mutation is reset', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(restored);
    const { client } = renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery finished' });

    // The mutation is reset once its result is copied into view state, so an idle
    // mutation must not be able to send the screen back to the form.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByRole('heading', { name: 'Recovery finished' })).toBeTruthy();

    // The reset is also what evicts the phrase: this screen stays mounted, so
    // without it the mutation keeps an observer and gcTime: 0 never collects the
    // `variables` that carried the phrase.
    await waitFor(() => expect(client.getMutationCache().getAll()).toHaveLength(0));
  });

  it('should only continue when the operator asks', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(restored);
    const { onRestored } = renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery finished' });
    expect(onRestored).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(onRestored).toHaveBeenCalled();
  });

  it('should show a daemon refusal as a full screen', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AdminApiError('invalid mnemonic'));
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery did not complete' });
    expect(screen.getByText('invalid mnemonic')).toBeTruthy();
  });

  it('should keep the phrase for the retry a daemon refusal invites', async () => {
    // Most of what the daemon refuses with is a fault outside the phrase — a seat
    // directory an earlier attempt left behind, an archive not yet on the relays —
    // which the operator clears and then retries with the very same twelve words.
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(
      new AdminApiError('seat seat-1 already has a directory on this host')
    );
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery did not complete' });
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

    const field = (await screen.findByLabelText('Recovery phrase')) as HTMLTextAreaElement;
    expect(field.value).toBe(PHRASE);
  });

  it('should make the operator acknowledge again before a second attempt', async () => {
    // The acknowledgement gates against two hosts running one guardian identity,
    // which the screen itself says no check can catch. Carrying a tick from the
    // previous attempt would let a retry inherit an assertion nobody made twice.
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AdminApiError('invalid mnemonic'));
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery did not complete' });
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));

    await screen.findByRole('heading', { name: 'Recover from your phrase' });
    expect(acknowledgement().checked).toBe(false);
    expect(submitButton().disabled).toBe(true);
  });

  it('should clear the phrase before showing the unknown result', async () => {
    // The unknown screen waits indefinitely for an explicit status check, so it is
    // the one branch that must never hold the phrase.
    vi.spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new NetworkError())
      .mockRejectedValue(
        new AdminApiError(
          'this Fleet Manager has not been onboarded yet: run `admin onboard new` or `admin onboard restore`',
          'not_onboarded'
        )
      );
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery result unknown' });
    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));

    await screen.findByRole('heading', { name: 'Recover from your phrase' });
    const field = screen.getByLabelText('Recovery phrase') as HTMLTextAreaElement;
    expect(field.value).toBe('');
  });

  it('should clear the phrase when the operator goes back to the setup options', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AdminApiError('invalid mnemonic'));
    const { onCancel } = renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery did not complete' });
    fireEvent.click(screen.getByRole('button', { name: 'Back to setup options' }));

    expect(onCancel).toHaveBeenCalled();
  });

  it('should not let the operator leave while the restore is in flight', async () => {
    // Leaving unmounts the screen, which drops the mutation's observer: the restore
    // lands with nothing left to report it.
    vi.spyOn(adminCallModule, 'adminCall').mockReturnValue(new Promise(() => {}));
    renderRestore();
    fillAndSubmit();

    await waitFor(() =>
      expect((screen.getByRole('button', { name: 'Back' }) as HTMLButtonElement).disabled).toBe(
        true
      )
    );
  });

  it('should show the unknown result for a transport failure', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new NetworkError());
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery result unknown' });
  });

  it('should continue without counts when the status check finds an identity', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new NetworkError())
      .mockResolvedValue({
        fman_name: 'mutual-hamster',
        service_pubkey: '02abc',
        service_nostr_pubkey: 'a'.repeat(64),
        nostr: { state: 'not_observed', checked_at: 1_760_000_000 }
      });
    const { onRestored } = renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery result unknown' });
    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));

    await screen.findByText(/recovery counts are not available/i);
    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

    expect(onRestored).toHaveBeenCalled();
    expect(adminCall).toHaveBeenCalledWith('Onboarding');
  });

  it('should return to the form when the status check says the host is not onboarded', async () => {
    vi.spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new NetworkError())
      .mockRejectedValue(
        new AdminApiError(
          'this Fleet Manager has not been onboarded yet: run `admin onboard new` or `admin onboard restore`',
          'not_onboarded'
        )
      );
    renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery result unknown' });
    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));

    await screen.findByRole('heading', { name: 'Recover from your phrase' });
  });

  it('should raise the sign-in gate when the status check is refused', async () => {
    // The check calls the daemon directly, so nothing else can put its 401 in front
    // of the Onboarding query, and the gate reads that query alone.
    vi.spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new NetworkError())
      .mockRejectedValue(new AuthError());
    const { client } = renderRestore();
    const refetch = vi.spyOn(client, 'refetchQueries');
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery result unknown' });
    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));

    await waitFor(() =>
      expect(refetch).toHaveBeenCalledWith({ queryKey: ['onboarding'], exact: true })
    );
  });

  it('should ask the daemon again rather than reuse a request already in flight', async () => {
    // react-query answers a `fetchQuery` with whatever request is already running
    // for the key. During setup that is a poll issued before the restore, so the
    // check has to call the daemon itself.
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockRejectedValueOnce(new NetworkError())
      .mockResolvedValue({
        fman_name: 'mutual-hamster',
        service_pubkey: '02abc',
        service_nostr_pubkey: 'a'.repeat(64),
        nostr: { state: 'not_observed', checked_at: 1_760_000_000 }
      });
    const { client } = renderRestore();
    fillAndSubmit();

    await screen.findByRole('heading', { name: 'Recovery result unknown' });

    // A poll for the same key is already running and will never settle.
    const stalled = new Promise(() => {});
    client.fetchQuery({ queryKey: ['onboarding'], queryFn: () => stalled });
    const callsBefore = adminCall.mock.calls.length;

    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));

    await screen.findByText(/recovery counts are not available/i);
    expect(adminCall.mock.calls.length).toBe(callsBefore + 1);
  });

  it('should refresh the onboarding query when authentication is refused', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new AuthError());
    const { client } = renderRestore();
    const refetch = vi.spyOn(client, 'refetchQueries');
    fillAndSubmit();

    await waitFor(() =>
      expect(refetch).toHaveBeenCalledWith({ queryKey: ['onboarding'], exact: true })
    );

    const field = (await screen.findByLabelText('Recovery phrase')) as HTMLTextAreaElement;
    expect(field.value).toBe('');
  });
});
