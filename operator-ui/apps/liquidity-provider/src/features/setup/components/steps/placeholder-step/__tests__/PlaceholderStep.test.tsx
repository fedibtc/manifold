import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PlaceholderStep } from '@/features/setup/components/steps/placeholder-step/PlaceholderStep';

describe('PlaceholderStep', () => {
  it('should title the card with the step it stands in for', () => {
    render(<PlaceholderStep title="Chain observer" />);

    expect(screen.getByText('Chain observer')).toBeTruthy();
  });

  it('should say the step is not built yet rather than render an empty card', () => {
    render(<PlaceholderStep title="Chain observer" />);

    expect(screen.getByText('Coming in the next step.')).toBeTruthy();
  });
});
