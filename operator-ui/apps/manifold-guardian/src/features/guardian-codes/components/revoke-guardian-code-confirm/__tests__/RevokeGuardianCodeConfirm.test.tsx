import type { SeatGrantEntry } from '@operator-ui/types';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { RevokeGuardianCodeConfirm } from '../RevokeGuardianCodeConfirm';

const grant: SeatGrantEntry = {
  grant_nonce: 'g',
  label: 'Lisbon meetup',
  issued_at: 1_786_000_000,
  status: { state: 'unused', uri: 'manifold:seat-grant?v=1' }
};

const renderConfirm = (overrides: Partial<SeatGrantEntry> = {}) => {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  render(
    <RevokeGuardianCodeConfirm
      grant={{ ...grant, ...overrides }}
      onConfirm={onConfirm}
      onCancel={onCancel}
      isPending={false}
      error={null}
    />
  );
  return { onConfirm, onCancel };
};

describe('RevokeGuardianCodeConfirm', () => {
  it('should name the code it revokes', () => {
    renderConfirm();

    expect(screen.getByRole('heading', { name: 'Revoke “Lisbon meetup”?' })).toBeTruthy();
  });

  it('should fall back to a generic name for an unlabelled code', () => {
    renderConfirm({ label: null });

    expect(screen.getByRole('heading', { name: 'Revoke this guardian code?' })).toBeTruthy();
  });

  // The safe choice takes focus, so Enter does not revoke by accident.
  it('should focus Keep code and cancel on Escape', () => {
    const { onCancel, onConfirm } = renderConfirm();

    expect(document.activeElement?.textContent).toBe('Keep code');
    fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });

    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it('should revoke only on the explicit action', () => {
    const { onConfirm } = renderConfirm();

    fireEvent.click(screen.getByRole('button', { name: 'Revoke code' }));

    expect(onConfirm).toHaveBeenCalledTimes(1);
  });
});
