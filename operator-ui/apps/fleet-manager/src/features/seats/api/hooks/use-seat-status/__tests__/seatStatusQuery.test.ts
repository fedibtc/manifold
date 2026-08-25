import type { SeatReport, SeatStatusResponse } from '@operator-ui/types';
import { afterEach, beforeEach, vi } from 'vitest';
import { LIST_POLL_MS, SEAT_FORMATION_POLL_MS } from '@/shared/api/pollingIntervals';
import {
  seatStatusKey,
  seatStatusQueryOptions,
  seatStatusRefetchInterval
} from '../seatStatusQuery';

// Only `report` is read by the polling policy; the rest of the response is not
// what this module decides on, so it is not built here.
const answered = (seatId: string, report: SeatReport) => ({
  queryKey: seatStatusKey(seatId),
  state: {
    status: 'success' as const,
    errorUpdateCount: 0,
    data: { report } as SeatStatusResponse
  }
});

const unanswered = (seatId: string) => ({
  queryKey: seatStatusKey(seatId),
  state: { status: 'error' as const, errorUpdateCount: 1 }
});

const RUNNING: SeatReport = {
  state: 'active',
  health: 'healthy',
  phase: 'running',
  invite_code: 'fed11testinvite'
};

// Midpoint jitter, so every interval below is exactly its nominal value.
beforeEach(() => {
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
});

afterEach(() => {
  vi.restoreAllMocks();
});

// Regression (W2.3): `running` used to end polling outright, on the ground that
// a settled seat stops changing. Health is the other axis of the same report and
// it never settles, so a guardian that died after formation kept rendering the
// health of its last fetch.
it('should keep a running seat on a numeric interval rather than stopping', () => {
  const interval = seatStatusRefetchInterval(answered('seat-running', RUNNING));

  expect(typeof interval).toBe('number');
  expect(interval).toBe(LIST_POLL_MS);
});

it('should poll a forming seat at the formation cadence', () => {
  const interval = seatStatusRefetchInterval(
    answered('seat-forming', { state: 'active', health: 'healthy', phase: 'dkg_in_progress' })
  );

  expect(interval).toBe(SEAT_FORMATION_POLL_MS);
});

it('should stop polling only a decommissioned seat', () => {
  const interval = seatStatusRefetchInterval(
    answered('seat-gone', { state: 'decommissioned', at_ms: 0 })
  );

  expect(interval).toBe(false);
});

it('should keep polling a seat whose read has never answered', () => {
  const interval = seatStatusRefetchInterval(unanswered('seat-silent'));

  expect(typeof interval).toBe('number');
});

it('should give the list and the detail page one cache key and one polling policy', () => {
  const detail = seatStatusQueryOptions('seat-01');
  const list = seatStatusQueryOptions('seat-01', { fanOut: true });

  expect(detail.queryKey).toEqual(list.queryKey);
  expect(detail.refetchInterval).toBe(list.refetchInterval);
});
