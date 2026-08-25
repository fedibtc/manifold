import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ListingCard } from '../ListingCard';

const renderCard = () =>
  render(
    <ListingCard
      provider="npub1qy3…kzsx"
      lastPublished="2h ago"
      expires="in 20h"
      sources="Gateway (Lightning) · Stability pool"
      endpoint="iroh: b3f9…c2ae"
    />
  );

describe('ListingCard', () => {
  it('should render the Listing section heading', () => {
    renderCard();
    expect(screen.getByRole('heading', { name: 'Listing' })).toBeTruthy();
  });

  it('should render every listing field with its value', () => {
    renderCard();
    expect(screen.getByText('Provider')).toBeTruthy();
    expect(screen.getByText('npub1qy3…kzsx')).toBeTruthy();
    expect(screen.getByText('2h ago')).toBeTruthy();
    expect(screen.getByText('in 20h')).toBeTruthy();
    expect(screen.getByText('Gateway (Lightning) · Stability pool')).toBeTruthy();
    expect(screen.getByText('iroh: b3f9…c2ae')).toBeTruthy();
  });

  it('should note that the public listing excludes balances', () => {
    renderCard();
    expect(screen.getByText(/never includes your balances/)).toBeTruthy();
  });
});
