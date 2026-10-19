import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { AuthorizeApplicationCallout } from '../AuthorizeApplicationCallout';

describe('AuthorizeApplicationCallout', () => {
  it('should show the real Authorize application row from PeerBadge', () => {
    render(<AuthorizeApplicationCallout />);

    expect(screen.getByRole('img', { name: /Authorize application/ })).toBeTruthy();
  });
});
