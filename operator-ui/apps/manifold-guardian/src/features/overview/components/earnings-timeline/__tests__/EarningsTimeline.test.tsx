import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { EarningsDay } from '@/features/overview/utils/deriveEarnings';
import { EarningsTimeline } from '../EarningsTimeline';

const day = (isoDay: string | null, totalMsat: number): EarningsDay => ({
  day: isoDay,
  totalMsat,
  events: [
    {
      key: `seat-sale:${isoDay}`,
      kind: 'seat-sale',
      amountMsat: totalMsat,
      detail: 'seat-01',
      atMs: 1_753_000_000_000
    }
  ]
});

describe('EarningsTimeline', () => {
  it('should invite the operator to earn when nothing has landed yet', () => {
    render(<EarningsTimeline days={[]} />);

    expect(screen.getByText(/Nothing earned yet/i)).toBeTruthy();
  });

  it('should render one bucket per day', () => {
    render(
      <EarningsTimeline days={[day('2026-08-04', 4_000_000), day('2026-08-03', 50_000_000)]} />
    );

    expect(screen.getByText('2026-08-04')).toBeTruthy();
    expect(screen.getByText('2026-08-03')).toBeTruthy();
  });

  it('should render an undated bucket alongside dated ones', () => {
    render(<EarningsTimeline days={[day('2026-08-04', 4_000_000), day(null, 7_000)]} />);

    expect(screen.getByText('Date unavailable')).toBeTruthy();
  });
});
