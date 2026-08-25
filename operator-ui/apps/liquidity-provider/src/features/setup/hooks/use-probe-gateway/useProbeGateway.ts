import type { ProbeGatewayRequest, ProbeGatewayResponse } from '@operator-ui/types';
import { useMutation } from '@tanstack/react-query';
import type { ConfigDraft } from '@/features/setup/services/draft';
import { adminCall } from '@/shared/api/adminCall';
import { storeDraftSecrets } from '@/shared/config/secrets';

// Asks the gateway who it is, so the operator does not have to transcribe an
// identifier a machine can fetch — and cannot mistype one that is frozen for
// the life of the deployment.
//
// The credential is stored first, because the daemon authenticates the probe
// with the stored one rather than accepting a secret in the request.
export const useProbeGateway = () =>
  useMutation({
    mutationFn: async (draft: ConfigDraft): Promise<ProbeGatewayResponse> => {
      await storeDraftSecrets(draft);
      return adminCall<ProbeGatewayRequest, ProbeGatewayResponse>('probe_gateway', {
        admin_url: draft.gateway.admin_url
      });
    }
  });
