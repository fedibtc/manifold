import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { GuardianFeeRow } from '@/features/payouts/hooks/use-guardian-fee-rows/useGuardianFeeRows';
import { GuardianFeeTable } from '../GuardianFeeTable';

const rows: GuardianFeeRow[] = [
  {
    seatId: 'seat-earning-01',
    collectableMsat: 16_000_000,
    collectedEcashMsat: 8_000_000,
    sweepableMsat: 7_960_000,
    earning: true
  }
];

const emptyRow: GuardianFeeRow = {
  seatId: 'seat-earning-01',
  collectableMsat: 0,
  collectedEcashMsat: 0,
  sweepableMsat: 0,
  earning: true
};

const renderTable = (guardianFeeRows: GuardianFeeRow[]) => {
  const onWithdraw = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <GuardianFeeTable rows={guardianFeeRows} onWithdraw={onWithdraw} />
    </QueryClientProvider>
  );
  return onWithdraw;
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

  // A real seat id is a 64-character hex string. Printed whole it forced the
  // row far wider than the two amount columns it exists to label.
  it('should middle-truncate a full-length seat id and offer it whole to the clipboard', () => {
    const seatId = 'a'.repeat(32) + 'b'.repeat(32);
    renderTable([{ ...emptyRow, seatId }]);

    expect(screen.getByText('aaaaaaaa…bbbbbbbb')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy seat ID' })).toBeInTheDocument();
  });

  it('should not offer a copy control for a seat id it shows in full', () => {
    renderTable(rows);

    expect(screen.queryByRole('button', { name: 'Copy seat ID' })).toBeNull();
  });

  it('should render an unread fee account as unknown rather than as zero', () => {
    renderTable([{ ...emptyRow, collectableMsat: null, collectedEcashMsat: null, earning: null }]);

    expect(screen.getAllByText('—')).toHaveLength(2);
  });

  // Collecting happens inside the withdraw dialog, so one button serves a seat.
  it('should offer one Withdraw per seat, opening that seat', () => {
    const onWithdraw = renderTable(rows);

    expect(screen.queryByRole('button', { name: /Collect/ })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /^Withdraw/ }));
    expect(onWithdraw).toHaveBeenCalledWith('seat-earning-01');
  });

  // Fees still in the pool can be collected from the dialog, so the seat is
  // not empty until the pool is empty too.
  it('should offer Withdraw for fees only in the pool, and nothing for an empty seat', () => {
    renderTable([
      { ...emptyRow, collectableMsat: 5_000_000 },
      { ...emptyRow, seatId: 'seat-empty-01' }
    ]);

    expect(screen.getAllByRole('button', { name: /^Withdraw/ })).toHaveLength(1);
    expect(screen.getByText('Nothing to withdraw')).toBeInTheDocument();
  });

  it('should explain an empty table instead of showing bare headers', () => {
    renderTable([]);

    expect(
      screen.getByText(
        'No guardian fees yet. They appear once a federation you guard is running and its members start sending payments.'
      )
    ).toBeInTheDocument();
    expect(screen.queryByText('In the fee pool')).toBeNull();
  });

  it('should answer why the figures are zero when every pool is empty', () => {
    renderTable([emptyRow]);

    expect(screen.getByText('Why is this 0?')).toBeInTheDocument();
  });

  it('should not answer that question when a pool holds something', () => {
    renderTable(rows);

    expect(screen.queryByText('Why is this 0?')).toBeNull();
  });

  it('should not answer that question for an unread fee account', () => {
    renderTable([{ ...emptyRow, collectableMsat: null, collectedEcashMsat: null, earning: null }]);

    expect(screen.queryByText('Why is this 0?')).toBeNull();
  });

  it('should not answer that question when collected fees are ready to send', () => {
    renderTable([{ ...emptyRow, collectedEcashMsat: 8_000_000 }]);

    expect(screen.queryByText('Why is this 0?')).toBeNull();
  });

  it('should not answer that question when a seat has stopped earning fees', () => {
    renderTable([emptyRow, { ...emptyRow, seatId: 'seat-stopped-01', earning: false }]);

    expect(screen.queryByText('Why is this 0?')).toBeNull();
  });

  it('should mark a seat whose federation has stopped charging guardian fees', () => {
    renderTable([emptyRow, { ...emptyRow, seatId: 'seat-stopped-01', earning: false }]);

    expect(screen.getAllByText('Fees stopped')).toHaveLength(1);
  });

  it('should not mark a seat whose fee policy could not be read', () => {
    renderTable([{ ...emptyRow, earning: null }]);

    expect(screen.queryByText('Fees stopped')).toBeNull();
  });

  it('should not answer that question when there are no seats at all', () => {
    renderTable([]);

    expect(screen.queryByText('Why is this 0?')).toBeNull();
  });
});
