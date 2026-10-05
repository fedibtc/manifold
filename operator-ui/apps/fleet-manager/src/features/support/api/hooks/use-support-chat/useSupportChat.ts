import type { SupportChatResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { SUPPORT_CHAT_POLL_MS } from '@/shared/api/pollingIntervals';

export const SUPPORT_KEY = ['support'] as const;

// The daemon reads relays itself and answers from its database, so this poll
// is local. The sidebar badge and the chat page share it.
export const useSupportChat = () =>
  useQuery({
    queryKey: SUPPORT_KEY,
    refetchInterval: SUPPORT_CHAT_POLL_MS,
    queryFn: () => adminCall<SupportChatResponse>('SupportChat')
  });
