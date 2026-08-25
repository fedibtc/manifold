import type { AttestationRemoveRequest, AttestationRemoveResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { ATTESTATION_LIST_KEY } from '@/features/attestations/api/hooks/use-attestation-list/useAttestationList';
import { adminCall } from '@/shared/api/adminCall';

export const useRemoveAttestation = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: AttestationRemoveRequest) =>
      adminCall<AttestationRemoveRequest, AttestationRemoveResponse>('attestation_remove', request),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ATTESTATION_LIST_KEY })
  });
};
