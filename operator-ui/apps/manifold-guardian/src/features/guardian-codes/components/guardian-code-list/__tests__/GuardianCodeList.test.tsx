import type { SeatGrantEntry } from '@operator-ui/types';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it, vi } from 'vitest';
import { GuardianCodeList } from '../GuardianCodeList';

const grant = (
  grant_nonce: string,
  label: string | null,
  status: SeatGrantEntry['status']
): SeatGrantEntry => ({ grant_nonce, label, issued_at: 1_786_000_000, status });

const grants = [
  grant('unused-1', 'Lisbon meetup', { state: 'unused', uri: 'manifold:seat-grant?v=1&g=1' }),
  grant('used-1', 'Cedar', { state: 'used', seat_id: 'seat-7' }),
  grant('revoked-1', 'Old', { state: 'revoked' }),
  grant('unused-2', null, { state: 'unused', uri: 'manifold:seat-grant?v=1&g=2' })
];

const renderList = () => {
  const onShow = vi.fn();
  const onRevoke = vi.fn();
  render(
    <MemoryRouter>
      <GuardianCodeList grants={grants} onShow={onShow} onRevoke={onRevoke} />
    </MemoryRouter>
  );
  return { onShow, onRevoke };
};

const row = (label: string) => screen.getByText(label).closest('tr') as HTMLElement;

describe('GuardianCodeList', () => {
  it('should offer Revoke on unused codes only', () => {
    renderList();

    expect(screen.getAllByRole('button', { name: 'Revoke' })).toHaveLength(2);
    expect(within(row('Lisbon meetup')).getByRole('button', { name: 'Revoke' })).toBeTruthy();
    expect(within(row('Cedar')).queryByRole('button', { name: 'Revoke' })).toBeNull();
    expect(within(row('Old')).queryByRole('button', { name: 'Revoke' })).toBeNull();
  });

  it('should hand the chosen code to revoke and show', () => {
    const { onShow, onRevoke } = renderList();

    fireEvent.click(within(row('Lisbon meetup')).getByRole('button', { name: 'Revoke' }));
    fireEvent.click(within(row('Lisbon meetup')).getByRole('button', { name: 'Show code' }));

    expect(onRevoke).toHaveBeenCalledWith(grants[0]);
    expect(onShow).toHaveBeenCalledWith(grants[0]);
  });

  it('should link a used code to the seat it created', () => {
    renderList();

    const link = within(row('Cedar')).getByRole('link', { name: 'View seat' });
    expect(link.getAttribute('href')).toBe('/seats/seat-7');
  });

  it('should name each status', () => {
    renderList();

    expect(within(row('Lisbon meetup')).getByText('Unused')).toBeTruthy();
    expect(within(row('Cedar')).getByText('Used')).toBeTruthy();
    expect(within(row('Old')).getByText('Revoked')).toBeTruthy();
  });

  it('should name an unlabelled code', () => {
    renderList();

    expect(within(row('No label')).getByRole('button', { name: 'Show code' })).toBeTruthy();
  });
});
