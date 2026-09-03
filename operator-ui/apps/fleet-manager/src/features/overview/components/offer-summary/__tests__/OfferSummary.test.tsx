import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { OfferSummary } from '../OfferSummary';

const renderSummary = (priceMsat: number | null) =>
  render(
    <MemoryRouter>
      <OfferSummary priceMsat={priceMsat} />
    </MemoryRouter>
  );

describe('OfferSummary', () => {
  it('should show the price per seat in sats', () => {
    renderSummary(50_000_000);

    expect(screen.getByText('50,000 sats per seat')).toBeTruthy();
  });

  it('should describe a zero price as free rather than as not selling', () => {
    renderSummary(0);

    expect(screen.getByText('Free')).toBeTruthy();
  });

  it('should say the fleet is not selling when no price is stored', () => {
    renderSummary(null);

    expect(screen.getByText('Not selling seats')).toBeTruthy();
  });

  // The link names the whole offer, not just the price: the page behind it also
  // sets the seat ceiling, and it is the only route to either.
  it('should link to the offer page', () => {
    renderSummary(50_000_000);

    expect(screen.getByRole('link', { name: 'Change offer' }).getAttribute('href')).toBe('/offer');
  });
});
