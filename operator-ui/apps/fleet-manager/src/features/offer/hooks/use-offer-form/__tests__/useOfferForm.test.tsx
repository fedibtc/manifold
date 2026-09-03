import { act, renderHook, waitFor } from '@testing-library/react';
import type { FormEvent, ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('@/shared/api/hooks/use-offer/useOffer', () => ({
  useOffer: vi.fn(),
  OFFER_KEY: ['offer']
}));
vi.mock('@/shared/api/hooks/use-capacity/useCapacity', () => ({
  useCapacity: vi.fn(),
  CAPACITY_KEY: ['capacity']
}));
vi.mock('@/shared/api/hooks/use-set-price/useSetPrice', () => ({ useSetPrice: vi.fn() }));
vi.mock('@/shared/api/hooks/use-set-capacity/useSetCapacity', () => ({ useSetCapacity: vi.fn() }));

import { useCapacity } from '@/shared/api/hooks/use-capacity/useCapacity';
import { useOffer } from '@/shared/api/hooks/use-offer/useOffer';
import { useSetCapacity } from '@/shared/api/hooks/use-set-capacity/useSetCapacity';
import { useSetPrice } from '@/shared/api/hooks/use-set-price/useSetPrice';
import { describeActionError } from '@/shared/utils/describeActionError';
import { useOfferForm } from '../useOfferForm';

const ANSWERED_AT_MS = 1_760_000_000_000;

const STORED_MAX_SEATS = 4;
const STORED_FREE_SLOTS = 2;

const refetch = vi.fn();
const refetchCapacity = vi.fn();

const plansFor = (priceMsat: number | null) =>
  priceMsat === null ? [] : [{ InfiniteBestEffort: { price_msats: priceMsat } }];

const mockOffer = (priceMsat: number | null): void => {
  vi.mocked(useOffer).mockReturnValue({
    data: { plans: plansFor(priceMsat) },
    isLoading: false,
    isError: false,
    error: null,
    dataUpdatedAt: ANSWERED_AT_MS,
    refetch
  } as unknown as ReturnType<typeof useOffer>);
};

const mockOfferError = (error: unknown): void => {
  vi.mocked(useOffer).mockReturnValue({
    data: undefined,
    isLoading: false,
    isError: true,
    error,
    dataUpdatedAt: 0,
    refetch
  } as unknown as ReturnType<typeof useOffer>);
};

// React-query keeps the answer through a failed refresh: the stored price is
// still known, and only the last attempt failed.
const mockOfferRefreshFailure = (priceMsat: number | null, error: unknown): void => {
  vi.mocked(useOffer).mockReturnValue({
    data: { plans: plansFor(priceMsat) },
    isLoading: false,
    isError: true,
    error,
    dataUpdatedAt: ANSWERED_AT_MS,
    refetch
  } as unknown as ReturnType<typeof useOffer>);
};

const mockCapacity = (maxSeats = STORED_MAX_SEATS, availableSlots = STORED_FREE_SLOTS): void => {
  vi.mocked(useCapacity).mockReturnValue({
    data: { max_seats: maxSeats, available_slots: availableSlots },
    isLoading: false,
    isError: false,
    error: null,
    dataUpdatedAt: ANSWERED_AT_MS,
    refetch: refetchCapacity
  } as unknown as ReturnType<typeof useCapacity>);
};

const mockCapacityError = (error: unknown): void => {
  vi.mocked(useCapacity).mockReturnValue({
    data: undefined,
    isLoading: false,
    isError: true,
    error,
    dataUpdatedAt: 0,
    refetch: refetchCapacity
  } as unknown as ReturnType<typeof useCapacity>);
};

const setPriceAsync = vi.fn();
const setCapacityAsync = vi.fn();

const mockSetPrice = (overrides: Record<string, unknown> = {}): void => {
  vi.mocked(useSetPrice).mockReturnValue({
    mutateAsync: setPriceAsync,
    isPending: false,
    isError: false,
    error: null,
    ...overrides
  } as unknown as ReturnType<typeof useSetPrice>);
};

const mockSetCapacity = (overrides: Record<string, unknown> = {}): void => {
  vi.mocked(useSetCapacity).mockReturnValue({
    mutateAsync: setCapacityAsync,
    isPending: false,
    isError: false,
    error: null,
    ...overrides
  } as unknown as ReturnType<typeof useSetCapacity>);
};

/** The stored offer answered, both writes idle. */
const mockStoredOffer = (priceMsat: number | null, maxSeats = STORED_MAX_SEATS): void => {
  mockOffer(priceMsat);
  mockCapacity(maxSeats);
  mockSetPrice();
  mockSetCapacity();
};

const wrapper = ({ children }: { children: ReactNode }) => <MemoryRouter>{children}</MemoryRouter>;

const submit = () => ({ preventDefault: vi.fn() }) as unknown as FormEvent<HTMLFormElement>;

describe('useOfferForm', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    setPriceAsync.mockReset();
    setCapacityAsync.mockReset();
    refetch.mockReset();
    refetchCapacity.mockReset();
  });

  it('should seed the field with the stored price in sats', () => {
    mockStoredOffer(50_000_000);

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.priceSats).toBe('50000');
  });

  it('should leave the field blank when the fleet is not selling', () => {
    mockStoredOffer(null);

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.priceSats).toBe('');
  });

  it('should seed the seats field with the stored ceiling', () => {
    mockStoredOffer(50_000_000, 6);

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.maxSeats).toBe('6');
  });

  // The value was unreadable anywhere after setup, which is the whole point of
  // this field: an operator has to see what they are changing.
  it('should describe the stored ceiling and its free slots', () => {
    mockOffer(50_000_000);
    mockCapacity(4, 2);
    mockSetPrice();
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.capacityHint).toBe('Currently 4, with 2 free.');
  });

  it('should submit a blank field as a null price', async () => {
    mockStoredOffer(50_000_000);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange(''));
    act(() => result.current.onSubmit(submit()));

    await waitFor(() => expect(setPriceAsync).toHaveBeenCalledWith(null));
  });

  it('should submit a zero price as a free seat', async () => {
    mockStoredOffer(null);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('0'));
    act(() => result.current.onSubmit(submit()));

    await waitFor(() => expect(setPriceAsync).toHaveBeenCalledWith(0));
  });

  it('should convert the entered sats to millisatoshis', async () => {
    mockStoredOffer(null);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('25000'));
    act(() => result.current.onSubmit(submit()));

    await waitFor(() => expect(setPriceAsync).toHaveBeenCalledWith(25_000_000));
  });

  it('should write the raised ceiling', async () => {
    mockStoredOffer(50_000_000, 4);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onMaxSeatsChange('8'));
    act(() => result.current.onSubmit(submit()));

    await waitFor(() => expect(setCapacityAsync).toHaveBeenCalledWith(8));
  });

  // A write of the value already stored still rotates the offer epoch, which
  // invalidates quotes nobody asked to invalidate.
  it('should not write the ceiling when the operator left it alone', async () => {
    mockStoredOffer(50_000_000, 4);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('25000'));
    act(() => result.current.onSubmit(submit()));

    await waitFor(() => expect(setPriceAsync).toHaveBeenCalled());
    expect(setCapacityAsync).not.toHaveBeenCalled();
  });

  it('should not write the price when the operator only changed the ceiling', async () => {
    mockStoredOffer(50_000_000, 4);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onMaxSeatsChange('8'));
    act(() => result.current.onSubmit(submit()));

    await waitFor(() => expect(setCapacityAsync).toHaveBeenCalledWith(8));
    expect(setPriceAsync).not.toHaveBeenCalled();
  });

  it('should write both when both changed', async () => {
    mockStoredOffer(50_000_000, 4);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onMaxSeatsChange('8'));
    act(() => result.current.onPriceChange('25000'));
    act(() => result.current.onSubmit(submit()));

    await waitFor(() => expect(setPriceAsync).toHaveBeenCalledWith(25_000_000));
    expect(setCapacityAsync).toHaveBeenCalledWith(8);
  });

  // The ceiling is the guarded write, so a refusal there has to leave the price
  // exactly as it was rather than half-applying the operator's intent.
  it('should not write the price when the ceiling was refused', async () => {
    mockStoredOffer(50_000_000, 4);
    setCapacityAsync.mockRejectedValue(new Error('cannot set max seats to 2; 4 seats are active'));

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onMaxSeatsChange('2'));
    act(() => result.current.onPriceChange('25000'));
    act(() => result.current.onSubmit(submit()));

    await waitFor(() => expect(setCapacityAsync).toHaveBeenCalledWith(2));
    expect(setPriceAsync).not.toHaveBeenCalled();
  });

  it('should report a validation error and not write an invalid price', () => {
    mockStoredOffer(null);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('-5'));
    act(() => result.current.onSubmit(submit()));

    expect(result.current.error).toBe('A price cannot be negative.');
    expect(setPriceAsync).not.toHaveBeenCalled();
  });

  it('should report a validation error and write nothing for a fractional ceiling', () => {
    mockStoredOffer(50_000_000);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onMaxSeatsChange('2.5'));
    act(() => result.current.onSubmit(submit()));

    expect(result.current.error).toBe('Seats cannot be fractional.');
    expect(setCapacityAsync).not.toHaveBeenCalled();
    expect(setPriceAsync).not.toHaveBeenCalled();
  });

  it('should clear a validation error once the field is edited again', () => {
    mockStoredOffer(null);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('-5'));
    act(() => result.current.onSubmit(submit()));
    act(() => result.current.onPriceChange('5'));

    expect(result.current.error).toBe(null);
  });

  it('should clear a validation error once the seats field is edited again', () => {
    mockStoredOffer(50_000_000);

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onMaxSeatsChange('2.5'));
    act(() => result.current.onSubmit(submit()));
    act(() => result.current.onMaxSeatsChange('8'));

    expect(result.current.error).toBe(null);
  });

  it('should report a content disposition once both reads have answered', () => {
    mockStoredOffer(50_000_000);

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.disposition).toEqual({ kind: 'content' });
  });

  it('should hand a first-read failure to the surface and block submission', () => {
    const loadFailure = new Error('boom');
    mockOfferError(loadFailure);
    mockCapacity();
    mockSetPrice();
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.canSubmit).toBe(false);
    expect(result.current.disposition).toEqual({ kind: 'failed', error: loadFailure });
  });

  // The ceiling is now part of what the form claims, so a screen that cannot
  // read it may not offer to overwrite it either.
  it('should block submission when the ceiling has never been read', () => {
    const loadFailure = new Error('capacity unreadable');
    mockOffer(50_000_000);
    mockCapacityError(loadFailure);
    mockSetPrice();
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.canSubmit).toBe(false);
    expect(result.current.disposition).toEqual({ kind: 'failed', error: loadFailure });
  });

  it('should keep the load failure off the form error line', () => {
    mockOfferError(new Error('boom'));
    mockCapacity();
    mockSetPrice();
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.error).toBe(null);
  });

  it('should allow submission once the offer loads successfully', () => {
    mockStoredOffer(50_000_000);

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.canSubmit).toBe(true);
  });

  it('should not mutate when submit is called directly while the offer query has failed', () => {
    mockOfferError(new Error('boom'));
    mockCapacity();
    mockSetPrice();
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onSubmit(submit()));

    expect(setPriceAsync).not.toHaveBeenCalled();
    expect(setCapacityAsync).not.toHaveBeenCalled();
  });

  // The fault this conversion exists to remove: the operator has the stored
  // price in front of them, one background poll fails, and the Save control
  // they were about to press goes away.
  it('should keep Save available when a background refresh fails but the price is known', () => {
    mockOfferRefreshFailure(50_000_000, new Error('daemon blip'));
    mockCapacity();
    mockSetPrice();
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.canSubmit).toBe(true);
    expect(result.current.disposition).toEqual({ kind: 'stale', updatedAtMs: ANSWERED_AT_MS });
  });

  it('should still write the price when a background refresh has failed', async () => {
    mockOfferRefreshFailure(50_000_000, new Error('daemon blip'));
    mockCapacity();
    mockSetPrice();
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('12000'));
    act(() => result.current.onSubmit(submit()));

    await waitFor(() => expect(setPriceAsync).toHaveBeenCalledWith(12_000_000));
  });

  // The second error channel. A load error used to occupy this line, so a
  // failed save could be reported as a failed read.
  it('should show the submission failure on its own error line', () => {
    const writeFailure = new Error('the daemon refused the price');
    mockOffer(50_000_000);
    mockCapacity();
    mockSetPrice({ isError: true, error: writeFailure });
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.error).toBe(describeActionError(writeFailure));
  });

  // The refusal an operator who over-shrinks their fleet has to read, in the
  // daemon's own words — the UI does not re-derive the active seat count.
  it('should show the below-active-seats refusal on the form error line', () => {
    const refusal = new Error('cannot set max seats to 2; 4 seats are active');
    mockOffer(50_000_000);
    mockCapacity();
    mockSetPrice();
    mockSetCapacity({ isError: true, error: refusal });

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.error).toBe('cannot set max seats to 2; 4 seats are active');
  });

  it('should keep the submission failure on its own line while a refresh is failing', () => {
    const writeFailure = new Error('the daemon refused the price');
    mockOfferRefreshFailure(50_000_000, new Error('daemon blip'));
    mockCapacity();
    mockSetPrice({ isError: true, error: writeFailure });
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.error).toBe(describeActionError(writeFailure));
  });

  it('should report a pending write while the ceiling is being saved', () => {
    mockOffer(50_000_000);
    mockCapacity();
    mockSetPrice();
    mockSetCapacity({ isPending: true });

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.isPending).toBe(true);
  });

  it('should force a fresh read of both the offer and the ceiling when retried', () => {
    mockOfferError(new Error('boom'));
    mockCapacity();
    mockSetPrice();
    mockSetCapacity();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.retry());

    expect(refetch).toHaveBeenCalledTimes(1);
    expect(refetchCapacity).toHaveBeenCalledTimes(1);
  });
});
