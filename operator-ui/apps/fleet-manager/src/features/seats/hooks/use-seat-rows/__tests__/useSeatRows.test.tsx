import type { SeatStatusResponse, SeatSummary } from '@operator-ui/types';
import { renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('@/shared/api/hooks/use-seats/useSeats', () => ({ useSeats: vi.fn() }));
vi.mock('@/features/seats/api/hooks/use-seat-reports/useSeatReports', () => ({
  useSeatReports: vi.fn()
}));

import { useSeatReports } from '@/features/seats/api/hooks/use-seat-reports/useSeatReports';
import { useSeats } from '@/shared/api/hooks/use-seats/useSeats';
import { useSeatRows } from '../useSeatRows';

const seat = (id: string, decommissioned: boolean): SeatSummary => ({
  seat_id: id,
  fi_id: `fi-${id}`,
  plan: { InfiniteBestEffort: { price_msats: 50_000_000 } },
  created_at_ms: 1_753_000_000_000,
  payment_claim: { state: 'success', at_ms: 0 },
  completion_callback: { state: 'not_configured' },
  decommissioned,
  backup: null
});

const activeReport: SeatStatusResponse['report'] = {
  state: 'active',
  health: 'healthy',
  phase: 'running',
  invite_code: 'fed11testinvite'
};

const mockSeats = (seats: SeatSummary[]): void => {
  vi.mocked(useSeats).mockReturnValue({ data: { seats } } as ReturnType<typeof useSeats>);
};

const mockReports = (reports: unknown[]): void => {
  vi.mocked(useSeatReports).mockReturnValue(reports as ReturnType<typeof useSeatReports>);
};

describe('useSeatRows', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('should report empty when there are no seats', () => {
    mockSeats([]);
    mockReports([]);

    const { result } = renderHook(() => useSeatRows());

    expect(result.current.isEmpty).toBe(true);
    expect(result.current.rows).toHaveLength(0);
  });

  it('should split active and decommissioned counts and join reports to active seats', () => {
    mockSeats([seat('01', false), seat('02', true)]);
    mockReports([{ data: { report: activeReport }, isPending: false }]);

    const { result } = renderHook(() => useSeatRows());

    expect(result.current.activeCount).toBe(1);
    expect(result.current.decommissionedCount).toBe(1);
    expect(result.current.isEmpty).toBe(false);
    expect(result.current.rows).toHaveLength(2);
    expect(result.current.rows[0].report).toEqual(activeReport);
    expect(result.current.rows[1].report).toBeUndefined();
  });

  it('should mark a seat whose report is still fetching as loading', () => {
    mockSeats([seat('01', false)]);
    mockReports([{ data: undefined, isPending: true }]);

    const { result } = renderHook(() => useSeatRows());

    expect(result.current.rows[0].reportLoading).toBe(true);
    expect(result.current.rows[0].report).toBeUndefined();
  });
});
