import type { GuardianLinkResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { adminCall } from '@/shared/api/adminCall';
import { GUARDIAN_LINK_POLL_MS } from '@/shared/api/pollingIntervals';

export const GUARDIAN_LINK_KEY = ['guardian-link'] as const;

export const useGuardianLink = () => {
  const [now, setNow] = useState(Date.now);
  const query = useQuery({
    queryKey: GUARDIAN_LINK_KEY,
    queryFn: () => adminCall<GuardianLinkResponse>('GuardianLink'),
    refetchInterval: GUARDIAN_LINK_POLL_MS,
    gcTime: 0
  });
  const expiresAt = query.data?.offer?.expires_at;
  useEffect(() => {
    if (expiresAt == null) return;
    const timer = window.setTimeout(
      () => setNow(Date.now()),
      Math.max(0, expiresAt * 1000 - Date.now() + 1)
    );
    return () => window.clearTimeout(timer);
  }, [expiresAt]);

  const data = query.data;
  const offer = data?.offer && data.offer.expires_at * 1000 >= now ? data.offer : null;
  return { ...query, data: data ? { ...data, offer } : undefined };
};
