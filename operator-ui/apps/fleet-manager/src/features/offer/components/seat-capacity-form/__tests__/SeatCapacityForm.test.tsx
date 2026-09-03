import type { AdminRequest } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { CAPACITY_KEY } from '@/shared/api/hooks/use-capacity/useCapacity';
import { parseSeatCapacity, SeatCapacityForm } from '../SeatCapacityForm';

let storedMaxSeats = 4;
let writeError: Error | null = null;

const mockDaemon = () =>
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request: AdminRequest) => {
    if (request === 'ShowCapacity') {
      return { max_seats: storedMaxSeats, available_slots: storedMaxSeats } as never;
    }
    if (typeof request !== 'string' && 'SetCapacity' in request) {
      if (writeError) throw writeError;
      storedMaxSeats = request.SetCapacity.max_seats;
      return {} as never;
    }
    throw new Error('unexpected admin request');
  });

const renderForm = () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <SeatCapacityForm />
    </QueryClientProvider>
  );
  return client;
};

afterEach(() => {
  vi.restoreAllMocks();
  storedMaxSeats = 4;
  writeError = null;
});

it.each(['', '-1', '2.5', '4294967296'])('should reject invalid capacity %j', (value) => {
  expect(parseSeatCapacity(value)).toEqual({
    ok: false,
    error: 'Enter a whole number from 0 to 4294967295.'
  });
});

it('should preserve an edited limit through refresh and save it', async () => {
  const adminCall = mockDaemon();
  const client = renderForm();
  const input = await screen.findByLabelText('Maximum active seats');

  expect(input).toHaveValue(4);
  fireEvent.change(input, { target: { value: '6' } });

  storedMaxSeats = 8;
  await act(() => client.invalidateQueries({ queryKey: CAPACITY_KEY }));
  expect(input).toHaveValue(6);

  fireEvent.click(screen.getByRole('button', { name: 'Save seat limit' }));
  await waitFor(() => expect(adminCall).toHaveBeenCalledWith({ SetCapacity: { max_seats: 6 } }));
  expect(screen.getByRole('button', { name: 'Save seat limit' })).toBeDisabled();
});

it('should show the daemon refusal', async () => {
  writeError = new Error('cannot set max seats to 2; 4 seats are active');
  mockDaemon();
  renderForm();
  const input = await screen.findByLabelText('Maximum active seats');

  fireEvent.change(input, { target: { value: '2' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save seat limit' }));

  await screen.findByText('cannot set max seats to 2; 4 seats are active');
});
