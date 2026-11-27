import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { PayoutDialog } from '../PayoutDialog';

const SEAT = 'seat-0123456789abcdef0123456789';

describe('PayoutDialog', () => {
  it('should name itself by its title and show the balance it draws on', () => {
    render(
      <PayoutDialog
        title="Withdraw guardian fees"
        source={{ kind: 'guardian_fee', seatId: SEAT }}
        note="Lightning can take a few minutes."
        footer={<button type="button">Done</button>}
      >
        <p>Body</p>
      </PayoutDialog>
    );

    const dialog = screen.getByRole('dialog', { name: 'Withdraw guardian fees' });
    expect(within(dialog).getByRole('button', { name: 'Copy seat ID' })).toBeInTheDocument();
    expect(within(dialog).getByText('Lightning can take a few minutes.')).toBeInTheDocument();
    expect(dialog).toHaveFocus();
  });

  it('should close on Escape when the step may be dismissed', () => {
    const onDismiss = vi.fn();
    render(
      <PayoutDialog title="Withdraw seat sales" onDismiss={onDismiss} footer={null}>
        <p>Review</p>
      </PayoutDialog>
    );

    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });

    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
