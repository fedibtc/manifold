import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { AdvertisementHeader } from '../AdvertisementHeader';

describe('AdvertisementHeader', () => {
  it('should render the title and subtitle', () => {
    render(<AdvertisementHeader />);

    expect(screen.getByRole('heading', { name: 'Advertisement' })).toBeTruthy();
    expect(
      screen.getByText('What federations see when they look for liquidity on Nostr.')
    ).toBeTruthy();
  });

  it('should render a status slot when provided', () => {
    render(<AdvertisementHeader status={<span>Published</span>} />);

    expect(screen.getByText('Published')).toBeTruthy();
  });
});
