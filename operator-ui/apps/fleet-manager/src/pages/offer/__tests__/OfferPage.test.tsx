import type { AdminRequest } from '@operator-ui/types';
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

const STORED_MAX_SEATS = 4;
const STORED_FREE_SLOTS = 2;

const verbOf = (request: AdminRequest): string =>
  typeof request === 'string' ? request : Object.keys(request)[0];

interface StoredOffer {
  priceMsat?: number | null;
  maxSeats?: number;
  availableSlots?: number;
  /** Answers for the writes, keyed by verb. A rejection here is the daemon
   *  refusing, which is how the below-active-seats guard reaches the screen. */
  writes?: Record<string, () => unknown>;
}

/**
 * The page makes two reads, so a single blanket mock would answer `ShowCapacity`
 * with a plan list. Every test dispatches on the verb instead.
 */
const mockDaemon = ({
  priceMsat = null,
  maxSeats = STORED_MAX_SEATS,
  availableSlots = STORED_FREE_SLOTS,
  writes = {}
}: StoredOffer = {}) =>
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request) => {
    const verb = verbOf(request);
    const write = writes[verb];
    if (write) return write() as never;
    if (verb === 'ShowCapacity') {
      return { max_seats: maxSeats, available_slots: availableSlots } as never;
    }
    const plans = priceMsat === null ? [] : [{ InfiniteBestEffort: { price_msats: priceMsat } }];
    return { plans } as never;
  });

const priceField = () => screen.getByLabelText('Price per seat (sats)') as HTMLInputElement;
const seatsField = () => screen.getByLabelText('Maximum active seats') as HTMLInputElement;

// The form renders only once the stored offer has answered, so waiting on the
// field's value alone would pass instantly against a field that is not there.
const readyPriceField = async () => screen.findByLabelText('Price per seat (sats)');
const readySeatsField = async () => screen.findByLabelText('Maximum active seats');

const save = () => fireEvent.click(screen.getByRole('button', { name: 'Save' }));

afterEach(() => {
  vi.restoreAllMocks();
});

it('should seed the price field from the stored offer', async () => {
  mockDaemon({ priceMsat: 50_000_000 });
  renderPage();

  await waitFor(() => expect(priceField().value).toBe('50000'));
});

it('should seed the seats field from the stored ceiling', async () => {
  mockDaemon({ priceMsat: 50_000_000, maxSeats: 6 });
  renderPage();

  await waitFor(() => expect(seatsField().value).toBe('6'));
});

// The reason this page exists: the stored ceiling was not readable anywhere
// after setup.
it('should show the stored ceiling and its free slots', async () => {
  mockDaemon({ maxSeats: 4, availableSlots: 2 });
  renderPage();

  await screen.findByText('Currently 4, with 2 free.');
});

it('should write the entered price as millisatoshis', async () => {
  const adminCallSpy = mockDaemon({ priceMsat: null });
  renderPage();

  fireEvent.change(await readyPriceField(), { target: { value: '25000' } });
  save();

  await waitFor(() =>
    expect(adminCallSpy).toHaveBeenCalledWith({ SetPrice: { price_msats: 25_000_000 } })
  );
});

it('should write a null price when the field is cleared', async () => {
  const adminCallSpy = mockDaemon({ priceMsat: 50_000_000 });
  renderPage();

  await waitFor(() => expect(priceField().value).toBe('50000'));
  fireEvent.change(priceField(), { target: { value: '' } });
  save();

  await waitFor(() =>
    expect(adminCallSpy).toHaveBeenCalledWith({ SetPrice: { price_msats: null } })
  );
});

it('should write a raised ceiling', async () => {
  const adminCallSpy = mockDaemon({ priceMsat: 50_000_000, maxSeats: 4 });
  renderPage();

  fireEvent.change(await readySeatsField(), { target: { value: '8' } });
  save();

  await waitFor(() => expect(adminCallSpy).toHaveBeenCalledWith({ SetCapacity: { max_seats: 8 } }));
});

// Writing the ceiling that is already stored would rotate the offer epoch and
// invalidate quotes for nothing.
it('should not write the ceiling when only the price changed', async () => {
  const adminCallSpy = mockDaemon({ priceMsat: 50_000_000, maxSeats: 4 });
  renderPage();

  fireEvent.change(await readyPriceField(), { target: { value: '25000' } });
  save();

  await waitFor(() =>
    expect(adminCallSpy).toHaveBeenCalledWith({ SetPrice: { price_msats: 25_000_000 } })
  );
  expect(adminCallSpy).not.toHaveBeenCalledWith({ SetCapacity: { max_seats: 4 } });
});

