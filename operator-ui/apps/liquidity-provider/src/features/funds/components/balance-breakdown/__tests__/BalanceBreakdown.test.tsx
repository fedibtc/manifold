import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { BalanceRow } from '@/features/funds/utils/deriveFunds';
import { BalanceBreakdown } from '../BalanceBreakdown';

const rows: BalanceRow[] = [
  { key: 'spendable', label: 'Spendable', value: 4_200_000 },
  { key: 'available_balance', label: 'Available', value: 3_250_000, strong: true }
];

describe('BalanceBreakdown', () => {
  it('should render the section title', () => {
    render(<BalanceBreakdown rows={rows} />);

    expect(screen.getByRole('heading', { name: 'Balance breakdown' })).toBeTruthy();
  });

  it('should render a labelled, formatted row per balance entry', () => {
    render(<BalanceBreakdown rows={rows} />);

    expect(screen.getByText('Spendable')).toBeTruthy();
    expect(screen.getByText('4,200,000 sats')).toBeTruthy();
    expect(screen.getByText('Available')).toBeTruthy();
    expect(screen.getByText('3,250,000 sats')).toBeTruthy();
  });

  it('should render the manual top-up note', () => {
    render(<BalanceBreakdown rows={rows} />);

    expect(
      screen.getByText('Top-ups are manual — FLIP never moves funds in on its own.')
    ).toBeTruthy();
  });
});
