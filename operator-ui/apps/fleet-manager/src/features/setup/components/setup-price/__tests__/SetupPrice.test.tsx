import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { SetupPrice } from '../SetupPrice';

const renderPrice = (onDone = vi.fn()) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SetupPrice onDone={onDone} />
    </QueryClientProvider>
  );
  return { onDone };
};

const priceField = () => screen.getByLabelText('Price per seat (sats)');
const finishButton = () => screen.getByRole('button', { name: 'Finish setup' });

afterEach(() => {
  vi.restoreAllMocks();
});

describe('SetupPrice', () => {
  it('should write the entered price as millisatoshis', async () => {
    const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ plans: [] });
    renderPrice();

    fireEvent.change(priceField(), { target: { value: '50000' } });
    fireEvent.click(finishButton());

    await waitFor(() =>
      expect(adminCallSpy).toHaveBeenCalledWith({
        ConfigureInitialOffer: { max_seats: 0, price_msats: 50_000_000 }
      })
    );
  });

  it('should finish without selling when the price is left blank', async () => {
    const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ plans: [] });
    renderPrice();

    fireEvent.click(finishButton());

    await waitFor(() =>
      expect(adminCallSpy).toHaveBeenCalledWith({
        ConfigureInitialOffer: { max_seats: 0, price_msats: null }
      })
    );
  });

  it('should complete setup once the price is stored', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ plans: [] });
    const { onDone } = renderPrice();

    fireEvent.click(finishButton());

    await waitFor(() => expect(onDone).toHaveBeenCalled());
  });

  it('should reject an invalid price without calling the daemon', async () => {
    const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ plans: [] });
    const { onDone } = renderPrice();

    fireEvent.change(priceField(), { target: { value: '-5' } });
    fireEvent.click(finishButton());

    await screen.findByText('A price cannot be negative.');
    expect(adminCallSpy).not.toHaveBeenCalledWith(
      expect.objectContaining({ ConfigureInitialOffer: expect.anything() })
    );
    expect(onDone).not.toHaveBeenCalled();
  });
});
