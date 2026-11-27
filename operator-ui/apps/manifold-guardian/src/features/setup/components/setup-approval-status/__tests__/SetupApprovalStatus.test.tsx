import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SetupApprovalStatus } from '../SetupApprovalStatus';

const CHECKED_AT = 1_760_000_000;

describe('SetupApprovalStatus', () => {
  it('should show the waiting status after a read that found nothing', () => {
    render(<SetupApprovalStatus nostr={{ state: 'not_observed', checked_at: CHECKED_AT }} />);

    expect(screen.getByRole('heading', { name: 'Waiting for approval' })).toBeTruthy();
  });

  it('should report a failed read as a failed read', () => {
    render(<SetupApprovalStatus nostr={{ state: 'relay_error', error: 'connection refused' }} />);

    expect(screen.getByText(/Approval could not be checked/i)).toBeTruthy();
    expect(screen.getByText(/connection refused/i)).toBeTruthy();
    expect(screen.queryByText(/Waiting for approval/i)).toBeNull();
  });

  // The daemon starts every run in `checking` and reads the relays only when
  // asked, so this state must offer the check or setup has no way forward.
  it('should say approval is not checked yet before the first relay read', () => {
    render(<SetupApprovalStatus nostr={{ state: 'checking' }} />);

    expect(screen.getByText(/Not checked yet/i)).toBeTruthy();
    expect(screen.queryByText(/Waiting for approval/i)).toBeNull();
  });
});
