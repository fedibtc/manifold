import type { BackupStateGroup, BackupStore } from '@operator-ui/types';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useInspectBackup } from '@/features/restore/hooks/use-inspect-backup/useInspectBackup';
import { useRestoreBackup } from '@/features/restore/hooks/use-restore-backup/useRestoreBackup';
import { AdminApiError } from '@/shared/api/errors';
import { clearToken, setToken } from '@/shared/api/tokenStore';
import { RestoreConsolePage } from '../RestoreConsolePage';

vi.mock('@/features/restore/hooks/use-inspect-backup/useInspectBackup');
vi.mock('@/features/restore/hooks/use-restore-backup/useRestoreBackup');

const useInspectBackupMock = vi.mocked(useInspectBackup);
const useRestoreBackupMock = vi.mocked(useRestoreBackup);

const inspectDefaults = {
  mutate: vi.fn(),
  isPending: false,
  isError: false,
  isSuccess: false,
  data: undefined,
  error: null
};

const restoreDefaults = {
  mutate: vi.fn(),
  isPending: false,
  isError: false,
  isSuccess: false,
  data: undefined,
  error: null
};

const stateGroups: BackupStateGroup[] = ['database', 'operator_config'];

const manifest = {
  version: 3,
  created_at: 1721476800,
  state_groups: stateGroups,
  recovery_point: {
    quiesced_at: 1721476790,
    stores: ['sqlite', 'data_directory'] as BackupStore[]
  }
};

const arrange = (
  inspectOverrides: Partial<ReturnType<typeof useInspectBackup>> = {},
  restoreOverrides: Partial<ReturnType<typeof useRestoreBackup>> = {}
) => {
  useInspectBackupMock.mockReturnValue({
    ...inspectDefaults,
    ...inspectOverrides
  } as ReturnType<typeof useInspectBackup>);
  useRestoreBackupMock.mockReturnValue({
    ...restoreDefaults,
    ...restoreOverrides
  } as ReturnType<typeof useRestoreBackup>);
};

beforeEach(() => {
  // Existing tests below exercise the archive controls, not the token gate —
  // pre-seed a token so they render past it. The gate itself is covered by
  // the two tests at the end of this file, which clear the token first.
  setToken('test-token');
});

afterEach(() => {
  clearToken();
  vi.restoreAllMocks();
});

it('should require an operator token before the archive controls are reachable', () => {
  clearToken();
  const mutate = vi.fn();
  arrange({ mutate });

  render(<RestoreConsolePage />);

  expect(screen.queryByLabelText('Backup archive')).toBeNull();
  expect(screen.getByLabelText('Admin token')).toBeTruthy();

  fireEvent.change(screen.getByLabelText('Admin token'), { target: { value: 'op-token' } });
  fireEvent.click(screen.getByText('Continue'));

  expect(screen.getByLabelText("Backup archive path (on this daemon's filesystem)")).toBeTruthy();
  fireEvent.click(screen.getByText('Inspect backup'));
  expect(mutate).toHaveBeenCalledWith({ archive: '' });
});

it('should skip the token gate on a later render when a token was already set', () => {
  arrange();

  render(<RestoreConsolePage />);

  expect(screen.getByLabelText("Backup archive path (on this daemon's filesystem)")).toBeTruthy();
});

it('should call inspect_backup with the pasted archive path', () => {
  const mutate = vi.fn();
  arrange({ mutate });

  render(<RestoreConsolePage />);
  fireEvent.change(screen.getByLabelText("Backup archive path (on this daemon's filesystem)"), {
    target: { value: 'archive-text' }
  });
  fireEvent.click(screen.getByText('Inspect backup'));

  expect(mutate).toHaveBeenCalledWith({ archive: 'archive-text' });
});

it('should render the manifest after a successful inspect', () => {
  arrange({ isSuccess: true, data: { manifest } });

  render(<RestoreConsolePage />);

  expect(screen.getByText('Version 3')).toBeTruthy();
  expect(screen.getByText('State groups: database, operator_config')).toBeTruthy();
});

it('should require a confirm step before calling restore_backup', () => {
  const restoreMutate = vi.fn();
  arrange({ isSuccess: true, data: { manifest } }, { mutate: restoreMutate });

  render(<RestoreConsolePage />);
  fireEvent.click(screen.getByText('Restore from this backup'));

  expect(restoreMutate).not.toHaveBeenCalled();
  expect(screen.getByText('Restore this backup?')).toBeTruthy();

  fireEvent.click(screen.getByText('Confirm restore'));

  expect(restoreMutate).toHaveBeenCalledWith({ archive: '' });
});

it('should return to the non-confirming view when Back is clicked', () => {
  arrange({ isSuccess: true, data: { manifest } });

  render(<RestoreConsolePage />);
  fireEvent.click(screen.getByText('Restore from this backup'));
  fireEvent.click(screen.getByText('Back'));

  expect(screen.queryByText('Restore this backup?')).toBeNull();
  expect(screen.getByText('Restore from this backup')).toBeTruthy();
});

it('should render an error banner and keep the archive editable when inspect fails', () => {
  arrange({ isError: true, error: new AdminApiError('invalid_argument', 'not a valid archive') });

  render(<RestoreConsolePage />);

  expect(screen.getByText("Couldn't inspect backup")).toBeTruthy();
  const textarea = screen.getByLabelText(
    "Backup archive path (on this daemon's filesystem)"
  ) as HTMLTextAreaElement;
  fireEvent.change(textarea, { target: { value: 'new-archive' } });
  expect(textarea.value).toBe('new-archive');
});

it('should render an error banner and allow retrying restore without losing the manifest', () => {
  arrange(
    { isSuccess: true, data: { manifest } },
    { isError: true, error: new AdminApiError('invalid_argument', 'archive mismatch') }
  );

  render(<RestoreConsolePage />);

  expect(screen.getByText("Couldn't restore backup")).toBeTruthy();
  expect(screen.getByText('Restore from this backup')).toBeTruthy();
  expect(screen.getByText('Version 3')).toBeTruthy();
});

it('should render status, failed checks, restored state groups, and a restart instruction after a successful restore', () => {
  arrange(
    {},
    {
      isSuccess: true,
      data: {
        status: 'ready',
        validation: {
          status: 'failed',
          checks: [{ name: 'wallet_check', status: 'failed', detail: 'balance mismatch' }]
        },
        restored_state_groups: stateGroups
      }
    }
  );

  render(<RestoreConsolePage />);

  expect(screen.getByText('Status: ready')).toBeTruthy();
  expect(screen.getByText(/wallet_check/)).toBeTruthy();
  expect(screen.getByText('database')).toBeTruthy();
  expect(screen.getByText('operator_config')).toBeTruthy();
  expect(screen.getByText(/Restart the daemon to bring it out of restore mode/)).toBeTruthy();
});
