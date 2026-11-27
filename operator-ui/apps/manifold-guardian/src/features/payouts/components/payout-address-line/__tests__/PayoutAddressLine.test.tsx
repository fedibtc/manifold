import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { PayoutAddressLine } from '../PayoutAddressLine';

describe('PayoutAddressLine', () => {
  it('should say ecash works without an address when none is saved', () => {
    const onChange = vi.fn();
    render(<PayoutAddressLine destination={null} onChange={onChange} />);

    expect(screen.getByText(/Ecash works without it/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Add payout address' }));
    expect(onChange).toHaveBeenCalledOnce();
  });

  it('should show the saved address with a Change action', () => {
    const onChange = vi.fn();
    render(<PayoutAddressLine destination="operator@example.com" onChange={onChange} />);

    expect(screen.getByText('operator@example.com')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Change' }));
    expect(onChange).toHaveBeenCalledOnce();
  });
});
