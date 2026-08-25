import type { RestoreBackupRequest, RestoreBackupResponse } from '@operator-ui/types';
import { useMutation } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

// restore_backup applies the archive's state to the daemon. Only reachable
// from the restore-mode recovery console — never wired into the normal
// dashboard.
export const useRestoreBackup = () =>
  useMutation({
    mutationFn: (request: RestoreBackupRequest) =>
      adminCall<RestoreBackupRequest, RestoreBackupResponse>('restore_backup', request)
  });
