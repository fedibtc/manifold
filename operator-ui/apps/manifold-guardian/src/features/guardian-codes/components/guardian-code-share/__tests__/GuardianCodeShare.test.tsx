import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { GuardianCodeShare, type ShareableGrant } from '../GuardianCodeShare';

const URI = 'manifold:seat-grant?v=1&env=development&node=n&key=k&grant=g&sig=s';

const grant: ShareableGrant = {
  grant_nonce: 'g',
  label: 'Lisbon meetup',
  issued_at: 1_786_000_000,
  status: { state: 'unused', uri: URI }
};

describe('GuardianCodeShare', () => {
  it('should show the code as text with its one-use, free terms', () => {
    render(<GuardianCodeShare grant={grant} isNew onDone={vi.fn()} />);

    expect(screen.getByText(URI)).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Copy guardian code' })).toBeTruthy();
    expect(screen.getByText('One federation setup')).toBeTruthy();
    expect(screen.getByText('Free')).toBeTruthy();
    expect(screen.getByText(/Viewing or copying this code does not use it/)).toBeTruthy();
  });

  it('should announce a new code but not one shown again from the list', () => {
    const { rerender } = render(<GuardianCodeShare grant={grant} isNew onDone={vi.fn()} />);
    expect(screen.queryByText('Guardian code ready')).toBeTruthy();

    rerender(<GuardianCodeShare grant={grant} isNew={false} onDone={vi.fn()} />);
    expect(screen.queryByText('Guardian code ready')).toBeNull();
  });
});
