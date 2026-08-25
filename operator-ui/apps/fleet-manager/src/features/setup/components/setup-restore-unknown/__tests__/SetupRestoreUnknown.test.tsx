import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SetupRestoreUnknown } from '../SetupRestoreUnknown';

const networkError = {
  errorClass: 'network' as const,
  message: "Can't reach the fleet manager. Try again once it's back online."
};

const props = {
  error: networkError,
  isChecking: false,
  identityConfirmed: false,
  onCheckStatus: vi.fn(),
  onContinue: vi.fn()
};

describe('SetupRestoreUnknown', () => {
  it('should say the result is unknown rather than failed', () => {
    render(<SetupRestoreUnknown {...props} />);

    expect(screen.getByText(/we do not know whether the recovery finished/i)).toBeTruthy();
  });

  it('should never offer another recovery attempt', () => {
    render(<SetupRestoreUnknown {...props} />);

    expect(screen.queryByRole('button', { name: /recover/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /try again/i })).toBeNull();
  });

  it('should ask the daemon for its status', () => {
    const onCheckStatus = vi.fn();
    render(<SetupRestoreUnknown {...props} onCheckStatus={onCheckStatus} />);

    fireEvent.click(screen.getByRole('button', { name: 'Check status' }));
    expect(onCheckStatus).toHaveBeenCalled();
  });

  it('should disable the check while one is in flight', () => {
    render(<SetupRestoreUnknown {...props} isChecking />);

    const button = screen.getByRole('button', { name: 'Check status' }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
  });

  it('should continue without counts once an identity is confirmed', () => {
    const onContinue = vi.fn();
    render(<SetupRestoreUnknown {...props} identityConfirmed onContinue={onContinue} />);

    expect(screen.getByText(/recovery counts are not available/i)).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
    expect(onContinue).toHaveBeenCalled();
  });
});
