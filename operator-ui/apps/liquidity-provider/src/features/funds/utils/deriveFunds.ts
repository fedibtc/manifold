import type { ChipTone } from '@operator-ui/common-ui';
import type {
  GetFundsResponse,
  InventoryStatus,
  ReplenishmentStatus,
  Sats
} from '@operator-ui/types';

export interface SourceRow {
  key: string;
  source: string;
  available: Sats;
  status: InventoryStatus;
}

export interface BalanceRow {
  key: string;
  label: string;
  value: Sats;
  strong?: boolean;
}

export interface FundsBanner {
  variant: 'warn' | 'error';
  title: string;
  message: string;
}

const REPLENISHMENT_BANNERS: Record<Exclude<ReplenishmentStatus, 'ok'>, FundsBanner> = {
  warning: {
    variant: 'warn',
    title: 'Replenishment recommended',
    message: 'Available balance is below the warning threshold. Top up soon.'
  },
  critical: {
    variant: 'error',
    title: 'Critical balance',
    message:
      'Available balance is below the critical threshold — new requests may be rejected. Top up now.'
  }
};

const REPLENISHMENT_CHIPS: Record<ReplenishmentStatus, { label: string; tone: ChipTone }> = {
  ok: { label: 'Above thresholds', tone: 'ok' },
  warning: { label: 'Below warning threshold', tone: 'warn' },
  critical: { label: 'Critical', tone: 'bad' }
};

export interface FundsModel {
  availableBalance: Sats;
  banner: FundsBanner | null;
  balanceChip: { label: string; tone: ChipTone };
  balanceRows: BalanceRow[];
  sourceRows: SourceRow[];
}

export const deriveFunds = (data: GetFundsResponse): FundsModel => {
  const { balance, replenishment, gateway, stability_pool } = data;

  return {
    availableBalance: balance.available_balance,
    banner: replenishment === 'ok' ? null : REPLENISHMENT_BANNERS[replenishment],
    balanceChip: REPLENISHMENT_CHIPS[replenishment],
    balanceRows: [
      { key: 'spendable', label: 'Spendable', value: balance.spendable },
      { key: 'pending_incoming', label: 'Pending incoming', value: balance.pending_incoming },
      { key: 'pending_outgoing', label: 'Pending outgoing', value: balance.pending_outgoing },
      {
        key: 'in_flight_allocations',
        label: 'In-flight allocations',
        value: balance.in_flight_allocations
      },
      { key: 'fee_reserve', label: 'Fee reserve', value: balance.fee_reserve },
      {
        key: 'available_balance',
        label: 'Available',
        value: balance.available_balance,
        strong: true
      }
    ],
    sourceRows: [
      {
        key: 'gateway',
        source: gateway.gateway_name,
        available: gateway.available_amount,
        status: gateway.status
      },
      {
        key: 'stability_pool',
        source: 'Stability pool',
        available: stability_pool.available_amount,
        status: stability_pool.status
      }
    ]
  };
};
