import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { EarningEvent } from '@/features/overview/utils/deriveEarnings';
import { EarningsRow } from '../EarningsRow';

const event = (overrides: Partial<EarningEvent> = {}): EarningEvent => ({
  key: 'seat-sale:seat-01',
  kind: 'seat-sale',
  amountMsat: 50_000_000,
  detail: 'seat-01',
  atMs: 1_753_000_000_000,
  ...overrides
});

describe('EarningsRow', () => {
  it('should label a seat sale and show its amount in sats', () => {
    render(<EarningsRow event={event()} />);

    expect(screen.getByText('Seat sold')).toBeTruthy();
    expect(screen.getByText('50,000 sats')).toBeTruthy();
  });

  it('should label a guardian-fee remittance', () => {
    render(<EarningsRow event={event({ kind: 'guardian-fee', amountMsat: 4_000_000 })} />);

    expect(screen.getByText('Guardian fee')).toBeTruthy();
    expect(screen.getByText('4,000 sats')).toBeTruthy();
  });

  it('should show which seat the money came from', () => {
    render(<EarningsRow event={event({ detail: 'seat-earning-02' })} />);

    expect(screen.getByText('seat-earning-02')).toBeTruthy();
  });
});
