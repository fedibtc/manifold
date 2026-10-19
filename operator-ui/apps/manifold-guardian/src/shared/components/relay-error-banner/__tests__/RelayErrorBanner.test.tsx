import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { RelayErrorBanner } from '../RelayErrorBanner';

describe('RelayErrorBanner', () => {
  it('should say a relay error is a connection problem, with the reported error', () => {
    render(<RelayErrorBanner error="relay timed out" />);

    expect(screen.getByText(/Approval could not be checked/i)).toBeTruthy();
    expect(screen.getByText(/connection problem, not a refusal: relay timed out/i)).toBeTruthy();
  });

  it('should render the action it is given', () => {
    render(
      <RelayErrorBanner error="relay timed out" action={<button type="button">Retry</button>} />
    );

    expect(screen.getByRole('button', { name: 'Retry' })).toBeTruthy();
  });
});
