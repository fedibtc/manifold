import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { SetupApprovalStatus } from '../SetupApprovalStatus';

const CHECKED_AT = 1_760_000_000;
const action = <button type="button">Check status</button>;

describe('SetupApprovalStatus', () => {
  it('should show the re-check control inside the waiting status', () => {
    render(
      <SetupApprovalStatus
        nostr={{ state: 'not_observed', checked_at: CHECKED_AT }}
        action={action}
      />
    );

    const status = screen.getByRole('heading', { name: 'Waiting for approval' }).closest('section');
    expect(status?.textContent).toContain('Check status');
  });

  it('should report a failed read as a failed read, with the re-check beside it', () => {
    render(
      <SetupApprovalStatus
        nostr={{ state: 'relay_error', error: 'connection refused' }}
        action={action}
      />
    );

    expect(screen.getByText(/Approval could not be checked/i)).toBeTruthy();
    expect(screen.getByText(/connection refused/i)).toBeTruthy();
    expect(screen.queryByText(/Waiting for approval/i)).toBeNull();
    expect(screen.getByRole('button', { name: 'Check status' })).toBeTruthy();
  });

  it('should say nothing is known while the first relay read is outstanding', () => {
    render(<SetupApprovalStatus nostr={{ state: 'checking' }} action={action} />);

    expect(screen.getByText(/Checking whether this host has been approved/i)).toBeTruthy();
    expect(screen.queryByText(/Waiting for approval/i)).toBeNull();
  });
});
