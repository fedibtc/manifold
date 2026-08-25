import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { EarningsDay as EarningsDayModel } from '@/features/overview/utils/deriveEarnings';
import { EarningsDay } from '../EarningsDay';

const bucket: EarningsDayModel = {
  day: '2026-08-04',
  totalMsat: 54_000_000,
  events: [
    {
      key: 'seat-sale:seat-01',
      kind: 'seat-sale',
      amountMsat: 50_000_000,
      detail: 'seat-01',
      atMs: 1_753_000_000_000
    },
    {
      key: 'guardian-fee:tx1',
      kind: 'guardian-fee',
      amountMsat: 4_000_000,
      detail: 'seat-01',
      atMs: 1_753_000_000_000
    }
  ]
};

describe('EarningsDay', () => {
  it('should show the date and the day total', () => {
    render(<EarningsDay bucket={bucket} />);

    expect(screen.getByText('2026-08-04')).toBeTruthy();
    expect(screen.getByText('54,000 sats')).toBeTruthy();
  });

  it('should render every event in the bucket', () => {
    render(<EarningsDay bucket={bucket} />);

    expect(screen.getByText('Seat sold')).toBeTruthy();
    expect(screen.getByText('Guardian fee')).toBeTruthy();
  });

  it('should label an undated bucket rather than showing an empty heading', () => {
    render(<EarningsDay bucket={{ ...bucket, day: null }} />);

    expect(screen.getByText('Date unavailable')).toBeTruthy();
  });
});
