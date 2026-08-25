import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { PayoutDestinationCard } from '../PayoutDestinationCard';

const renderCard = (destination: string | null) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PayoutDestinationCard destination={destination} />
    </QueryClientProvider>
  );
};

const field = () => screen.getByLabelText('Lightning address or LNURL-pay');
const saveButton = () => screen.getByRole('button', { name: 'Save destination' });

afterEach(() => {
  vi.restoreAllMocks();
});

describe('PayoutDestinationCard', () => {
  it('should store the entered destination', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ destination: 'operator@example.com' });
    renderCard(null);

    fireEvent.change(field(), { target: { value: 'operator@example.com' } });
    fireEvent.click(saveButton());

    await waitFor(() =>
      expect(adminCall).toHaveBeenCalledWith({
        SetPayoutDestination: { destination: 'operator@example.com' }
      })
    );
  });

  it('should clear the stored destination', async () => {
    const adminCall = vi
      .spyOn(adminCallModule, 'adminCall')
      .mockResolvedValue({ destination: null });
    renderCard('operator@example.com');

    fireEvent.click(screen.getByRole('button', { name: 'Clear' }));

    await waitFor(() =>
      expect(adminCall).toHaveBeenCalledWith({ SetPayoutDestination: { destination: null } })
    );
  });

  it('should seed the field with the stored destination', () => {
    renderCard('operator@example.com');

    expect(field()).toHaveValue('operator@example.com');
  });

  // The daemon refuses an empty destination; clearing is its own control, so a
  // blank field is never sent as a write.
  it('should refuse to save a blank destination', () => {
    renderCard(null);

    expect(saveButton()).toBeDisabled();
  });

  it('should offer no clear control when there is nothing stored', () => {
    renderCard(null);

    expect(screen.queryByRole('button', { name: 'Clear' })).toBeNull();
  });

  // The ordering the daemon enforces has to be readable off the screen rather
  // than discovered through a refusal.
  it('should warn that sweeps refuse while no destination is stored', () => {
    renderCard(null);

    expect(screen.getByText('No payout destination')).toBeInTheDocument();
    expect(screen.getByText(/Sweeps are refused until one is set/)).toBeInTheDocument();
  });

  it('should name the destination revenue leaves to', () => {
    renderCard('operator@example.com');

    expect(screen.getByText('operator@example.com')).toBeInTheDocument();
    expect(screen.queryByText('No payout destination')).toBeNull();
  });

  it('should report a refused write instead of showing it as stored', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(
      new Error('payout destination is too long')
    );
    renderCard(null);

    fireEvent.change(field(), { target: { value: 'operator@example.com' } });
    fireEvent.click(saveButton());

    await waitFor(() =>
      expect(screen.getByText('payout destination is too long')).toBeInTheDocument()
    );
  });
});
