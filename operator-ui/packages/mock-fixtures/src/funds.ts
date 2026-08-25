// Funds + wallet-operation fixtures for the app mock servers.
// Typed against @operator-ui/types; keep aligned with the Rust admin surface.

import type { GetFundsResponse, WalletOperationSummary } from '@operator-ui/types';
// Both are the real Rust-generated contract fixtures (see A4 remediation
// task), so this scenario data can't drift from what the daemon's serde
// impls actually produce. Regenerate via `just gen-contract-fixtures`; never
// hand-edit the JSON.
import fundsFixture from '@operator-ui/types/fixtures/funds.json';
import pagingFixture from '@operator-ui/types/fixtures/paging.json';

// A healthy funds snapshot: balance above the warning threshold, gateway online.
export const healthyFunds: GetFundsResponse = fundsFixture as GetFundsResponse;

// A critical funds snapshot: balance below the critical threshold, gateway down.
export const criticalFunds: GetFundsResponse = {
  balance: {
    spendable: 80_000,
    pending_incoming: 0,
    pending_outgoing: 20_000,
    in_flight_allocations: 40_000,
    fee_reserve: 150_000,
    available_balance: 0
  },
  replenishment: 'critical',
  gateway: {
    gateway_id: 'gw-signet-01',
    gateway_name: 'Mock Signet Gateway',
    status: 'unavailable',
    available_amount: 0,
    observed_at: 1721476800
  },
  stability_pool: {
    status: 'disabled',
    available_amount: 0,
    observed_at: null
  },
  effective_liquidity: [{ source_type: 'gateway', gateway_id: 'gw-signet-01', amount: 0 }]
};

// A page of recent wallet operations, newest first. The first two come
// straight from the Rust-generated paging fixture; a third, older operation
// is added here for scenarios that need more than one page's worth of data.
export const walletOperationsPage: WalletOperationSummary[] = [
  ...(pagingFixture.operations.items as WalletOperationSummary[]),
  {
    operation_id: 'wop-0001',
    operation_type: 'withdrawal',
    amount: 250_000,
    status: 'pending',
    federation_id: null,
    created_at: 1721304000,
    updated_at: 1721304000
  }
];
