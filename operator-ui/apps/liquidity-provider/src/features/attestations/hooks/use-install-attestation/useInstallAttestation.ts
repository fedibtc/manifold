import type { AttestationInstallRequest, AttestationInstallResponse } from '@operator-ui/types';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { ATTESTATION_LIST_KEY } from '@/features/attestations/api/hooks/use-attestation-list/useAttestationList';
import { adminCall } from '@/shared/api/adminCall';

export const useInstallAttestation = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (file: File) => {
      const payload = Array.from(new Uint8Array(await file.arrayBuffer()));
      return adminCall<AttestationInstallRequest, AttestationInstallResponse>(
        'attestation_install',
        { payload }
      );
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ATTESTATION_LIST_KEY })
  });
};
