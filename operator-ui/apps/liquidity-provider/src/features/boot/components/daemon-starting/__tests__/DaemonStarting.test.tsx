import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { DaemonStarting } from '../DaemonStarting';

describe('DaemonStarting', () => {
  // The screen this replaces says the daemon "isn't answering" over the line
  // "GET /health · connection refused". That route answered — it is what told
  // the dashboard the daemon is restoring.
  it('should name a live restore rather than call the daemon unreachable', () => {
    render(<DaemonStarting reason="reloading" onRetry={() => {}} />);

    expect(screen.getByText('Restoring from a backup')).toBeTruthy();
    expect(screen.getByText('GET /health · mode: reloading')).toBeTruthy();
    expect(screen.queryByText(/unreachable|connection refused/i)).toBeNull();
  });

  it('should name a starting daemon apart from a restoring one', () => {
    render(<DaemonStarting reason="no-runtime" onRetry={() => {}} />);

    expect(screen.getByText('The FLIP daemon is starting')).toBeTruthy();
    expect(screen.getByText('GET /health · mode: no_runtime')).toBeTruthy();
  });

  it('should force a check when the operator asks for one', () => {
    const onRetry = vi.fn();

    render(<DaemonStarting reason="reloading" onRetry={onRetry} />);
    fireEvent.click(screen.getByRole('button', { name: 'Check now' }));

    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});
