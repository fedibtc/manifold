import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { GuardianFeeRow } from '@/features/payouts/hooks/use-guardian-fee-rows/useGuardianFeeRows';
import { GuardianFeeTable } from '../GuardianFeeTable';

const rows: GuardianFeeRow[] = [
  { seatId: 'seat-earning-01', collectableMsat: 16_000_000, collectedEcashMsat: 8_000_000 }
];

const renderTable = (guardianFeeRows: GuardianFeeRow[], hasDestination = true) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GuardianFeeTable rows={guardianFeeRows} hasDestination={hasDestination} />
    </QueryClientProvider>
  );
};

describe('GuardianFeeTable', () => {
  it('should separate what is still in the pool from what is ready to send', () => {
    renderTable(rows);

    expect(screen.getByText('16,000 sats')).toBeInTheDocument();
    expect(screen.getByText('8,000 sats')).toBeInTheDocument();
  });

  it('should name the seat each fee account belongs to', () => {
    renderTable(rows);

    expect(screen.getByText('seat-earning-01')).toBeInTheDocument();
  });

  it('should render an unread fee account as unknown rather than as zero', () => {
    renderTable([{ seatId: 'seat-earning-01', collectableMsat: null, collectedEcashMsat: null }]);

    expect(screen.getAllByText('—')).toHaveLength(2);
  });

  // The two steps are the point: a collection moves money out of the pool, and
  // only a second action sends it. One button would hide that.
  it('should offer both steps per seat', () => {
    renderTable(rows);

    expect(screen.getByRole('button', { name: '1. Collect out of the pool' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '2. Send to destination' })).toBeInTheDocument();
  });
});
