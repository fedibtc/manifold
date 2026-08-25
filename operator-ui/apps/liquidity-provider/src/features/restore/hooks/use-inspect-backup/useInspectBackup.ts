import type { InspectBackupRequest, InspectBackupResponse } from '@operator-ui/types';
import { useMutation } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

// inspect_backup previews an archive's manifest without applying it — safe to
// call repeatedly while the operator confirms they have the right backup.
export const useInspectBackup = () =>
  useMutation({
    mutationFn: (request: InspectBackupRequest) =>
      adminCall<InspectBackupRequest, InspectBackupResponse>('inspect_backup', request)
  });
