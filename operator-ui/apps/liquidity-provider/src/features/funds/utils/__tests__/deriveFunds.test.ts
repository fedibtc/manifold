import type { GetFundsResponse } from '@operator-ui/types';
import { describe, expect, it } from 'vitest';
import { deriveFunds } from '../deriveFunds';

const snapshot: GetFundsResponse = {
  balance: {
    spendable: 4_200_000,
    pending_incoming: 150_000,
    pending_outgoing: 50_000,
    in_flight_allocations: 800_000,
    fee_reserve: 150_000,
    available_balance: 3_250_000
  },
  replenishment: 'ok',
  gateway: {
    gateway_id: 'gw-signet-01',
    gateway_name: 'Mock Signet Gateway',
    status: 'available',
    available_amount: 3_000_000,
    observed_at: 1721476800
  },
  stability_pool: { status: 'unavailable', available_amount: 250_000, observed_at: 1721476800 },
  effective_liquidity: [{ source_type: 'gateway', gateway_id: 'gw-signet-01', amount: 3_000_000 }]
};

describe('deriveFunds', () => {
  it('should expose the available balance and a no-banner ok state for a healthy snapshot', () => {
    const model = deriveFunds(snapshot);

    expect(model.availableBalance).toBe(3_250_000);
    expect(model.banner).toBeNull();
    expect(model.balanceChip).toEqual({ label: 'Above thresholds', tone: 'ok' });
  });

  it('should build one balance row per component with the available row marked strong', () => {
    const model = deriveFunds(snapshot);

    expect(model.balanceRows).toHaveLength(6);
    expect(model.balanceRows[0]).toEqual({
      key: 'spendable',
      label: 'Spendable',
      value: 4_200_000
    });
    expect(model.balanceRows[5]).toEqual({
      key: 'available_balance',
      label: 'Available',
      value: 3_250_000,
      strong: true
    });
  });

  it('should map gateway and stability pool to source rows carrying their wire status', () => {
    const model = deriveFunds(snapshot);

    expect(model.sourceRows).toEqual([
      { key: 'gateway', source: 'Mock Signet Gateway', available: 3_000_000, status: 'available' },
      { key: 'stability_pool', source: 'Stability pool', available: 250_000, status: 'unavailable' }
    ]);
  });

  it('should surface the critical banner and rejections flag for a critical snapshot', () => {
    const model = deriveFunds({ ...snapshot, replenishment: 'critical' });

    expect(model.banner).toEqual({
      variant: 'error',
      title: 'Critical balance',
      message:
        'Available balance is below the critical threshold — new requests may be rejected. Top up now.'
    });
    expect(model.balanceChip).toEqual({ label: 'Critical', tone: 'bad' });
  });

  it('should surface the warning banner for a warning snapshot', () => {
    const model = deriveFunds({ ...snapshot, replenishment: 'warning' });

    expect(model.banner?.variant).toBe('warn');
    expect(model.balanceChip).toEqual({ label: 'Below warning threshold', tone: 'warn' });
  });
});
