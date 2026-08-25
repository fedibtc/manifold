import type { CreateBackupRequest, CreateBackupResponse } from '@operator-ui/types';
import { useMutation } from '@tanstack/react-query';
import { adminCall } from '@/shared/api/adminCall';

// create_backup takes no request body and produces a fresh archive + manifest
// on every call. Nothing cached elsewhere depends on it, so no invalidation.
export const useCreateBackup = () =>
  useMutation({
    mutationFn: () => adminCall<CreateBackupRequest, CreateBackupResponse>('create_backup', null)
  });
