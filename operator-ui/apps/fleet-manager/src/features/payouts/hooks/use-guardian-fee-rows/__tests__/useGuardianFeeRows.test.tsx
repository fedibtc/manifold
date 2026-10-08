import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { walletStatus } from '@/mocks/wallet-status';
import * as adminCallModule from '@/shared/api/adminCall';
import { useGuardianFeeRows } from '../useGuardianFeeRows';

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
};

const seat = (seat_id: string, decommissioned = false) => ({
  seat_id,
  decommissioned,
  fi_id: 'fi_01',
  plan: { InfiniteBestEffort: { price_msats: 1 } },
  created_at_ms: 0,
  payment_claim: { state: 'success', at_ms: 0 },
  completion_callback: { state: 'not_configured' },
  guardian_fee: { remittance_account: '{}' }
});

const answer = (request: unknown) => {
  if (request === 'ListSeats') {
    return Promise.resolve({ seats: [seat('seat-live-01'), seat('seat-gone-01', true)] });
  }
  return Promise.resolve(fees());
};

const fees = (policy: object = { configured: true, send_ppm: 1_000 }) => ({
  collectable_msat: 16_000_000,
  wallet: walletStatus(8_000_000),
  policy
});

const answerWithPolicy = (policy: object) =>
  ((request: unknown) =>
    request === 'ListSeats'
      ? Promise.resolve({ seats: [seat('seat-live-01')] })
      : Promise.resolve(fees(policy))) as typeof adminCallModule.adminCall;

afterEach(() => {
  vi.restoreAllMocks();
});

describe('useGuardianFeeRows', () => {
  it('should split each live seat into what is pooled and what is collected', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockImplementation(
      answer as typeof adminCallModule.adminCall
    );

    const { result } = renderHook(() => useGuardianFeeRows(), { wrapper });

    await waitFor(() =>
      expect(result.current).toEqual([
        {
          seatId: 'seat-live-01',
          collectableMsat: 16_000_000,
          collectedEcashMsat: 8_000_000,
          earning: true
        }
      ])
    );
  });

  // A decommissioned seat earns nothing new; it is not what an operator comes to
  // this screen to move.
  it('should leave decommissioned seats out', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockImplementation(
      answer as typeof adminCallModule.adminCall
    );

    const { result } = renderHook(() => useGuardianFeeRows(), { wrapper });

    await waitFor(() => expect(result.current).toHaveLength(1));
    expect(result.current.map((row) => row.seatId)).not.toContain('seat-gone-01');
  });

  // The seat keeps its row: dropping it would hide money, and a zero would claim
  // there is none.
  it('should report unknown amounts for a seat whose fee account cannot be read', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockImplementation(((request: unknown) =>
      request === 'ListSeats'
        ? Promise.resolve({ seats: [seat('seat-live-01')] })
        : Promise.reject(
            new Error('seat has no federation yet')
          )) as typeof adminCallModule.adminCall);

    const { result } = renderHook(() => useGuardianFeeRows(), { wrapper });

    await waitFor(() =>
      expect(result.current).toEqual([
        { seatId: 'seat-live-01', collectableMsat: null, collectedEcashMsat: null, earning: null }
      ])
    );
  });

  it('should report a seat whose federation charges a rate of zero as not earning', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockImplementation(
      answerWithPolicy({ configured: true, send_ppm: 0 })
    );

    const { result } = renderHook(() => useGuardianFeeRows(), { wrapper });

    await waitFor(() => expect(result.current[0]?.earning).toBe(false));
  });

  it('should report a seat whose federation has no fee policy as not earning', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockImplementation(
      answerWithPolicy({ configured: false, send_ppm: null })
    );

    const { result } = renderHook(() => useGuardianFeeRows(), { wrapper });

    await waitFor(() => expect(result.current[0]?.earning).toBe(false));
  });

  it('should not guess whether a seat earns when its fee policy cannot be read', async () => {
    vi.spyOn(adminCallModule, 'adminCall').mockImplementation(
      answerWithPolicy({ policy_error: 'meta module unavailable' })
    );

    const { result } = renderHook(() => useGuardianFeeRows(), { wrapper });

    await waitFor(() => expect(result.current[0]?.collectableMsat).toBe(16_000_000));
    expect(result.current[0]?.earning).toBeNull();
  });
});
