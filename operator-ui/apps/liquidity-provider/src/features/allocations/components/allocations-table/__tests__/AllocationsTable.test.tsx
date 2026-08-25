import type { AdminAllocationSummary } from '@operator-ui/types';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AllocationsTable } from '../AllocationsTable';

const summary: AdminAllocationSummary = {
  federation_id: 'ft-1',
  gateway_status: 'completed',
  stability_pool_status: null,
  committed_amount: 1_234_567,
  created_at: 1,
  updated_at: 2
};

const noop = () => {};

describe('AllocationsTable', () => {
  it('should render an empty state when the daemon reported no allocations', () => {
    render(<AllocationsTable rows={[]} selectedId={null} onSelect={noop} />);

    expect(screen.getByText('No allocations yet.')).toBeTruthy();
  });

  it('should render a row per allocation', () => {
    render(<AllocationsTable rows={[summary]} selectedId={null} onSelect={noop} />);

    expect(screen.getByRole('button', { name: 'ft-1' })).toBeTruthy();
    expect(screen.getByText('1,234,567')).toBeTruthy();
    expect(screen.getByText('Completed')).toBeTruthy();
  });

  it('should call onSelect with the allocation id when a row is chosen', () => {
    const onSelect = vi.fn();
    render(<AllocationsTable rows={[summary]} selectedId={null} onSelect={onSelect} />);

    fireEvent.click(screen.getByRole('button', { name: 'ft-1' }));

    expect(onSelect).toHaveBeenCalledWith('ft-1');
  });
});
