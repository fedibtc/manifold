import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SetupTerms } from '../SetupTerms';

describe('SetupTerms', () => {
  it('should show the guardian terms with their address', () => {
    render(<SetupTerms onAccepted={vi.fn()} />);

    expect(screen.getByRole('heading', { name: 'Accept the terms of service' })).toBeTruthy();
    expect(
      screen.getByRole('link', { name: /public\.qgcut\.org\/Fedi-verified_Guardian_ToS\.pdf/ })
    ).toBeTruthy();
  });

  it('should state the arbitration and waiver terms before the operator accepts', () => {
    render(<SetupTerms onAccepted={vi.fn()} />);

    expect(
      screen.getByText(
        'The terms include mandatory individual arbitration, and waive class actions and jury trials.'
      )
    ).toBeTruthy();
  });

  it('should describe the accept button with what selecting it agrees to', () => {
    render(<SetupTerms onAccepted={vi.fn()} />);

    const button = screen.getByRole('button', { name: 'Accept and continue' });
    const consent = document.getElementById(button.getAttribute('aria-describedby') ?? '');

    expect(consent?.textContent).toBe(
      'By selecting Accept and continue, you agree to the Fedi-Verified Guardian Terms of Service.'
    );
  });

  it('should continue once when the operator accepts', () => {
    const onAccepted = vi.fn();
    render(<SetupTerms onAccepted={onAccepted} />);

    fireEvent.click(screen.getByRole('button', { name: 'Accept and continue' }));

    expect(onAccepted).toHaveBeenCalledTimes(1);
  });
});
