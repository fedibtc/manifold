import type { ShowMnemonicResponse } from '@operator-ui/types';
import { useMutation } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

// A mutation, not a query: ShowMnemonic must fire only on explicit operator action,
// never on mount/refetch/cache-restore. That keeps the fleet's root mnemonic out of
// the query cache, but not out of the MutationCache, which holds a settled
// mutation's result for gcTime after its last observer goes. gcTime: 0 collects
// that entry as soon as nothing observes it, and the revealing screen resets the
// mutation on its way out, so the phrase does not outlive the screen that showed it.
export const useShowMnemonic = () =>
  useMutation({
    gcTime: 0,
    mutationFn: () => adminCall<ShowMnemonicResponse>('ShowMnemonic')
  });
