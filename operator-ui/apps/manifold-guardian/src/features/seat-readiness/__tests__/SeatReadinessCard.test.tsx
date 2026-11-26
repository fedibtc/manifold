import type { ReadinessReport } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import {
  describeOutcome,
  guidance,
  labels,
  orderingLatencyGuidance,
  outcomes,
  pressureGuidance,
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
  pressure: 'pass',
  host_pressure: null,
  max_host_pressure: null,
  ordering_latency: 'not_applicable',
  ordering_latency_6h_ms: null,
  max_ordering_latency_6h_ms: null,
  uptime: 'pass',
  uptime_comparison: null,
  ...overrides
});

const MAXIMUMS = {
  cpu: { '6h': 150, '3.5d': 100 },
  io: { '6h': 250, '3.5d': 150 },
  memory: { '6h': 700, '3.5d': 700 }
};

const MINIMUMS = { '1d': 700, '7d': 900, '30d': 930 };

describe('SeatReadinessCard', () => {
  it('should count a check that does not apply as passing', () => {
    render(<SeatReadinessCard readiness={{ ready_for_new_seats: true, report: report() }} />);

    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
    expect(screen.getByText('Not checked in this deployment.')).toBeTruthy();
    // No published minimum is said as such, not as a check that is off.
    expect(screen.getByText('No minimum set.')).toBeTruthy();
    expect(screen.getByText(describeOutcome('ordering_latency', 'not_applicable'))).toBeTruthy();
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

  it('should show each pressure window against its maximum and explain a failing one', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: false,
          report: report({
            pressure: 'host_pressure_3_5d_high',
            host_pressure: {
              cpu: { '6h': 128, '3.5d': 117 },
              io: { '6h': 21, '3.5d': 18 },
              memory: { '6h': 742, '3.5d': 512 }
            },
            max_host_pressure: MAXIMUMS
          })
        }}
      />
    );

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText(pressureGuidance, { exact: false })).toBeTruthy();
    const meters = screen.getAllByRole('meter');
    expect(meters.map((m) => m.getAttribute('aria-label'))).toEqual([
      'CPU, 6-hour average',
      'CPU, 3.5-day average',
      'Disk, 6-hour average',
      'Disk, 3.5-day average',
      'Memory, 6-hour average',
      'Memory, 3.5-day average'
    ]);
    expect(meters.map((m) => m.getAttribute('aria-valuenow'))).toEqual([
      '12.8',
      '11.7',
      '2.1',
      '1.8',
      '74.2',
      '51.2'
    ]);
    expect(screen.getAllByText('/ 70.0% maximum')).toHaveLength(2);
    expect(screen.getAllByText('/ 15.0% maximum')).toHaveLength(2);
    expect(screen.getByText('/ 10.0% maximum')).toBeTruthy();
    expect(screen.getByText('/ 25.0% maximum')).toBeTruthy();
    expect(screen.getByText(/once every average is within its maximum/)).toBeTruthy();
    // The bars replace the one-line check, and its read-failure guidance.
    expect(screen.queryByText(outcomes.host_pressure_3_5d_high)).toBeNull();
    expect(screen.queryByText(guidance.pressure)).toBeNull();
    expect(screen.queryByText(updateGuidance, { exact: false })).toBeNull();
  });

  it('should show passing pressure windows without a not-ready banner', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: true,
          report: report({
            host_pressure: {
              cpu: { '6h': 41, '3.5d': 52 },
              io: { '6h': 14, '3.5d': 11 },
              memory: { '6h': 63, '3.5d': 47 }
            },
            max_host_pressure: MAXIMUMS
          })
        }}
      />
    );
    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
    expect(screen.getAllByRole('meter')).toHaveLength(6);
    expect(screen.getByText('Every average is within its maximum.')).toBeTruthy();
    expect(screen.queryByText(pressureGuidance, { exact: false })).toBeNull();
  });

  it('should explain pressure that cannot be read', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: false,
          report: report({
            pressure: 'host_pressure_unavailable',
            host_pressure: null,
            max_host_pressure: MAXIMUMS
          })
        }}
      />
    );
    expect(screen.getByText(outcomes.host_pressure_unavailable)).toBeTruthy();
    expect(screen.getByText(guidance.pressure)).toBeTruthy();
    expect(screen.queryByText(pressureGuidance, { exact: false })).toBeNull();
    expect(screen.queryAllByRole('meter')).toHaveLength(0);
  });

  it('should say a fresh host is still measuring, without read-failure guidance', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: false,
          report: report({
            pressure: 'host_pressure_measuring',
            host_pressure: null,
            max_host_pressure: MAXIMUMS
          })
        }}
      />
    );
    expect(screen.getByText(outcomes.host_pressure_measuring)).toBeTruthy();
    expect(screen.queryByText(guidance.pressure)).toBeNull();
    expect(screen.queryByText(pressureGuidance, { exact: false })).toBeNull();
  });

  it('should leave out the pressure check for a daemon that predates it', () => {
    const { pressure: _, host_pressure: __, max_host_pressure: ___, ...older } = report();
    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: true, report: older as ReadinessReport }}
      />
    );

    expect(screen.queryByText(labels.pressure)).toBeNull();
    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
  });

  it('should show the consensus speed against its maximum and explain a slow host', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: false,
          report: report({
            ordering_latency: 'ordering_latency_high',
            ordering_latency_6h_ms: 6200,
            max_ordering_latency_6h_ms: 5000
          })
        }}
      />
    );

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText(orderingLatencyGuidance, { exact: false })).toBeTruthy();
    expect(screen.queryByText(pressureGuidance, { exact: false })).toBeNull();
    const meter = screen.getByRole('meter');
    expect(meter.getAttribute('aria-label')).toBe('Ordering time, 6-hour average');
    expect(meter.getAttribute('aria-valuenow')).toBe('6.2');
    expect(screen.getByText('6.2 s')).toBeTruthy();
    expect(screen.getByText('/ 5.0 s maximum')).toBeTruthy();
    expect(screen.getByText(/once the average is within its maximum/)).toBeTruthy();
    // The bar replaces the one-line check.
    expect(screen.queryByText(outcomes.ordering_latency_high)).toBeNull();
  });

  it('should show a passing consensus speed without a not-ready banner', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: true,
          report: report({
            ordering_latency: 'pass',
            ordering_latency_6h_ms: 1400,
            max_ordering_latency_6h_ms: 5000
          })
        }}
      />
    );

    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
    expect(screen.getByText('1.4 s')).toBeTruthy();
    expect(screen.getByText('The average is within its maximum.')).toBeTruthy();
    expect(screen.queryByText(orderingLatencyGuidance, { exact: false })).toBeNull();
  });

  it('should say the consensus speed has no figure yet, without closing the host', () => {
    render(
      <SeatReadinessCard
        readiness={{
          ready_for_new_seats: true,
          report: report({
            ordering_latency: 'not_applicable',
            ordering_latency_6h_ms: null,
            max_ordering_latency_6h_ms: 5000
          })
        }}
      />
    );

    expect(screen.getByText('Readiness checks passed')).toBeTruthy();
    expect(screen.getByText(labels.ordering_latency)).toBeTruthy();
    expect(screen.getByText(describeOutcome('ordering_latency', 'not_applicable'))).toBeTruthy();
    expect(screen.queryByText(orderingLatencyGuidance, { exact: false })).toBeNull();
    expect(screen.queryAllByRole('meter')).toHaveLength(0);
  });

  it('should leave out the consensus speed check for a daemon that predates it', () => {
    const {
      ordering_latency: _,
      ordering_latency_6h_ms: __,
      max_ordering_latency_6h_ms: ___,
      ...older
    } = report();
    render(
      <SeatReadinessCard
        readiness={{ ready_for_new_seats: true, report: older as ReadinessReport }}
      />
    );

    expect(screen.queryByText(labels.ordering_latency)).toBeNull();
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
