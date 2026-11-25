import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { SeatAvailability } from '../SeatAvailability';

const renderCard = (props: Parameters<typeof SeatAvailability>[0]) =>
  render(
    <MemoryRouter>
      <SeatAvailability {...props} />
    </MemoryRouter>
  );

const rowNames = () =>
  ['Fedi-verified selection', 'Guardian codes'].sort((a, b) =>
    screen.getAllByText(a)[0].compareDocumentPosition(screen.getAllByText(b)[0]) &
    Node.DOCUMENT_POSITION_FOLLOWING
      ? -1
      : 1
  );

describe('SeatAvailability', () => {
  it('should lead with the priced, Fedi-verified path on a badged host', () => {
    renderCard({ priceMsat: 50_000_000, guardianCodes: true, authorized: true });

    expect(rowNames()).toEqual(['Fedi-verified selection', 'Guardian codes']);
    expect(screen.getByText('50,000 sats per seat')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Change price and seats' })).toBeTruthy();
  });

  // Without a badge there is no price: codes lead, and selection is off.
  it('should lead with guardian codes and show no price without a badge', () => {
    renderCard({ priceMsat: null, guardianCodes: true, authorized: false });

    expect(rowNames()).toEqual(['Guardian codes', 'Fedi-verified selection']);
    expect(screen.getByText('Off')).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'Change price and seats' })).toBeNull();
    expect(screen.getByRole('link', { name: 'Add a badge' }).getAttribute('href')).toBe(
      '/authorization'
    );
    expect(screen.getByText('Free setup')).toBeTruthy();
  });

  // Production, badge connected: the network offers no guardian codes, so the
  // card must not claim them just because the host has a badge.
  it('should not mention guardian codes where the network does not offer them', () => {
    renderCard({ priceMsat: 50_000_000, guardianCodes: false, authorized: true });

    expect(screen.getByText('Fedi-verified selection')).toBeTruthy();
    expect(screen.getByText('50,000 sats per seat')).toBeTruthy();
    expect(screen.queryByText('Guardian codes')).toBeNull();
    expect(screen.queryByText('Free setup')).toBeNull();
  });

  // Production, no badge yet: still the priced path, never the badgeless one.
  it('should keep the priced path for an unapproved host where codes are unavailable', () => {
    renderCard({ priceMsat: 50_000_000, guardianCodes: false, authorized: false });

    expect(screen.getByRole('link', { name: 'Change price and seats' })).toBeTruthy();
    expect(screen.queryByText('Off')).toBeNull();
    expect(screen.queryByText('Guardian codes')).toBeNull();
  });
});
