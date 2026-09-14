import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { GuardianTerms } from '../GuardianTerms';

describe('GuardianTerms', () => {
  it('should name the terms and the parties to them', () => {
    render(<GuardianTerms />);

    expect(screen.getByText('Fedi-Verified Guardian Terms of Service')).toBeTruthy();
    expect(screen.getByText('Between you and Fedi, Inc. · PDF')).toBeTruthy();
  });

  it('should show the full address as the link text', () => {
    render(<GuardianTerms />);

    const link = screen.getByRole('link', {
      name: 'https://public.qgcut.org/Fedi-verified_Guardian_ToS.pdf (opens in a new tab)'
    });

    expect(link.getAttribute('href')).toBe(
      'https://public.qgcut.org/Fedi-verified_Guardian_ToS.pdf'
    );
  });

  it('should open the terms in a new tab without handing it this window', () => {
    render(<GuardianTerms />);

    const link = screen.getByRole('link');

    expect(link.getAttribute('target')).toBe('_blank');
    expect(link.getAttribute('rel')).toBe('noopener noreferrer');
  });
});
