import type { ReadinessReport } from '@operator-ui/types';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { guidance } from '@/shared/utils/seatReadiness';
import { SeatReadinessCard } from '../SeatReadinessCard';

const report = (overrides: Partial<ReadinessReport> = {}): ReadinessReport => ({
  checked_at_ms: Date.UTC(2026, 9, 1, 12, 34, 56),
  relay: 'pass',
  discovery: 'not_applicable',
  bitcoin: 'pass',
  ...overrides
});

describe('SeatReadinessCard', () => {
  it('should count a check that does not apply as passing', () => {
    render(<SeatReadinessCard report={report()} />);

    expect(screen.getByText('Accepting new seats')).toBeTruthy();
    expect(screen.getByText('Not checked in this deployment.')).toBeTruthy();
    expect(screen.getByText(/Last checked 2026-10-01 12:34 UTC/)).toBeTruthy();
  });

  it('should explain only the failing check', () => {
    render(<SeatReadinessCard report={report({ bitcoin: 'bitcoin_syncing' })} />);

    expect(screen.getByText('Not accepting new seats')).toBeTruthy();
    expect(screen.getByText('Bitcoin Core is still in initial block download.')).toBeTruthy();
    expect(screen.getByText(guidance.bitcoin)).toBeTruthy();
    expect(screen.queryByText(guidance.relay)).toBeNull();
  });

  it('should not claim readiness before the first run', () => {
    render(<SeatReadinessCard report={null} />);

    expect(screen.getByText('Checking readiness')).toBeTruthy();
    expect(screen.queryByText('Accepting new seats')).toBeNull();
  });
});
