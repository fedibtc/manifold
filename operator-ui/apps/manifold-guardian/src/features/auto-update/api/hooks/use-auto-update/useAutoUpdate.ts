import type { ShowAutoUpdateResponse } from '@operator-ui/types';
import { useQuery } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

export const AUTO_UPDATE_KEY = ['auto-update'] as const;

// Whether the daemon installs newer signed releases on its own
// (crates/mg/core/src/fleet.rs::auto_update_enabled).
export const useAutoUpdate = () =>
  useQuery({
    queryKey: AUTO_UPDATE_KEY,
    queryFn: () => adminCall<ShowAutoUpdateResponse>('ShowAutoUpdate')
  });
