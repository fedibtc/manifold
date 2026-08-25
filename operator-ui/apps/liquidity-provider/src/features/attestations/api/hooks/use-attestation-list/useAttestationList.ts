import type { AttestationListRequest, AttestationListResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';
import { POLL_SLOW_MS } from '@/shared/api/pollingIntervals';

export const ATTESTATION_LIST_KEY = ['attestation-list'] as const;

export const useAttestationList = () =>
  useQuery({
    retry: false,
    staleTime: 55_000,
    refetchInterval: POLL_SLOW_MS,
    queryKey: ATTESTATION_LIST_KEY,
    refetchOnWindowFocus: true,
    queryFn: () =>
      adminCall<AttestationListRequest, AttestationListResponse>('attestation_list', null)
  });
