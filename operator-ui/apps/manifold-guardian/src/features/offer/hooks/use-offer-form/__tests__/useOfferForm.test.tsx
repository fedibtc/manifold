import { act, renderHook } from '@testing-library/react';
import type { FormEvent, ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('@/shared/api/hooks/use-offer/useOffer', () => ({
  useOffer: vi.fn(),
  OFFER_KEY: ['offer']
}));
vi.mock('@/shared/api/hooks/use-set-price/useSetPrice', () => ({ useSetPrice: vi.fn() }));

import { useOffer } from '@/shared/api/hooks/use-offer/useOffer';
import { useSetPrice } from '@/shared/api/hooks/use-set-price/useSetPrice';
import { describeActionError } from '@/shared/utils/describeActionError';
import { useOfferForm } from '../useOfferForm';

const ANSWERED_AT_MS = 1_760_000_000_000;

const refetch = vi.fn();

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

const mutate = vi.fn();

const mockSetPrice = (overrides: Record<string, unknown> = {}): void => {
  vi.mocked(useSetPrice).mockReturnValue({
    mutate,
    isPending: false,
    isError: false,
    error: null,
    ...overrides
  } as unknown as ReturnType<typeof useSetPrice>);
};

const wrapper = ({ children }: { children: ReactNode }) => <MemoryRouter>{children}</MemoryRouter>;

const submit = () => ({ preventDefault: vi.fn() }) as unknown as FormEvent<HTMLFormElement>;

describe('useOfferForm', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    mutate.mockReset();
    refetch.mockReset();
  });

  it('should seed the field with the stored price in sats', () => {
    mockOffer(50_000_000);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.priceSats).toBe('50000');
  });

  it('should leave the field blank when the fleet is not selling', () => {
    mockOffer(null);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.priceSats).toBe('');
  });

  it('should submit a blank field as a null price', () => {
    mockOffer(50_000_000);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange(''));
    act(() => result.current.onSubmit(submit()));

    expect(mutate).toHaveBeenCalledWith(null, expect.anything());
  });

  it('should submit a zero price as a free seat', () => {
    mockOffer(null);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('0'));
    act(() => result.current.onSubmit(submit()));

    expect(mutate).toHaveBeenCalledWith(0, expect.anything());
  });

  it('should convert the entered sats to millisatoshis', () => {
    mockOffer(null);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('25000'));
    act(() => result.current.onSubmit(submit()));

    expect(mutate).toHaveBeenCalledWith(25_000_000, expect.anything());
  });

  it('should report a validation error and not write an invalid price', () => {
    mockOffer(null);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('-5'));
    act(() => result.current.onSubmit(submit()));

    expect(result.current.error).toBe('A price cannot be negative.');
    expect(mutate).not.toHaveBeenCalled();
  });

  it('should clear a validation error once the field is edited again', () => {
    mockOffer(null);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('-5'));
    act(() => result.current.onSubmit(submit()));
    act(() => result.current.onPriceChange('5'));

    expect(result.current.error).toBe(null);
  });

  it('should report a content disposition once the offer has answered', () => {
    mockOffer(50_000_000);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.disposition).toEqual({ kind: 'content' });
  });

  it('should hand a first-read failure to the surface and block submission', () => {
    const loadFailure = new Error('boom');
    mockOfferError(loadFailure);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.canSubmit).toBe(false);
    expect(result.current.disposition).toEqual({ kind: 'failed', error: loadFailure });
  });

  it('should keep the load failure off the form error line', () => {
    mockOfferError(new Error('boom'));
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.error).toBe(null);
  });

  it('should allow submission after the offer loads and the operator edits it', () => {
    mockOffer(50_000_000);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.canSubmit).toBe(false);
    act(() => result.current.onPriceChange('12000'));
    expect(result.current.canSubmit).toBe(true);
  });

  it('should mark the price saved after a successful write', () => {
    mockOffer(50_000_000);
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('12000'));
    act(() => result.current.onSubmit(submit()));
    const options = mutate.mock.calls[0][1] as { onSuccess: () => void };
    act(() => options.onSuccess());

    expect(result.current.canSubmit).toBe(false);
  });

  it('should not mutate when submit is called directly while the offer query has failed', () => {
    mockOfferError(new Error('boom'));
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onSubmit(submit()));

    expect(mutate).not.toHaveBeenCalled();
  });

  // The fault this conversion exists to remove: the operator has the stored
  // price in front of them, one background poll fails, and the Save control
  // they were about to press goes away.
  it('should keep Save available when a background refresh fails but the price is known', () => {
    mockOfferRefreshFailure(50_000_000, new Error('daemon blip'));
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('12000'));
    expect(result.current.canSubmit).toBe(true);
    expect(result.current.disposition).toEqual({ kind: 'stale', updatedAtMs: ANSWERED_AT_MS });
  });

  it('should still write the price when a background refresh has failed', () => {
    mockOfferRefreshFailure(50_000_000, new Error('daemon blip'));
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.onPriceChange('12000'));
    act(() => result.current.onSubmit(submit()));

    expect(mutate).toHaveBeenCalledWith(12_000_000, expect.anything());
  });

  // The second error channel. A load error used to occupy this line, so a
  // failed save could be reported as a failed read.
  it('should show the submission failure on its own error line', () => {
    const writeFailure = new Error('the daemon refused the price');
    mockOffer(50_000_000);
    mockSetPrice({ isError: true, error: writeFailure });

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.error).toBe(describeActionError(writeFailure));
  });

  it('should keep the submission failure on its own line while a refresh is failing', () => {
    const writeFailure = new Error('the daemon refused the price');
    mockOfferRefreshFailure(50_000_000, new Error('daemon blip'));
    mockSetPrice({ isError: true, error: writeFailure });

    const { result } = renderHook(() => useOfferForm(), { wrapper });

    expect(result.current.error).toBe(describeActionError(writeFailure));
  });

  it('should force a fresh read of the offer when retried', () => {
    mockOfferError(new Error('boom'));
    mockSetPrice();

    const { result } = renderHook(() => useOfferForm(), { wrapper });
    act(() => result.current.retry());

    expect(refetch).toHaveBeenCalledTimes(1);
  });
});
