import type { WalletOperationSummary } from '@operator-ui/types';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { WalletOperationsTable } from '../WalletOperationsTable';

const operations: WalletOperationSummary[] = [
  {
    operation_id: 'wop-0003',
    operation_type: 'deposit',
    amount: 1_000_000,
    status: 'confirmed',
    federation_id: null,
    created_at: 1721476800,
    updated_at: 1721477100
  }
];

const frozen: WalletOperationSummary = {
  operation_id: 'wop-frozen',
  operation_type: 'withdrawal',
  amount: 250_000,
  status: 'manual_review_required',
  federation_id: null,
  created_at: 1721476800,
  updated_at: 1721477100
};

describe('WalletOperationsTable', () => {
  it('should render a row per wallet operation', () => {
    render(<WalletOperationsTable operations={operations} />);

    expect(screen.getByText('wop-0003')).toBeTruthy();
    expect(screen.getByText('deposit')).toBeTruthy();
    expect(screen.getByText('1,000,000 sats')).toBeTruthy();
    expect(screen.getByText('confirmed')).toBeTruthy();
  });

  // Only the frozen rows get a control. Everything else either advances by
  // itself or is already terminal, so offering a resolution would invite an
  // operator to act on something that needs no acting on.
  it('should offer a resolution only for an operation under manual review', () => {
    const onResolve = vi.fn();

    render(<WalletOperationsTable operations={[...operations, frozen]} onResolve={onResolve} />);

    expect(screen.getAllByRole('button', { name: 'Resolve' })).toHaveLength(1);

    fireEvent.click(screen.getByRole('button', { name: 'Resolve' }));
    expect(onResolve).toHaveBeenCalledWith('wop-frozen');
  });

  it('should render no resolution control when the screen offers no handler', () => {
    render(<WalletOperationsTable operations={[frozen]} />);

    expect(screen.queryByRole('button', { name: 'Resolve' })).toBeNull();
  });

  it('should render the empty note when there are no operations', () => {
    render(<WalletOperationsTable operations={[]} />);

    expect(screen.getByText('No wallet operations yet.')).toBeTruthy();
    expect(screen.queryByRole('table')).toBeNull();
  });
});
