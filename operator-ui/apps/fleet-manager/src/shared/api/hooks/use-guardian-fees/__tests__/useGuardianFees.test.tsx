import type { GuardianFeesResponse } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, vi } from 'vitest';
import { walletStatus } from '@/mocks/wallet-status';
import * as adminCallModule from '@/shared/api/adminCall';
import { SEAT_FAN_OUT_LIMIT } from '@/shared/api/requestLimit';
import { useGuardianFees } from '../useGuardianFees';

const guardianFeesResponse = (
  seat_id: string,
  overrides: Partial<GuardianFeesResponse> = {}
): GuardianFeesResponse => ({
  seat_id,
  federation_id: 'fed1',
  remittance_account: '{}',
  collectable_msat: 0,
  staged_msat: 0,
  locked_msat: 0,
  idle_msat: 0,
  wallet: walletStatus(0),
  lifetime_remitted_msat: 0,
  policy: {
    configured: true,
    send_ppm: 1_000,
    recipients: null,
    share_matches_policy: true,
    our_weight: 1,
    total_weight: 4
  },
  remittances: [],
  ...overrides
});

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

// Midpoint jitter, drawn before any seat draws its own offset, so every interval
// below is exactly its nominal value.
beforeEach(() => {
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('should call GuardianFees once per seat id in parallel', async () => {
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockImplementation((request) =>
      Promise.resolve(
        guardianFeesResponse(
          (request as { GuardianFees: { seat_id: string } }).GuardianFees.seat_id
        )
      )
    );

  const { result } = renderHook(() => useGuardianFees(['seat-01', 'seat-02']), { wrapper });

  await waitFor(() => expect(result.current.every((query) => query.isSuccess)).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith({ GuardianFees: { seat_id: 'seat-01', limit: null } });
  expect(adminCallSpy).toHaveBeenCalledWith({ GuardianFees: { seat_id: 'seat-02', limit: null } });
});

it('should refetch every 60s', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue(guardianFeesResponse('seat-01'));

  renderHook(() => useGuardianFees(['seat-01']), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(60_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

it('should back off a seat that keeps failing instead of holding the healthy cadence', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockRejectedValue(new Error('no fee account'));

  renderHook(() => useGuardianFees(['seat-01']), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(60_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);

  await vi.advanceTimersByTimeAsync(60_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);

  await vi.advanceTimersByTimeAsync(60_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(3);
});

// react-query clears and restarts the poll timer whenever the interval it
// computes differs from the running one, and it recomputes on every render. A
// jitter drawn per call would reset the timer on every render of this screen,
// which renders far more often than once a minute, and the fees would never
// refresh at all.
it('should keep polling across renders', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValue(guardianFeesResponse('seat-01'));

  const { rerender } = renderHook(() => useGuardianFees(['seat-01']), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(30_000);
  rerender();
  await vi.advanceTimersByTimeAsync(20_000);
  rerender();
  await vi.advanceTimersByTimeAsync(10_000);

  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

it('should hold per-seat calls to the fan-out bound rather than opening one per seat at once', async () => {
  let inFlight = 0;
  let peak = 0;
  const seatIds = Array.from({ length: 12 }, (_, index) => `seat-${index}`);
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async () => {
    inFlight += 1;
    peak = Math.max(peak, inFlight);
    await new Promise((resolve) => setTimeout(resolve, 5));
    inFlight -= 1;
    return guardianFeesResponse('seat');
  });

  const { result } = renderHook(() => useGuardianFees(seatIds), { wrapper });

  await waitFor(() => expect(result.current.every((query) => query.isSuccess)).toBe(true));
  expect(peak).toBeLessThanOrEqual(SEAT_FAN_OUT_LIMIT);
  expect(result.current).toHaveLength(seatIds.length);
});
