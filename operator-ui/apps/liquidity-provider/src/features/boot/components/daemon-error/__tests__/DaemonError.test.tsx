import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { DaemonError } from '../DaemonError';

describe('DaemonError', () => {
  it('should name the failure as a connection problem', () => {
    render(<DaemonError onRetry={() => {}} />);

    expect(screen.getByRole('heading', { name: "Can't reach the FLIP daemon" })).toBeTruthy();
  });

  it('should reassure the operator that funds and configuration are untouched', () => {
    render(<DaemonError onRetry={() => {}} />);

    expect(screen.getByText(/funds and\s+configuration are untouched/)).toBeTruthy();
  });

  it('should call onRetry when the operator retries', () => {
    const onRetry = vi.fn();
    render(<DaemonError onRetry={onRetry} />);

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(onRetry).toHaveBeenCalledOnce();
  });
});