it('should reject a fractional price without calling the daemon', async () => {
  const adminCallSpy = mockDaemon();
  renderPage();

  const field = await readyPriceField();
  adminCallSpy.mockClear();
  fireEvent.change(field, { target: { value: '12.5' } });
  save();

  await screen.findByText('Sats cannot be fractional.');
  expect(adminCallSpy).not.toHaveBeenCalled();
});

it('should reject a fractional ceiling without calling the daemon', async () => {
  const adminCallSpy = mockDaemon();
  renderPage();

  const field = await readySeatsField();
  adminCallSpy.mockClear();
  fireEvent.change(field, { target: { value: '2.5' } });
  save();

  await screen.findByText('Seats cannot be fractional.');
  expect(adminCallSpy).not.toHaveBeenCalled();
});

// The guard the operator will actually hit, in the daemon's own words. The
// active seat count is not on the wire, so the screen does not re-derive it.
it('should show the daemon refusal when the ceiling is below the active seats', async () => {
  mockDaemon({
    priceMsat: 50_000_000,
    maxSeats: 4,
    writes: {
      SetCapacity: () => {
        throw new Error('cannot set max seats to 2; 4 seats are active');
      }
    }
  });
  renderPage();

  fireEvent.change(await readySeatsField(), { target: { value: '2' } });
  save();

  await screen.findByText('cannot set max seats to 2; 4 seats are active');
});

it('should not write the price when the ceiling was refused', async () => {
  const adminCallSpy = mockDaemon({
    priceMsat: 50_000_000,
    maxSeats: 4,
    writes: {
      SetCapacity: () => {
        throw new Error('cannot set max seats to 2; 4 seats are active');
      }
    }
  });
  renderPage();

  fireEvent.change(await readySeatsField(), { target: { value: '2' } });
  fireEvent.change(priceField(), { target: { value: '25000' } });
  save();

  await screen.findByText('cannot set max seats to 2; 4 seats are active');
  expect(adminCallSpy).not.toHaveBeenCalledWith({ SetPrice: { price_msats: 25_000_000 } });
});

it('should explain that a blank price stops selling and zero is free', async () => {
  mockDaemon();
  renderPage();

  await screen.findByText(/Leave the price blank to stop selling seats/);
  screen.getByText(/gives seats away free/);
});

// Both writes rotate the offer epoch, and that is what a quote's validity is
// checked against — so the operator is told before they save, not after.
it('should warn that saving a change invalidates unpaid quotes', async () => {
  mockDaemon();
  renderPage();

  await screen.findByText(/stops being valid/);
});

it('should offer a retry instead of a form when the offer has never loaded', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockRejectedValue(new Error('boom'));
  renderPage();

  await screen.findByText('boom');
  screen.getByRole('button', { name: 'Try again' });
  expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
});

// The ceiling is part of what this form claims, so a screen that cannot read it
// must not offer to overwrite it either.
it('should offer a retry instead of a form when the ceiling has never loaded', async () => {
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request) => {
    if (verbOf(request) === 'ShowCapacity') throw new Error('capacity unreadable');
    return { plans: [] } as never;
  });
  renderPage();

  await screen.findByText('capacity unreadable');
  expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
});

// The fault: a failed background poll took the Save control away from an
// operator who had the stored price in front of them and was about to change it.
it('should keep the form usable under a staleness marker when a refresh fails', async () => {
  let answered = false;
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request) => {
    if (answered) throw new Error('daemon blip');
    if (verbOf(request) === 'ShowCapacity') {
      return { max_seats: STORED_MAX_SEATS, available_slots: STORED_FREE_SLOTS } as never;
    }
    return { plans: [{ InfiniteBestEffort: { price_msats: 50_000_000 } }] } as never;
  });
  const client = renderPage();

  await waitFor(() => expect(priceField().value).toBe('50000'));
  answered = true;
  await act(async () => {
    await client.refetchQueries();
  });

  await screen.findByText('Showing last-known data');
  expect(priceField().value).toBe('50000');
  expect(seatsField().value).toBe('4');
  expect(screen.getByRole('button', { name: 'Save' })).not.toBeDisabled();
});
