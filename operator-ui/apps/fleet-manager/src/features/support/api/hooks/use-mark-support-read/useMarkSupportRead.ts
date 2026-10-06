import type { MarkSupportReadResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { SUPPORT_KEY } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { adminCall } from '@/shared/api/adminCall';

export const useMarkSupportRead = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (upTo: string) =>
      adminCall<MarkSupportReadResponse>({ MarkSupportRead: { up_to: upTo } }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: SUPPORT_KEY });
    }
  });
};
