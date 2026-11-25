import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { WithdrawCell } from '../WithdrawCell';

describe('WithdrawCell', () => {
  it('should offer no button for an empty balance', () => {
    render(<WithdrawCell isEmpty balanceName="seat sales" onWithdraw={vi.fn()} />);

    expect(screen.getByText('Nothing to withdraw')).toBeInTheDocument();
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('should name the balance its button withdraws for a screen reader', () => {
    const onWithdraw = vi.fn();
    render(
      <WithdrawCell
        isEmpty={false}
        balanceName="seat sales from fed1aaaa"
        onWithdraw={onWithdraw}
      />
    );

    fireEvent.click(screen.getByRole('button', { name: 'Withdraw seat sales from fed1aaaa' }));
    expect(onWithdraw).toHaveBeenCalledOnce();
  });
});
