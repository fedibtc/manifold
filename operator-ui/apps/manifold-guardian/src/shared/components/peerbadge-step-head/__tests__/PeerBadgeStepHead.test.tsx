import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PeerBadgeStepHead } from '../PeerBadgeStepHead';

describe('PeerBadgeStepHead', () => {
  it('should show the step number, title and instruction', () => {
    render(
      <PeerBadgeStepHead number={2} title="Authorize application">
        Tap Authorize application in PeerBadge.
      </PeerBadgeStepHead>
    );

    expect(screen.getByText('2')).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Authorize application' })).toBeTruthy();
    expect(screen.getByText('Tap Authorize application in PeerBadge.')).toBeTruthy();
  });
});
