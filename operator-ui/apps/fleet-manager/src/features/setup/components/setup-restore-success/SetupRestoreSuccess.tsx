import { Banner, Button } from '@operator-ui/common-ui';
import type { SafeRestoreResult } from '@/features/setup/utils/restoreViewState';
import styles from './SetupRestoreSuccess.module.css';

interface SetupRestoreSuccessProps {
  result: SafeRestoreResult;
  onContinue: () => void;
}

// The daemon answers `{ onboarded, seats, formed }` and nothing more. It does not
// say a seat is running, so neither does this screen: `seats` is a count of seat
// records recovered, `formed` the subset that carries guardian configuration.
export const SetupRestoreSuccess = ({ result, onContinue }: SetupRestoreSuccessProps) => {
  const foundNothing = result.seats === 0;

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <h1 className={styles.heading}>Recovery finished</h1>

        <p className={styles.intro}>This host now carries the identity behind that phrase.</p>
      </div>

      <ul className={styles.counts}>
        <li className={styles.countRow}>
          <span className={styles.countValue}>{result.seats}</span>

          <span className={styles.countLabel}>seat records recovered</span>
        </li>

        <li className={styles.countRow}>
          <span className={styles.countValue}>{result.formed}</span>

          <span className={styles.countLabel}>of them include guardian configuration</span>
        </li>
      </ul>
      {foundNothing ? (
        <Banner variant="warn">
          <div className={styles.callout}>
            <p>
              No seat records were found for this phrase. That has several possible causes, and this
              screen cannot tell them apart:
            </p>

            <ul className={styles.reasons}>
              <li>the fleet never sold a seat;</li>

              <li>its records are not on the relay this host reads;</li>

              <li>this host points at a different environment;</li>

              <li>the phrase is another valid phrase, but for a different fleet.</li>
            </ul>

            <p>
              The daemon has already installed this identity, so you cannot repeat setup on this
              host. Check the environment and the relay before you continue.
            </p>
          </div>
        </Banner>
      ) : null}

      <div className={styles.actions}>
        <Button onClick={onContinue}>Continue</Button>
      </div>
    </div>
  );
};
