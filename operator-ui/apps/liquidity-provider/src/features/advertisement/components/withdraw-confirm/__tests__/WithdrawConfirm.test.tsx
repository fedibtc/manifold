import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { WithdrawConfirm } from '../WithdrawConfirm';

const renderPanel = (isPending = false) => {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  render(<WithdrawConfirm onConfirm={onConfirm} onCancel={onCancel} isPending={isPending} />);
  return { onConfirm, onCancel };
};

describe('WithdrawConfirm', () => {
  it('should call onConfirm with null when no reason is entered', () => {
    const { onConfirm } = renderPanel();

    fireEvent.click(screen.getByRole('button', { name: 'Confirm withdrawal' }));

    expect(onConfirm).toHaveBeenCalledWith(null);
  });

  it('should call onConfirm with the trimmed reason when one is entered', () => {
    const { onConfirm } = renderPanel();

    fireEvent.change(screen.getByLabelText('Reason (optional)'), {
      target: { value: '  liquidity rebalance  ' }
    });
    fireEvent.click(screen.getByRole('button', { name: 'Confirm withdrawal' }));

    expect(onConfirm).toHaveBeenCalledWith('liquidity rebalance');
  });

  it('should call onCancel and not onConfirm when Cancel is clicked', () => {
    const { onConfirm, onCancel } = renderPanel();

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(onCancel).toHaveBeenCalled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it('should show the confirm button as loading while pending', () => {
    renderPanel(true);

    expect(
      screen.getByRole('button', { name: 'Confirm withdrawal' }).hasAttribute('disabled')
    ).toBe(true);
  });
});
