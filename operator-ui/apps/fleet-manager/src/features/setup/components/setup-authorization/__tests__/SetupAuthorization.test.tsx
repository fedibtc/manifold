import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MOCK_HOLDER_PUBKEY, MOCK_SERVICE_NOSTR_PUBKEY } from '@/mocks/world/keys';
import * as adminCallModule from '@/shared/api/adminCall';
import { AdminApiError } from '@/shared/api/errors';
import { ONBOARDING_KEY } from '@/shared/api/hooks/use-onboarding/useOnboarding';
import { SetupAuthorization } from '../SetupAuthorization';

const waiting = {
  service_pubkey: '02abc',
  service_nostr_pubkey: MOCK_SERVICE_NOSTR_PUBKEY,
  nostr: { state: 'not_observed', checked_at: 1_760_000_000 }
};

const observed = {
  ...waiting,
  nostr: {
    state: 'authorization_observed',
    authorizations: [],
    holders: [MOCK_HOLDER_PUBKEY],
    checked_at: 1_760_000_000
  }
};

const renderAuthorization = (onSettled = vi.fn(), initial = waiting) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(ONBOARDING_KEY, initial);
  render(
    <QueryClientProvider client={client}>
      <SetupAuthorization onSettled={onSettled} />
    </QueryClientProvider>
  );
  return { onSettled };
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SetupAuthorization', () => {
  it('should contact the relay only when the operator checks now', async () => {
    const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValueOnce(waiting);
    renderAuthorization();

    await screen.findByText(MOCK_SERVICE_NOSTR_PUBKEY);
    expect(adminCall).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Check now' }));
    await vi.waitFor(() => expect(adminCall).toHaveBeenCalledWith('RefreshHolderAuthorizations'));
  });

  it('should show the key an attester signs over', async () => {
    renderAuthorization();

    await screen.findByText(MOCK_SERVICE_NOSTR_PUBKEY);
  });

  it('should say it is waiting while no authorization has been observed', async () => {
    renderAuthorization();

    await screen.findByText(/no authorization for this fleet/i);
    expect((screen.getByRole('button', { name: 'Continue' }) as HTMLButtonElement).disabled).toBe(
      true
    );
  });

  // Fake timers never advance here, so no automatic tick can stand in for the
  // click: the calls after it are the ones the operator asked for.
  it('should force an immediate poll when the operator checks now', async () => {
    vi.useFakeTimers();
    try {
      const adminCall = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(waiting);
      renderAuthorization();

      await vi.waitFor(() => screen.getByText(MOCK_SERVICE_NOSTR_PUBKEY));
      const button = screen.getByRole('button', { name: 'Check now' });
      const callsBeforeClick = adminCall.mock.calls.length;

      await act(async () => {
        fireEvent.click(button);
      });

      expect(adminCall.mock.calls.slice(callsBeforeClick).map(([request]) => request)).toEqual([
        'RefreshHolderAuthorizations'
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('should retain the last observed state when a manual refresh fails', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(
      new AdminApiError('authorization watch unavailable')
    );
    renderAuthorization();

    fireEvent.click(screen.getByRole('button', { name: 'Check now' }));
    await vi.waitFor(() =>
      expect(screen.getByText(/no authorization for this fleet/i)).toBeTruthy()
    );
  });

  it('should continue on its own once the authorization is observed', async () => {
    vi.useFakeTimers();
    try {
      const { onSettled } = renderAuthorization(vi.fn(), observed);

      await vi.waitFor(() => expect(screen.getByRole('status').textContent).toMatch(/observed/i));
      expect(screen.queryByRole('button', { name: 'Skip for now' })).toBeNull();
      expect(onSettled).not.toHaveBeenCalled();

      await act(async () => {
        vi.advanceTimersByTime(2000);
      });

      expect(onSettled).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('should continue once when the timer and the manual action race', async () => {
    vi.useFakeTimers();
    try {
      const { onSettled } = renderAuthorization(vi.fn(), observed);

      const button = await vi.waitFor(() => screen.getByRole('button', { name: 'Continue now' }));

      await act(async () => {
        fireEvent.click(button);
        vi.advanceTimersByTime(2000);
      });

      expect(onSettled).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('should clear its timer when it unmounts', async () => {
    vi.useFakeTimers();
    try {
      vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue(observed);
      const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      client.setQueryData(ONBOARDING_KEY, observed);
      const onSettled = vi.fn();
      const { unmount } = render(
        <QueryClientProvider client={client}>
          <SetupAuthorization onSettled={onSettled} />
        </QueryClientProvider>
      );

      await vi.waitFor(() => expect(screen.getByRole('status')).toBeTruthy());
      unmount();

      await act(async () => {
        vi.advanceTimersByTime(5000);
      });

      expect(onSettled).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});
