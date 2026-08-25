import { Banner, Button } from '@operator-ui/common-ui';
import { type ChangeEvent, useState } from 'react';
import { RestoreResult } from '@/features/restore/components/restore-result/RestoreResult';
import { useInspectBackup } from '@/features/restore/hooks/use-inspect-backup/useInspectBackup';
import { useRestoreBackup } from '@/features/restore/hooks/use-restore-backup/useRestoreBackup';
import { useRestoreToken } from '@/features/restore/hooks/use-restore-token/useRestoreToken';
import { describeActionError } from '@/shared/utils/describeActionError';
import styles from './RestoreConsolePage.module.css';

// Standalone recovery console rendered by BootGate when the daemon reports
// restore mode (see isRestoreMode). Never routed — reached only via the boot
// gate, outside the normal shell and nav. inspect_backup/restore_backup are
// the most destructive calls in the app, so this requires an operator token
// (same in-memory-only tokenStore as AuthPromptPage) before either is
// reachable — the restore-mode boot path skips the normal G2 auth gate
// entirely, so without this the archive controls below would be callable by
// anyone who can reach the page, with no credential at all.
export const RestoreConsolePage = () => {
  const { tokenEntered, tokenValue, onTokenChange, onTokenSubmit } = useRestoreToken();
  const [archive, setArchive] = useState('');
  const [confirming, setConfirming] = useState(false);
  const inspectBackup = useInspectBackup();
  const restoreBackup = useRestoreBackup();

  const handleArchiveChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    setArchive(event.target.value);
  };

  const handleInspect = () => inspectBackup.mutate({ archive });
  const handleRestoreStart = () => setConfirming(true);
  const handleRestoreBack = () => setConfirming(false);
  const handleRestoreConfirm = () => {
    setConfirming(false);
    restoreBackup.mutate({ archive });
  };

  const manifest = inspectBackup.data?.manifest;

  if (!tokenEntered) {
    return (
      <div className={styles.page}>
        <div className={styles.card}>
          <h1 className={styles.title}>Restore console</h1>

          <p className={styles.intro}>
            This daemon booted in restore mode. Enter the operator token before inspecting or
            restoring a backup.
          </p>

          <form className={styles.form} onSubmit={onTokenSubmit}>
            <label className={styles.fieldLabel} htmlFor="restore-token">
              Admin token
            </label>

            <span className={styles.hint}>
              Kept in memory for this tab only — never stored in the browser.
            </span>

            <input
              id="restore-token"
              type="password"
              className={styles.input}
              placeholder="Paste your token"
              value={tokenValue}
              onChange={onTokenChange}
            />

            <Button variant="primary" type="submit">
              Continue
            </Button>
          </form>
        </div>
      </div>
    );
  }

  if (restoreBackup.isSuccess && restoreBackup.data) {
    return <RestoreResult result={restoreBackup.data} />;
  }

  return (
    <div className={styles.page}>
      <div className={styles.card}>
        <h1 className={styles.title}>Restore console</h1>

        <p className={styles.intro}>
          This daemon booted in restore mode. Enter the path to a backup archive on this daemon's
          filesystem to inspect it, then restore to bring its state back.
        </p>

        <label className={styles.fieldLabel} htmlFor="restore-archive">
          Backup archive path (on this daemon's filesystem)
        </label>

        <textarea
          id="restore-archive"
          className={styles.textarea}
          value={archive}
          onChange={handleArchiveChange}
        />

        <Button variant="primary" onClick={handleInspect} loading={inspectBackup.isPending}>
          Inspect backup
        </Button>

        {inspectBackup.isError ? (
          <Banner variant="error" title="Couldn't inspect backup">
            {describeActionError(inspectBackup.error)}
          </Banner>
        ) : null}

        {manifest ? (
          <div className={styles.section}>
            <ul className={styles.list}>
              <li>Version {manifest.version}</li>

              <li>State groups: {manifest.state_groups.join(', ')}</li>
            </ul>

            {confirming ? (
              <div className={styles.confirm}>
                <span className={styles.confirmLabel}>Restore this backup?</span>

                <div className={styles.confirmActions}>
                  <Button variant="danger" onClick={handleRestoreConfirm}>
                    Confirm restore
                  </Button>

                  <Button variant="secondary" onClick={handleRestoreBack}>
                    Back
                  </Button>
                </div>
              </div>
            ) : (
              <Button
                variant="danger"
                loading={restoreBackup.isPending}
                onClick={handleRestoreStart}
              >
                Restore from this backup
              </Button>
            )}

            {restoreBackup.isError ? (
              <Banner variant="error" title="Couldn't restore backup">
                {describeActionError(restoreBackup.error)}
              </Banner>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
};
