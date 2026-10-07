import type { SendSupportMessageResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { SUPPORT_KEY } from '@/features/support/api/hooks/use-support-chat/useSupportChat';
import { adminCall } from '@/shared/api/adminCall';

export const useSendSupportMessage = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (body: string) =>
      adminCall<SendSupportMessageResponse>({ SendSupportMessage: { body } }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: SUPPORT_KEY });
    }
  });
};
