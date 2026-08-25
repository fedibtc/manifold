import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SetupRestoreFailed } from '../SetupRestoreFailed';

const daemonError = {
  errorClass: 'daemon' as const,
  message: 'seat directory already exists: /var/lib/fman/seats/abc — remove it and retry'
};

describe('SetupRestoreFailed', () => {
  it('should show the daemon message word for word', () => {
    render(<SetupRestoreFailed error={daemonError} onTryAgain={vi.fn()} onBackToDoors={vi.fn()} />);

    expect(screen.getByText(daemonError.message)).toBeTruthy();
  });

  it('should offer a retry that returns to the form', () => {
    const onTryAgain = vi.fn();
    render(
      <SetupRestoreFailed error={daemonError} onTryAgain={onTryAgain} onBackToDoors={vi.fn()} />
    );

    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(onTryAgain).toHaveBeenCalled();
  });

  it('should offer a way back to the setup options', () => {
    const onBackToDoors = vi.fn();
    render(
      <SetupRestoreFailed error={daemonError} onTryAgain={vi.fn()} onBackToDoors={onBackToDoors} />
    );

    fireEvent.click(screen.getByRole('button', { name: 'Back to setup options' }));
    expect(onBackToDoors).toHaveBeenCalled();
  });

  it('should state that nothing was installed', () => {
    render(<SetupRestoreFailed error={daemonError} onTryAgain={vi.fn()} onBackToDoors={vi.fn()} />);

    expect(screen.getByText(/host still has no identity/i)).toBeTruthy();
  });
});
