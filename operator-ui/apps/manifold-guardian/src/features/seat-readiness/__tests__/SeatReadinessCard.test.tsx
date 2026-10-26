import type { ReadinessReport } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { guidance, labels, outcomes } from '@/shared/utils/seatReadiness';
import { SeatReadinessCard } from '../SeatReadinessCard';

const CHECKED_AT = Date.UTC(2026, 9, 1, 12, 34, 56);

const report = (overrides: Partial<ReadinessReport> = {}): ReadinessReport => ({
  checked_at_ms: CHECKED_AT,
  relay: 'pass',
  discovery: 'not_applicable',
  bitcoin: 'pass',
  uptime: 'pass',
  uptime_permille: null,
  ...overrides
});

const MINIMUMS = { '1d': 700, '7d': 900, '30d': 930 };

describe('SeatReadinessCard', () => {
  it('should count a check that does not apply as passing', () => {
    render(<SeatReadinessCard readiness={{ ready_for_new_seats: true, report: report() }} />);

    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
    expect(screen.getByText('Not checked in this deployment.')).toBeTruthy();
    expect(screen.getByText(/Last checked 2026-10-01 12:34 UTC/)).toBeTruthy();
  });

  it('should explain only the failing check', () => {
    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: false, report: report({ bitcoin: 'bitcoin_syncing' }) }}
      />
    );

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText('Bitcoin Core is still in initial block download.')).toBeTruthy();
    expect(screen.getByText(guidance.bitcoin)).toBeTruthy();
  });

  it('should explain a closed gate whose only failure is uptime', () => {
    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: false, report: report({ uptime: 'uptime_30d_low' }) }}
      />
    );

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText('Uptime')).toBeTruthy();
    expect(screen.getByText(outcomes.uptime_30d_low)).toBeTruthy();
    expect(screen.getByText(guidance.uptime)).toBeTruthy();
    expect(screen.queryByText(guidance.bitcoin)).toBeNull();
    expect(screen.queryByText(guidance.relay)).toBeNull();
  });

  it('should show each uptime window against its minimum and how long the failing ones need', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: false,
          report: report({
            uptime: 'uptime_7d_low',
            uptime_permille: {
              scores: { '1d': 784, '7d': 832, '30d': 914 },
              minimums: MINIMUMS
            }
          })
        }}
      />
    );

    const meters = screen.getAllByRole('meter');
    expect(meters.map((m) => m.getAttribute('aria-valuenow'))).toEqual(['78.4', '83.2', '91.4']);
    // 7·ln(0.168/0.1005) ≈ 3.6 days and 30·ln(0.086/0.0705) ≈ 6.0 days of unbroken uptime.
    expect(screen.getByText(/Passes after about 4 days/)).toBeTruthy();
    expect(screen.getByText(/Passes after about 6 days/)).toBeTruthy();
    expect(screen.getByText(/New seats reopen after about 6 days/)).toBeTruthy();
    expect(screen.getAllByText(/Passes after/)).toHaveLength(2);
    // The bars replace the generic uptime check.
    expect(screen.queryByText(outcomes.uptime_7d_low)).toBeNull();
  });

  it('should count a score equal to its minimum as passing', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: true,
          report: report({
            uptime_permille: { scores: { '1d': 700, '7d': 900, '30d': 930 }, minimums: MINIMUMS }
          })
        }}
      />
    );

    expect(screen.queryByText(/Passes after/)).toBeNull();
    expect(screen.getByText('Every window is above its minimum.')).toBeTruthy();
  });

  it('should show no uptime figures when the run could not record them', () => {
    render(<SeatReadinessCard readiness={{ ready_for_new_seats: true, report: report() }} />);

    expect(screen.queryAllByRole('meter')).toHaveLength(0);
    // Without figures the generic uptime check stays.
    expect(screen.getByText(labels.uptime)).toBeTruthy();
    expect(screen.queryByText(/any window under its minimum/)).toBeNull();
  });

  it('should show no uptime figures for a daemon that predates them', () => {
    const { uptime_permille: _, ...older } = report();
    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: true, report: older as ReadinessReport }}
      />
    );

    expect(screen.queryAllByRole('meter')).toHaveLength(0);
    // Without figures the generic uptime check stays.
    expect(screen.getByText(labels.uptime)).toBeTruthy();
    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
  });

  it('should take the verdict from the stored result before the first run after a restart', () => {
    render(<SeatReadinessCard readiness={{ ready_for_new_seats: false, report: null }} />);

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText(/No check has finished since this host started/)).toBeTruthy();
  });
});
