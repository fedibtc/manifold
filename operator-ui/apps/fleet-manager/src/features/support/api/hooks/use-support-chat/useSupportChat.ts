import type { SupportChatResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

export const SUPPORT_KEY = ['support'] as const;

// The daemon holds a live relay subscription and answers from its database,
// so this poll is local. The sidebar dot and the chat page share the query,
// each at its own interval, and TanStack Query runs the shorter one.
export const useSupportChat = (refetchInterval: number) =>
  useQuery({
    queryKey: SUPPORT_KEY,
    refetchInterval,
    queryFn: () => adminCall<SupportChatResponse>('SupportChat')
  });
