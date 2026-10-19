import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PeerBadgePhoneMockup } from '../PeerBadgePhoneMockup';

describe('PeerBadgePhoneMockup', () => {
  it('should show the given PeerBadge screen', () => {
    render(<PeerBadgePhoneMockup src="screen.webp" alt="PeerBadge badge details" />);

    expect(screen.getByRole('img', { name: 'PeerBadge badge details' }).getAttribute('src')).toBe(
      'screen.webp'
    );
  });

  it('should draw pointers over the phone', () => {
    render(
      <PeerBadgePhoneMockup src="screen.webp" alt="PeerBadge badge details">
        <span>Tap here</span>
      </PeerBadgePhoneMockup>
    );

    expect(screen.getByText('Tap here')).toBeTruthy();
  });
});
