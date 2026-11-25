import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { DialogResult } from '../DialogResult';

describe('DialogResult', () => {
  it('should announce the outcome with what it means', () => {
    render(
      <DialogResult tone="warning" heading="The payment did not go through">
        <p>Nothing left this host.</p>
      </DialogResult>
    );

    const status = screen.getByRole('status');
    expect(status).toHaveTextContent('!The payment did not go throughNothing left this host.');
  });

  it('should mark a success with a tick rather than a warning', () => {
    render(<DialogResult tone="success" heading="Sent 1,000 sats" />);

    expect(screen.getByRole('status')).toHaveTextContent(/^✓Sent 1,000 sats$/);
  });
});
