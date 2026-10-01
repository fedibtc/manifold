import type { ReadinessReport } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { guidance } from '@/shared/utils/seatReadiness';
import { SeatReadinessCard } from '../SeatReadinessCard';

const CHECKED_AT = Date.UTC(2026, 9, 1, 12, 34, 56);
const MINUTE = 60_000;

const report = (overrides: Partial<ReadinessReport> = {}): ReadinessReport => ({
  checked_at_ms: CHECKED_AT,
  relay: 'pass',
  discovery: 'not_applicable',
  bitcoin: 'pass',
  ...overrides
});

describe('SeatReadinessCard', () => {
  it('should count a check that does not apply as passing', () => {
    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: true, report: report() }}
        nowMs={CHECKED_AT + MINUTE}
      />
    );

    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
    expect(screen.getByText('Not checked in this deployment.')).toBeTruthy();
    expect(screen.getByText(/Last checked 2026-10-01 12:34 UTC/)).toBeTruthy();
    expect(screen.queryByText('Checks may have stopped')).toBeNull();
  });

  it('should explain only the failing check', () => {
    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: false, report: report({ bitcoin: 'bitcoin_syncing' }) }}
        nowMs={CHECKED_AT + MINUTE}
      />
    );

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText('Bitcoin Core is still in initial block download.')).toBeTruthy();
    expect(screen.getByText(guidance.bitcoin)).toBeTruthy();
    expect(screen.queryByText(guidance.relay)).toBeNull();
  });

  it('should take the verdict from the stored result before the first run after a restart', () => {
    render(
      <SeatReadinessCard readiness={{ ready_for_new_seats: false, report: null }} nowMs={0} />
    );

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText(/No check has finished since this host started/)).toBeTruthy();
  });

  it('should warn when the last check is older than a ready interval allows', () => {
    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: true, report: report() }}
        nowMs={CHECKED_AT + 16 * MINUTE}
      />
    );

    expect(screen.getByText('Checks may have stopped')).toBeTruthy();
  });
});
