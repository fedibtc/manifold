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
    renderCard({ priceMsat: 50_000_000, badgeless: false });

    expect(rowNames()).toEqual(['Fedi-verified selection', 'Guardian codes']);
    expect(screen.getByText('50,000 sats per seat')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Change price and seats' })).toBeTruthy();
  });

  // Without a badge there is no price: codes lead, and selection is off.
  it('should lead with guardian codes and show no price without a badge', () => {
    renderCard({ priceMsat: null, badgeless: true });

    expect(rowNames()).toEqual(['Guardian codes', 'Fedi-verified selection']);
    expect(screen.getByText('Off')).toBeTruthy();
    expect(screen.queryByRole('link', { name: 'Change price and seats' })).toBeNull();
    expect(screen.getByRole('link', { name: 'Add a badge' }).getAttribute('href')).toBe(
      '/authorization'
    );
    expect(screen.getByText('Free setup')).toBeTruthy();
  });
});
