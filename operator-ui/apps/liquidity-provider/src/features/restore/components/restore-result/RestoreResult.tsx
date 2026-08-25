import { Banner } from '@operator-ui/common-ui';
import type { RestoreBackupResponse, SetupValidationCheck } from '@operator-ui/types';
import styles from './RestoreResult.module.css';

interface RestoreResultProps {
  result: RestoreBackupResponse;
}

const renderCheck = (check: SetupValidationCheck) => (
  <li key={check.name}>
    {check.name}
    {check.detail ? ` — ${check.detail}` : ''}
  </li>
);

const renderGroup = (group: string) => <li key={group}>{group}</li>;

export const RestoreResult = ({ result }: RestoreResultProps) => {
  const { status, validation, restored_state_groups } = result;
  const failedChecks = validation.checks.filter((check) => check.status !== 'passed');

  return (
    <div className={styles.page}>
      <div className={styles.card}>
        <h1 className={styles.title}>Restore complete</h1>

        <p className={styles.intro}>Status: {status}</p>

        {failedChecks.length > 0 ? (
          <Banner variant="warn" title="Some checks did not pass">
            <ul className={styles.list}>{failedChecks.map(renderCheck)}</ul>
          </Banner>
        ) : (
          <Banner variant="success">All validation checks passed.</Banner>
        )}

        <div className={styles.section}>
          <span className={styles.sectionHeading}>Restored state groups</span>

          <ul className={styles.list}>{restored_state_groups.map(renderGroup)}</ul>
        </div>

        <Banner variant="info" title="Restart required">
          Restart the daemon to bring it out of restore mode and resume normal operation.
        </Banner>
      </div>
    </div>
  );
};
