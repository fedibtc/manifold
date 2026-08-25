import type { BackupStateGroup, BackupStore } from '@operator-ui/types';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useCreateBackup } from '@/features/settings/hooks/use-create-backup/useCreateBackup';
import { AdminApiError } from '@/shared/api/errors';
import { BackupCard } from '../BackupCard';

vi.mock('@/features/settings/hooks/use-create-backup/useCreateBackup');

const useCreateBackupMock = vi.mocked(useCreateBackup);

const backupDefaults = {
  mutate: vi.fn(),
  isPending: false,
  isError: false,
  isSuccess: false,
  data: undefined,
  error: null
};

const arrange = (overrides: Partial<ReturnType<typeof useCreateBackup>>) => {
  useCreateBackupMock.mockReturnValue({
    ...backupDefaults,
    ...overrides
  } as ReturnType<typeof useCreateBackup>);
};

afterEach(() => {
  vi.restoreAllMocks();
});

const stateGroups: BackupStateGroup[] = ['database'];
const twoStateGroups: BackupStateGroup[] = ['database', 'operator_config'];

it('should call create_backup when Create backup is clicked', () => {
  const mutate = vi.fn();
  arrange({ mutate });

  render(<BackupCard />);
  fireEvent.click(screen.getByText('Create backup'));

  expect(mutate).toHaveBeenCalled();
});

it('should present the archive as a daemon-side path, not a download', () => {
  const response = {
    archive: '/var/lib/flip/backups/flip-backup-1721476800.tar.gz',
    manifest: {
      version: 3,
      created_at: 1721476800,
      state_groups: stateGroups,
      recovery_point: {
        quiesced_at: 1721476790,
        stores: ['sqlite', 'data_directory'] as BackupStore[]
      }
    }
  };
  arrange({ isSuccess: true, data: response });

  render(<BackupCard />);

  expect(screen.getByText(/on the daemon host/i)).toBeTruthy();
  expect(screen.getByText('/var/lib/flip/backups/flip-backup-1721476800.tar.gz')).toBeTruthy();
  expect(screen.queryByRole('button', { name: /download/i })).toBeNull();
});

it('should render the manifest version and state groups after a successful backup', () => {
  const response = {
    archive: 'opaque-archive-contents',
    manifest: {
      version: 3,
      created_at: 1721476800,
      state_groups: twoStateGroups,
      recovery_point: {
        quiesced_at: 1721476790,
        stores: ['sqlite', 'data_directory'] as BackupStore[]
      }
    }
  };
  arrange({ isSuccess: true, data: response });

  render(<BackupCard />);

  expect(screen.getByText('Version 3')).toBeTruthy();
  expect(screen.getByText('State groups: database, operator_config')).toBeTruthy();
});

it('should render an error banner when create_backup fails', () => {
  arrange({ isError: true, error: new AdminApiError('internal', 'daemon exploded') });

  render(<BackupCard />);

  expect(screen.getByText("Couldn't create backup")).toBeTruthy();
  expect(screen.getByText('internal: daemon exploded')).toBeTruthy();
});
