import { Banner, Button, SectionCard } from '@operator-ui/common-ui';
import { useCreateBackup } from '@/features/settings/hooks/use-create-backup/useCreateBackup';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './BackupCard.module.css';

// Creates a fresh backup archive on demand. The daemon writes the archive to
// its own filesystem and returns the path — there is no browser-side byte
// transport yet, so the UI can only point the operator at where it landed.
export const BackupCard = () => {
  const createBackup = useCreateBackup();

  const handleCreateBackup = () => {
    createBackup.mutate();
  };

  return (
    <SectionCard title="Backup">
      <p className={styles.intro}>Create a full backup archive of this provider's state.</p>

      <Button variant="primary" onClick={handleCreateBackup} loading={createBackup.isPending}>
        Create backup
      </Button>

      {createBackup.isError ? (
        <Banner variant="error" title="Couldn't create backup">
          {describeActionError(createBackup.error)}
        </Banner>
      ) : null}

      {createBackup.isSuccess && createBackup.data ? (
        <Banner variant="success" title="Backup created">
          <p className={styles.intro}>
            Backup written on the daemon host at the path below. Copy it for use with
            inspect_backup/restore_backup on that host. Browser download/restore is not yet
            supported.
          </p>

          <code className={styles.path}>{createBackup.data.archive}</code>

          <ul className={styles.manifestList}>
            <li>Version {createBackup.data.manifest.version}</li>

            <li>State groups: {createBackup.data.manifest.state_groups.join(', ')}</li>
          </ul>
        </Banner>
      ) : null}
    </SectionCard>
  );
};
