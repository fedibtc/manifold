import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AccessDenied } from '../AccessDenied';

describe('AccessDenied', () => {
  it('should name the failure as a permission problem, not a connection one', () => {
    render(<AccessDenied onRetry={() => {}} />);

    expect(screen.getByRole('heading', { name: "This token can't access that" })).toBeTruthy();
    expect(screen.getByText('permission_denied')).toBeTruthy();
  });

  it('should tell the operator their token is valid so they do not re-authenticate', () => {
    render(<AccessDenied onRetry={() => {}} />);

    expect(screen.getByText(/Your operator token is valid/)).toBeTruthy();
  });

  it('should call onRetry when the operator retries', () => {
    const onRetry = vi.fn();
    render(<AccessDenied onRetry={onRetry} />);

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));

    expect(onRetry).toHaveBeenCalledOnce();
  });
});
