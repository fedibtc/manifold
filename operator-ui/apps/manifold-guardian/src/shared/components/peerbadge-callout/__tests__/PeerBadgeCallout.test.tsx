import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PeerBadgeCallout } from '../PeerBadgeCallout';

describe('PeerBadgeCallout', () => {
  it('should show the given PeerBadge row with its name', () => {
    render(<PeerBadgeCallout src="row.webp" alt="My badges: a row" />);

    expect(screen.getByRole('img', { name: 'My badges: a row' }).getAttribute('src')).toBe(
      'row.webp'
    );
  });
});
