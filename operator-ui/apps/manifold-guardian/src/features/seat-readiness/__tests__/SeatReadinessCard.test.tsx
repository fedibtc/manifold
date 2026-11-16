import type { ReadinessReport } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import {
  guidance,
  labels,
  pressureGuidance,
  outcomes,
  updateGuidance
} from '@/shared/utils/seatReadiness';
import { SeatReadinessCard } from '../SeatReadinessCard';

const CHECKED_AT = Date.UTC(2026, 9, 1, 12, 34, 56);

const report = (overrides: Partial<ReadinessReport> = {}): ReadinessReport => ({
  checked_at_ms: CHECKED_AT,
  relay: 'pass',
  discovery: 'not_applicable',
  bitcoin: 'pass',
  version: 'not_applicable',
  pressure: 'not_applicable',
  host_pressure: { cpu: { '6h': 12, '24h': 14 }, io: { '6h': null, '24h': null } },
  host_pressure_limit: null,
  uptime: 'pass',
  uptime_comparison: null,
  ...overrides
});

const LIMIT = { cpu: { block_above: 15, resume_below: 10 }, io: { block_above: 10, resume_below: 5 } };

const MINIMUMS = { '1d': 700, '7d': 900, '30d': 930 };

describe('SeatReadinessCard', () => {
  it('should count a check that does not apply as passing', () => {
    render(<SeatReadinessCard readiness={{ ready_for_new_seats: true, report: report() }} />);

    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
    expect(screen.getByText('Not checked in this deployment.')).toBeTruthy();
    // No published minimum is said as such, not as a check that is off.
    expect(screen.getByText('No minimum set.')).toBeTruthy();
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
    expect(screen.queryByText(updateGuidance, { exact: false })).toBeNull();
  });

  it('should tell the host to update when its version is below the minimum', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: false,
          report: report({ version: 'mg_version_too_old' })
        }}
      />
    );

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText(labels.version)).toBeTruthy();
    expect(screen.getByText('Update required.')).toBeTruthy();
    expect(screen.getByText(updateGuidance, { exact: false })).toBeTruthy();
    expect(screen.queryByText(guidance.uptime)).toBeNull();
  });

  it('should show what a held host waits for and explain the hold', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: false,
          report: report({
            pressure: 'host_pressure_high',
            host_pressure: { cpu: { '6h': 92, '24h': 134 }, io: { '6h': 12, '24h': 18 } },
            host_pressure_limit: LIMIT
          })
        }}
      />
    );

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText(labels.pressure)).toBeTruthy();
    expect(screen.getByText(outcomes.host_pressure_high)).toBeTruthy();
    // Held, the host waits on the 24-hour averages, whatever the 6-hour ones are.
    expect(
      screen.getByText(
        '24-hour average: CPU 13.4%, disk 1.8%. New seats reopen at CPU 10% and disk 5% or less.'
      )
    ).toBeTruthy();
    expect(screen.getByText(pressureGuidance, { exact: false })).toBeTruthy();
    expect(screen.queryByText(guidance.pressure)).toBeNull();
    expect(screen.queryByText(updateGuidance, { exact: false })).toBeNull();
  });

  it('should say where new seats would stop, and mark a window without enough history', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: true,
          report: report({
            pressure: 'pass',
            host_pressure: { cpu: { '6h': 21, '24h': null }, io: { '6h': 4, '24h': null } },
            host_pressure_limit: LIMIT
          })
        }}
      />
    );
    expect(
      screen.getByText(
        '6-hour average: CPU 2.1%, disk 0.4%. New seats stop above CPU 15% or disk 10%.'
      )
    ).toBeTruthy();
  });

  it('should explain pressure that cannot be read', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: false,
          report: report({
            pressure: 'host_pressure_unavailable',
            host_pressure: null,
            host_pressure_limit: LIMIT
          })
        }}
      />
    );
    expect(screen.getByText(outcomes.host_pressure_unavailable)).toBeTruthy();
    expect(screen.getByText(guidance.pressure)).toBeTruthy();
    expect(screen.queryByText(pressureGuidance, { exact: false })).toBeNull();
    expect(screen.queryByText(/6-hour average/)).toBeNull();
  });

  it('should show the pressure without a limit, and nothing where it was not read', () => {
    const { unmount } = render(
      <SeatReadinessCard readiness={{ ready_for_new_seats: true, report: report() }} />
    );
    expect(screen.getByText('No limit set.')).toBeTruthy();
    expect(screen.getByText('6-hour average: CPU 1.2%, disk not enough history yet.')).toBeTruthy();
    unmount();

    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: true, report: report({ host_pressure: null }) }}
      />
    );
    expect(screen.queryByText(/6-hour average/)).toBeNull();
  });

  it('should leave out the pressure check for a daemon that predates it', () => {
    const { pressure: _, host_pressure: __, host_pressure_limit: ___, ...older } = report();
    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: true, report: older as ReadinessReport }}
      />
    );

    expect(screen.queryByText(labels.pressure)).toBeNull();
    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
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
            uptime_comparison: {
              scores: { '1d': 784, '7d': 832, '30d': 914 },
              minimums: MINIMUMS,
              wait_secs: { '1d': 0, '7d': 311_400, '30d': 518_400 }
            }
          })
        }}
      />
    );

    const meters = screen.getAllByRole('meter');
    expect(meters.map((m) => m.getAttribute('aria-valuenow'))).toEqual(['78.4', '83.2', '91.4']);
    // The waits come from the daemon; the card only words them.
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
            uptime_comparison: {
              scores: { '1d': 700, '7d': 900, '30d': 930 },
              minimums: MINIMUMS,
              wait_secs: { '1d': 0, '7d': 0, '30d': 0 }
            }
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
    const { uptime_comparison: _, ...older } = report();
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
