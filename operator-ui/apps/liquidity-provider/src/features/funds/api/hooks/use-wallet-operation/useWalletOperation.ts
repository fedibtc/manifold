import type { GetWalletOperationRequest, GetWalletOperationResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

export const walletOperationKey = (operationId: string) =>
  ['wallet-operation', operationId] as const;

// One operation in full. The list this is opened from carries only id, type,
// amount, status and timestamps; an operator resolving a send held for manual
// review has to see where it was going and what chain evidence exists, and
// neither is in the list shape.
//
// Not polled. It is read when a resolution panel opens, and a frozen operation
// does not move on its own — only the resolution moves it, which invalidates
// this key.
export const useWalletOperation = (operationId: string | null) =>
  useQuery({
    retry: false,
    enabled: operationId !== null,
    queryKey: walletOperationKey(operationId ?? ''),
    queryFn: () =>
      adminCall<GetWalletOperationRequest, GetWalletOperationResponse>('get_wallet_operation', {
        operation_id: operationId as string
      })
  });
