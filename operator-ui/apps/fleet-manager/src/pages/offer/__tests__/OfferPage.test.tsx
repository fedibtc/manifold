import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { OfferPage } from '../OfferPage';

const renderPage = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <OfferPage />
      </MemoryRouter>
    </QueryClientProvider>
  );
  return client;
};

const priceField = () => screen.getByLabelText('Price per seat (sats)') as HTMLInputElement;

// The form renders only once the stored offer has answered, so waiting on the
// field's value alone would pass instantly against a field that is not there.
const readyPriceField = async () => screen.findByLabelText('Price per seat (sats)');

afterEach(() => {
  vi.restoreAllMocks();
});

it('should seed the price field from the stored offer', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    plans: [{ InfiniteBestEffort: { price_msats: 50_000_000 } }]
  });
  renderPage();

  await waitFor(() => expect(priceField().value).toBe('50000'));
});

it('should write the entered price as millisatoshis', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ plans: [] });
  renderPage();

  fireEvent.change(await readyPriceField(), { target: { value: '25000' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));

  await waitFor(() =>
    expect(adminCallSpy).toHaveBeenCalledWith({ SetPrice: { price_msats: 25_000_000 } })
  );
});

it('should write a null price when the field is cleared', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({
    plans: [{ InfiniteBestEffort: { price_msats: 50_000_000 } }]
  });
  renderPage();

  await waitFor(() => expect(priceField().value).toBe('50000'));
  fireEvent.change(priceField(), { target: { value: '' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));

  await waitFor(() =>
    expect(adminCallSpy).toHaveBeenCalledWith({ SetPrice: { price_msats: null } })
  );
});

it('should reject a fractional price without calling the daemon', async () => {
  const adminCallSpy = vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ plans: [] });
  renderPage();

  const field = await readyPriceField();
  adminCallSpy.mockClear();
  fireEvent.change(field, { target: { value: '12.5' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));

  await screen.findByText('Sats cannot be fractional.');
  expect(adminCallSpy).not.toHaveBeenCalled();
});

it('should explain that blank stops selling and zero is free', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockResolvedValue({ plans: [] });
  renderPage();

  await screen.findByText(/Leave the field blank to stop selling seats/);
  screen.getByText(/gives seats away free/);
});

it('should offer a retry instead of a form when the offer has never loaded', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new Error('boom'));
  renderPage();

  await screen.findByText('boom');
  screen.getByRole('button', { name: 'Try again' });
  expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
});

// The fault: a failed background poll took the Save control away from an
// operator who had the stored price in front of them and was about to change it.
it('should keep the form usable under a staleness marker when a refresh fails', async () => {
  vi.spyOn(adminCallModule, 'adminCall')
    .mockResolvedValueOnce({ plans: [{ InfiniteBestEffort: { price_msats: 50_000_000 } }] })
    .mockRejectedValue(new Error('daemon blip'));
  const client = renderPage();

  await waitFor(() => expect(priceField().value).toBe('50000'));
  await act(async () => {
    await client.refetchQueries();
  });

  await screen.findByText('Showing last-known data');
  expect(priceField().value).toBe('50000');
  expect(screen.getByRole('button', { name: 'Save' })).not.toBeDisabled();
});
