import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { StaleDataBanner } from '../StaleDataBanner';

describe('StaleDataBanner', () => {
  it('should name the data as last-known rather than current', () => {
    render(<StaleDataBanner />);

    expect(screen.getByText('Showing last-known data')).toBeInTheDocument();
  });

  it('should state that the connection is being retried when no stamp is known', () => {
    render(<StaleDataBanner />);

    expect(screen.getByText('Retrying the connection.')).toBeInTheDocument();
  });

  it('should stamp the last successful load when one is known', () => {
    const updatedAtMs = 1721476800000;

    render(<StaleDataBanner updatedAtMs={updatedAtMs} />);

    const stamp = new Date(updatedAtMs).toLocaleTimeString();
    expect(
      screen.getByText(`Retrying the connection — last updated ${stamp}.`)
    ).toBeInTheDocument();
  });
});
