import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ApprovalWaiting } from '../ApprovalWaiting';

describe('ApprovalWaiting', () => {
  it('should state the host is waiting and when it was last checked', () => {
    render(<ApprovalWaiting lastChecked="2026-08-06 07:06 UTC" />);

    expect(screen.getByRole('heading', { name: 'Waiting for approval' })).toBeTruthy();
    expect(screen.getByText(/Last checked 2026-08-06 07:06 UTC\./)).toBeTruthy();
  });

  it('should not promise an automatic confirmation', () => {
    render(<ApprovalWaiting lastChecked="2026-08-06 07:06 UTC" />);

    expect(screen.queryByText(/automatically/i)).toBeNull();
  });

  it('should show the surface re-check control beside the status', () => {
    render(
      <ApprovalWaiting
        lastChecked="2026-08-06 07:06 UTC"
        action={<button type="button">Check status</button>}
      />
    );

    expect(screen.getByRole('button', { name: 'Check status' })).toBeTruthy();
  });
});
