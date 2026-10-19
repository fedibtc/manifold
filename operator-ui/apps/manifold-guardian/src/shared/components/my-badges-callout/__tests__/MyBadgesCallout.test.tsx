import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { MyBadgesCallout } from '../MyBadgesCallout';

describe('MyBadgesCallout', () => {
  it('should show the real My badges row from PeerBadge', () => {
    render(<MyBadgesCallout />);

    expect(screen.getByRole('img', { name: /My badges/ })).toBeTruthy();
  });
});
