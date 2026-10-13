import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { LIST_POLL_MS } from '@/shared/api/pollingIntervals';
import { SEAT_FAN_OUT_LIMIT } from '@/shared/api/requestLimit';
import { useSeatReports } from '../useSeatReports';

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

it('should call SeatStatus once per seat id in parallel', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockImplementation((request) =>
    Promise.resolve({
      seat_id: (request as { SeatStatus: { seat_id: string } }).SeatStatus.seat_id
    })
  );

  const { result } = renderHook(() => useSeatReports(['seat-01', 'seat-02']), { wrapper });

  await waitFor(() => expect(result.current.every((query) => query.isSuccess)).toBe(true));
  expect(adminCallSpy).toHaveBeenCalledWith({ SeatStatus: { seat_id: 'seat-01' } });
  expect(adminCallSpy).toHaveBeenCalledWith({ SeatStatus: { seat_id: 'seat-02' } });
});

it('should return no queries for an empty seat id list', () => {
  const { result } = renderHook(() => useSeatReports([]), { wrapper });

  expect(result.current).toEqual([]);
});

it('should refetch every 5s while a seat is in a non-terminal phase', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-01',
    report: { state: 'active', health: 'healthy', phase: 'dkg_in_progress' }
  });

  renderHook(() => useSeatReports(['seat-01']), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

it('should back off a forming seat whose polls keep failing', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(1_000_000);
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValueOnce({
      seat_id: 'seat-01',
      report: { state: 'active', health: 'healthy', phase: 'dkg_in_progress' }
    })
    .mockRejectedValue(new Error('seat unreadable'));

  renderHook(() => useSeatReports(['seat-01']), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);

  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(3);

  // Two failures in, the cadence has doubled — this window passes with no call.
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(3);

  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(4);
});

// Regression: polling used to stop unless a report had already arrived, so a seat
// whose FIRST poll failed was never asked again — its row stayed blank for as long
// as the screen was open, with the one attempt that could have filled it spent.
it('should keep polling a seat whose first report never arrived, backing off as it fails', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockRejectedValue(new Error('seat unreadable'));

  renderHook(() => useSeatReports(['seat-never-answered']), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));

  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);

  // Two failures in, the cadence has doubled — this window passes with no call.
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);

  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(3);
});

it('should back off a settled seat whose health reads start failing, without giving up', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi
    .spyOn(adminCallModule, 'adminCall')
    .mockResolvedValueOnce({
      seat_id: 'seat-settled',
      report: {
        state: 'active',
        health: 'healthy',
        phase: 'running',
        invite_code: 'fed11testinvite'
      }
    })
    .mockRejectedValue(new Error('seat unreadable'));

  renderHook(() => useSeatReports(['seat-settled']), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(LIST_POLL_MS);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);

  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(3);

  // Two failures in, the cadence has doubled — this window passes with no call.
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(3);

  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(4);
});

it('should hold per-seat calls to the fan-out bound rather than opening one per seat at once', async () => {
  let inFlight = 0;
  let peak = 0;
  const seatIds = Array.from({ length: 12 }, (_, index) => `report-seat-${index}`);
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async () => {
    inFlight += 1;
    peak = Math.max(peak, inFlight);
    await new Promise((resolve) => setTimeout(resolve, 5));
    inFlight -= 1;
    return { seat_id: 'seat', report: { state: 'decommissioned', at_ms: 0 } };
  });

  const { result } = renderHook(() => useSeatReports(seatIds), { wrapper });

  await waitFor(() => expect(result.current.every((query) => query.isSuccess)).toBe(true));
  expect(peak).toBeLessThanOrEqual(SEAT_FAN_OUT_LIMIT);
  expect(result.current).toHaveLength(seatIds.length);
});

// Regression (W2.3): a row that reached `running` used to stop being polled, so
// its health chip froze at the health of its last fetch.
it('should slow to the health cadence once a seat is running, not stop', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-running',
    report: { state: 'active', health: 'healthy', phase: 'running', invite_code: 'fed11testinvite' }
  });

  renderHook(() => useSeatReports(['seat-running']), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);

  await vi.advanceTimersByTimeAsync(LIST_POLL_MS - 5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(2);
});

it('should stop polling a decommissioned seat', async () => {
  vi.useFakeTimers();
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    seat_id: 'seat-01',
    report: { state: 'decommissioned', at_ms: 0 }
  });

  renderHook(() => useSeatReports(['seat-01']), { wrapper });

  await vi.waitFor(() => expect(adminCallSpy).toHaveBeenCalledTimes(1));
  await vi.advanceTimersByTimeAsync(5_000);
  expect(adminCallSpy).toHaveBeenCalledTimes(1);
});
