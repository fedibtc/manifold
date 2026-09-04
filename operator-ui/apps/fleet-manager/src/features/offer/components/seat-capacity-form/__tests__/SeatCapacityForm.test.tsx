import type { AdminRequest } from '@operator-ui/types';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import * as adminCallModule from '@/shared/api/adminCall';
import { CAPACITY_KEY } from '@/shared/api/hooks/use-capacity/useCapacity';
import { parseSeatCapacity, SeatCapacityForm } from '../SeatCapacityForm';

let storedMaxSeats = 4;
let writeError: Error | null = null;
/** Seats the daemon reports. The floor counts the ones not decommissioned, the
 *  same way `Db::set_max_seats` does. Empty by default so a test opts in to the
 *  floor rather than inheriting it. */
let storedSeats: { decommissioned: boolean }[] = [];
/** A seat list that never answers, to prove the field still works without it. */
let seatsUnavailable = false;

const mockDaemon = () =>
  vi.spyOn(adminCallModule, 'adminCall').mockImplementation(async (request: AdminRequest) => {
    if (request === 'ShowCapacity') {
      return { max_seats: storedMaxSeats, available_slots: storedMaxSeats } as never;
    }
    if (request === 'ListSeats') {
      if (seatsUnavailable) throw new Error('seat list unavailable');
      return { seats: storedSeats, backup_scan: null } as never;
    }
    if (typeof request !== 'string' && 'SetCapacity' in request) {
      if (writeError) throw writeError;
      storedMaxSeats = request.SetCapacity.max_seats;
      return {} as never;
    }
    throw new Error('unexpected admin request');
  });

const activeSeats = (count: number) =>
  Array.from({ length: count }, () => ({ decommissioned: false }));

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
  storedSeats = [];
  seatsUnavailable = false;
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

// maan2003 on PR #39: the operator should meet the floor in the form, not as a
// daemon error after pressing Save.
it('should accept a limit at the active seat count', () => {
  expect(parseSeatCapacity('3', 3)).toEqual({ ok: true, maxSeats: 3 });
});

it('should reject a limit below the active seat count', () => {
  expect(parseSeatCapacity('2', 3)).toEqual({
    ok: false,
    error: 'You have 3 active seats. Decommission a seat before lowering the limit below that.'
  });
});

it('should say seat in the singular for a single active seat', () => {
  expect(parseSeatCapacity('0', 1)).toEqual({
    ok: false,
    error: 'You have 1 active seat. Decommission a seat before lowering the limit below that.'
  });
});

// Without the seat list there is no floor to check, and the daemon still holds it.
it('should skip the floor when the active seat count is unknown', () => {
  expect(parseSeatCapacity('0', null)).toEqual({ ok: true, maxSeats: 0 });
});

it('should tell the operator the floor before they hit it', async () => {
  storedSeats = activeSeats(3);
  mockDaemon();
  renderForm();

  await screen.findByText('3 seats are active. The limit cannot go below that.');
});

// `min` makes the field itself range-invalid, so the browser stops the submit
// before the handler runs. The operator gets the rule from the hint and the
// stepper floor rather than from a round trip to the daemon.
it('should refuse a below-floor limit without calling the daemon', async () => {
  storedSeats = activeSeats(3);
  const adminCall = mockDaemon();
  renderForm();
  const input = (await screen.findByLabelText('Maximum active seats')) as HTMLInputElement;
  await screen.findByText('3 seats are active. The limit cannot go below that.');

  expect(input).toHaveAttribute('min', '3');
  fireEvent.change(input, { target: { value: '2' } });
  expect(input.validity.rangeUnderflow).toBe(true);

  fireEvent.click(screen.getByRole('button', { name: 'Save seat limit' }));

  await waitFor(() => expect(adminCall).toHaveBeenCalledWith('ListSeats'));
  expect(adminCall).not.toHaveBeenCalledWith({ SetCapacity: { max_seats: 2 } });
});

// The floor the browser cannot hold: no `min` is set until the seat list has
// answered, and the count can move under a form the operator left open.
it('should keep the floor in the validator for values the field lets through', () => {
  expect(parseSeatCapacity('2', 3).ok).toBe(false);
});

it('should count only seats that are not decommissioned', async () => {
  storedSeats = [...activeSeats(2), { decommissioned: true }];
  mockDaemon();
  renderForm();

  await screen.findByText('2 seats are active. The limit cannot go below that.');
});

// The seat list only sharpens the error, so losing it must not cost the
// operator the field.
it('should still allow a save when the seat list cannot be read', async () => {
  seatsUnavailable = true;
  const adminCall = mockDaemon();
  renderForm();
  const input = await screen.findByLabelText('Maximum active seats');

  fireEvent.change(input, { target: { value: '6' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save seat limit' }));

  await waitFor(() => expect(adminCall).toHaveBeenCalledWith({ SetCapacity: { max_seats: 6 } }));
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
