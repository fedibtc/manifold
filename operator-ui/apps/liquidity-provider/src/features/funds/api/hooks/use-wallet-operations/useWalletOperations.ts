import type {
  ListWalletOperationsResponse,
  WalletOperationStatus,
  WalletOperationSummary
} from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { WALLET_OPERATIONS_KEY } from '@/features/funds/api/hooks/use-funds/useFunds';
import { adminCall } from '@/shared/api/adminCall';
import { POLL_ACTIVE_MS, POLL_STANDARD_MS } from '@/shared/api/pollingIntervals';

const WALLET_OPERATIONS_PAGE_LIMIT = 50;

const ACTIVE_STATUSES: ReadonlySet<WalletOperationStatus> = new Set(['pending', 'broadcast']);

interface WalletOperationsOptions {
  // Arm the fast (5s) watch immediately after a top-up, before the new deposit
  // op has appeared in the list (US-FLIP-061 deposit watch).
  watch?: boolean;
}

// Poll at 5s while a deposit watch is armed or any visible op is pending/broadcast;
// idle at 30s otherwise. Exported for unit testing the cadence rule.
export const walletOperationsInterval = (
  operations: readonly WalletOperationSummary[] | undefined,
  watch: boolean
): number => {
  if (watch) return POLL_ACTIVE_MS;
  const active = (operations ?? []).some((op) => ACTIVE_STATUSES.has(op.status));
  return active ? POLL_ACTIVE_MS : POLL_STANDARD_MS;
};

export const useWalletOperations = (options?: WalletOperationsOptions) =>
  useQuery({
    retry: false,
    staleTime: 4_000,
    queryKey: WALLET_OPERATIONS_KEY,
    refetchOnWindowFocus: true,
    refetchInterval: (query) =>
      walletOperationsInterval(query.state.data?.operations.items, options?.watch ?? false),
    queryFn: () =>
      adminCall<{ page: { limit: number } }, ListWalletOperationsResponse>(
        'list_wallet_operations',
        {
          page: { limit: WALLET_OPERATIONS_PAGE_LIMIT }
        }
      )
  });
